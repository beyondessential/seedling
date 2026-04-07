use std::{
    borrow::Cow,
    net::{IpAddr, Ipv6Addr},
};

use futures_util::StreamExt;
use nftables::{
    batch::Batch,
    expr::{Expression, Meta, MetaKey, NamedExpression, Payload, PayloadField, Prefix},
    helper,
    schema::{Chain, FlushObject, NfCmd, NfListObject, Rule, Table},
    stmt::{Match, NAT, NATFamily, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};
use rtnetlink::{
    Handle, RouteMessageBuilder, RouteNextHopBuilder, new_connection,
    packet_route::route::{RouteAddress, RouteAttribute, RouteProtocol, RouteType},
};
use snafu::Snafu;

use crate::system::{
    DataPlane,
    types::{DataPlaneRules, ForwardProto, IngressRule, MountRule, ServiceRoute},
};

const TABLE: &str = "seedling_net";
const CHAIN_PRE: &str = "prerouting";
const CHAIN_FWD: &str = "forward";
const PRIO_DSTNAT: i32 = -100;
const PRIO_FILTER: i32 = 0;

/// Netfilter protocol number for IPv6 (`NFPROTO_IPV6`).
/// Used to guard inet-table ingress rules so they only match IPv6 packets.
/// IPv4 ingress support requires a dual-stack `seedling-proxy` network and
/// is deferred (NAT64 / NAT46 out of scope for the initial implementation).
const NFPROTO_IPV6: u32 = 10;

#[derive(Debug, Snafu)]
pub(crate) enum DataPlaneError {
    #[snafu(display("nftables error: {source}"))]
    Nftables {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("rtnetlink error: {source}"))]
    Netlink {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },
    #[snafu(display("spawn_blocking task panicked"))]
    JoinError { source: tokio::task::JoinError },
}

pub(crate) struct NftablesDataPlane {
    route_handle: Handle,
}

impl NftablesDataPlane {
    pub(crate) fn new() -> std::io::Result<Self> {
        let (connection, handle, _) = new_connection()?;
        tokio::spawn(connection);
        Ok(Self {
            route_handle: handle,
        })
    }
}

impl DataPlane for NftablesDataPlane {
    type Error = DataPlaneError;

    async fn apply_rules(&self, rules: &DataPlaneRules) -> Result<(), Self::Error> {
        let mut batch = Batch::new();
        batch.add(nft_table());
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(table())));
        batch.add(prerouting_chain());
        batch.add(forward_chain());

        for rule in &rules.ingress {
            for stmts in ingress_rule_stmts(rule) {
                batch.add(rule_obj(CHAIN_PRE, stmts));
            }
        }

        for rule in &rules.mounts {
            for stmts in mount_rule_stmts(rule) {
                batch.add(rule_obj(CHAIN_PRE, stmts));
            }
        }

        batch.add(rule_obj(CHAIN_FWD, seedling_forward_stmts()));

        let nft = batch.to_nftables();
        helper::apply_ruleset_async(&nft)
            .await
            .map_err(|e| DataPlaneError::Nftables {
                source: Box::new(e),
            })
    }

    async fn apply_routes(&self, routes: &[ServiceRoute]) -> Result<(), Self::Error> {
        self.delete_managed_routes().await?;
        for svc in routes {
            self.add_service_route(svc).await?;
        }
        Ok(())
    }

    async fn clear_all(&self) -> Result<(), Self::Error> {
        let mut batch = Batch::new();
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(table())));
        let nft = batch.to_nftables();
        let _ = helper::apply_ruleset_async(&nft).await;
        self.delete_managed_routes().await
    }
}

impl NftablesDataPlane {
    async fn delete_managed_routes(&self) -> Result<(), DataPlaneError> {
        let query = RouteMessageBuilder::<Ipv6Addr>::new().build();
        let mut stream = self.route_handle.route().get(query).execute();

        let mut to_delete = Vec::new();
        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e: rtnetlink::Error| DataPlaneError::Netlink {
                source: Box::new(e),
            })?;
            if msg.header.protocol != RouteProtocol::Static
                || msg.header.destination_prefix_length != 128
            {
                continue;
            }
            let in_range = msg.attributes.iter().any(|attr| {
                if let RouteAttribute::Destination(RouteAddress::Inet6(a)) = attr {
                    let b = a.octets();
                    b[0] == 0xfd && b[1] == 0x5e
                } else {
                    false
                }
            });
            if in_range {
                to_delete.push(msg);
            }
        }

        for route in to_delete {
            self.route_handle
                .route()
                .del(route)
                .execute()
                .await
                .map_err(|e| DataPlaneError::Netlink {
                    source: Box::new(e),
                })?;
        }

        Ok(())
    }

    async fn add_service_route(&self, svc: &ServiceRoute) -> Result<(), DataPlaneError> {
        let route = match svc.backends.len() {
            0 => RouteMessageBuilder::<Ipv6Addr>::new()
                .destination_prefix(svc.service_ip, 128)
                .kind(RouteType::BlackHole)
                .build(),
            1 => RouteMessageBuilder::<Ipv6Addr>::new()
                .destination_prefix(svc.service_ip, 128)
                .gateway(svc.backends[0])
                .build(),
            _ => {
                let nexthops = svc
                    .backends
                    .iter()
                    .map(|&b| {
                        RouteNextHopBuilder::new_ipv6()
                            .via(IpAddr::V6(b))
                            .unwrap()
                            .build()
                    })
                    .collect();
                RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .multipath(nexthops)
                    .build()
            }
        };
        self.route_handle
            .route()
            .add(route)
            .replace()
            .execute()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: Box::new(e),
            })
    }
}

fn table() -> Table<'static> {
    Table {
        family: NfFamily::INet,
        name: Cow::Borrowed(TABLE),
        handle: None,
    }
}

fn nft_table() -> NfListObject<'static> {
    NfListObject::Table(table())
}

fn prerouting_chain() -> NfListObject<'static> {
    NfListObject::Chain(Chain {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(CHAIN_PRE),
        newname: None,
        handle: None,
        _type: Some(NfChainType::NAT),
        hook: Some(NfHook::Prerouting),
        prio: Some(PRIO_DSTNAT),
        dev: None,
        policy: Some(NfChainPolicy::Accept),
    })
}

fn forward_chain() -> NfListObject<'static> {
    NfListObject::Chain(Chain {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(CHAIN_FWD),
        newname: None,
        handle: None,
        _type: Some(NfChainType::Filter),
        hook: Some(NfHook::Forward),
        prio: Some(PRIO_FILTER),
        dev: None,
        policy: Some(NfChainPolicy::Accept),
    })
}

fn rule_obj(chain: &'static str, stmts: Vec<Statement<'static>>) -> NfListObject<'static> {
    NfListObject::Rule(Rule {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        chain: Cow::Borrowed(chain),
        expr: Cow::Owned(stmts),
        handle: None,
        index: None,
        comment: None,
    })
}

fn payload_expr(protocol: &'static str, field: &'static str) -> Expression<'static> {
    Expression::Named(NamedExpression::Payload(Payload::PayloadField(
        PayloadField {
            protocol: Cow::Borrowed(protocol),
            field: Cow::Borrowed(field),
        },
    )))
}

fn prefix_expr(addr: String, len: u8) -> Expression<'static> {
    Expression::Named(NamedExpression::Prefix(Prefix {
        addr: Box::new(Expression::String(Cow::Owned(addr))),
        len: len as u32,
    }))
}

fn match_eq(left: Expression<'static>, right: Expression<'static>) -> Statement<'static> {
    Statement::Match(Match {
        left,
        right,
        op: Operator::EQ,
    })
}

/// Produces `meta nfproto <num>` — used to restrict inet-table rules to a
/// single address family without splitting the table by family.
fn match_nfproto(proto_num: u32) -> Statement<'static> {
    match_eq(
        Expression::Named(NamedExpression::Meta(Meta {
            key: MetaKey::Nfproto,
        })),
        Expression::Number(proto_num),
    )
}

fn dnat_ip6(addr: String, port: u16) -> Statement<'static> {
    Statement::DNAT(Some(NAT {
        addr: Some(Expression::String(Cow::Owned(addr))),
        family: Some(NATFamily::IP6),
        port: Some(Expression::Number(port as u32)),
        flags: None,
    }))
}

fn ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    let caddy_ip = match rule.caddy_addr.ip() {
        IpAddr::V6(ip) => ip.to_string(),
        IpAddr::V4(ip) => format!("::ffff:{ip}"),
    };
    let caddy_port = rule.caddy_addr.port();
    let ext_port = rule.external_port as u32;

    // Guard with `meta nfproto ipv6` so that in the inet (dual-stack) table
    // the DNAT statement only evaluates for IPv6 packets. IPv4 packets
    // matching the same dport would otherwise reach `dnat ip6 to`, which
    // nftables silently skips for IPv4 — leaving IPv4 callers unserved with
    // no clear signal. The explicit guard makes the IPv6-only behaviour
    // intentional and visible in `nft list ruleset` output.
    let make = |proto: &'static str| {
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_eq(payload_expr(proto, "dport"), Expression::Number(ext_port)),
            dnat_ip6(caddy_ip.clone(), caddy_port),
        ]
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

fn mount_rule_stmts(rule: &MountRule) -> Vec<Vec<Statement<'static>>> {
    let pod_addr = rule.pod_prefix.network().to_string();
    let pod_len = rule.pod_prefix.prefix_len();
    let mount_addr = rule.mount_addr.to_string();
    let svc_ip = rule.service_ip.to_string();
    let mount_port = rule.mount_port as u32;
    let svc_port = rule.service_port;

    let make = |proto: &'static str| {
        vec![
            match_eq(
                payload_expr("ip6", "saddr"),
                prefix_expr(pod_addr.clone(), pod_len),
            ),
            match_eq(
                payload_expr("ip6", "daddr"),
                Expression::String(Cow::Owned(mount_addr.clone())),
            ),
            match_eq(payload_expr(proto, "dport"), Expression::Number(mount_port)),
            dnat_ip6(svc_ip.clone(), svc_port),
        ]
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

fn seedling_forward_stmts() -> Vec<Statement<'static>> {
    let pfx = prefix_expr("fd5e:ed::".to_owned(), 24);
    vec![
        match_eq(payload_expr("ip6", "saddr"), pfx.clone()),
        match_eq(payload_expr("ip6", "daddr"), pfx),
        Statement::Accept(None),
    ]
}
