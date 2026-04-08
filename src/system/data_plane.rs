use std::{
    borrow::Cow,
    net::{IpAddr, Ipv6Addr},
};

use futures_util::StreamExt;
use nftables::{
    batch::Batch,
    expr::{
        Expression, Fib, FibFlag, FibResult, Meta, MetaKey, NamedExpression, Payload, PayloadField,
        Prefix,
    },
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
use tracing::{error, warn};

use crate::system::{
    BoxError, BoxFuture, DataPlane,
    types::{DataPlaneRules, ForwardProto, IngressRule, MountRule, ServiceRoute},
};

const TABLE: &str = "seedling_net";
const CHAIN_PRE: &str = "prerouting";
const CHAIN_OUT: &str = "output";
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

impl NftablesDataPlane {
    // r[impl infra.dataplane.output-nat]
    async fn apply_rules_impl(&self, rules: &DataPlaneRules) -> Result<(), DataPlaneError> {
        let mut batch = Batch::new();
        batch.add(nft_table());
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(table())));
        batch.add(prerouting_chain());
        batch.add(output_chain());
        batch.add(forward_chain());

        for rule in &rules.ingress {
            for stmts in ingress_rule_stmts(rule) {
                batch.add(rule_obj(CHAIN_PRE, stmts));
            }
            for stmts in output_ingress_rule_stmts(rule) {
                batch.add(rule_obj(CHAIN_OUT, stmts));
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

    async fn apply_routes_impl(&self, routes: &[ServiceRoute]) -> Result<(), DataPlaneError> {
        self.delete_managed_routes().await.map_err(|e| {
            error!(error = %e, "data_plane: delete_managed_routes failed");
            e
        })?;
        for svc in routes {
            self.add_service_route(svc).await.map_err(|e| {
                error!(
                    error = %e,
                    service_ip = %svc.service_ip,
                    backends = svc.backends.len(),
                    "data_plane: add_service_route failed"
                );
                e
            })?;
        }
        Ok(())
    }

    async fn clear_all_impl(&self) -> Result<(), DataPlaneError> {
        let mut batch = Batch::new();
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(table())));
        let nft = batch.to_nftables();
        let _ = helper::apply_ruleset_async(&nft).await;
        self.delete_managed_routes().await
    }
}

impl DataPlane for NftablesDataPlane {
    fn apply_rules<'a>(&'a self, rules: &'a DataPlaneRules) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_rules_impl(rules).await.map_err(Into::into) })
    }

    fn apply_routes<'a>(
        &'a self,
        routes: &'a [ServiceRoute],
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_routes_impl(routes).await.map_err(Into::into) })
    }

    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.clear_all_impl().await.map_err(Into::into) })
    }
}

impl NftablesDataPlane {
    #[tracing::instrument(level = "trace", skip(self))]
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
            // Reconstruct a minimal delete message from the destination only,
            // rather than echoing the full kernel-returned route message. The
            // kernel (5.2+) may include RTA_NH_ID and other NLAs that
            // netlink-packet-route stores as Other(DefaultNla); serialising
            // those back verbatim in RTM_DELROUTE causes EINVAL on kernel 6.x.
            let dst = route.attributes.iter().find_map(|attr| {
                if let RouteAttribute::Destination(RouteAddress::Inet6(a)) = attr {
                    Some(*a)
                } else {
                    None
                }
            });
            let Some(dst) = dst else {
                warn!("managed route has no IPv6 destination attribute; skipping deletion");
                continue;
            };
            let del_route = RouteMessageBuilder::<Ipv6Addr>::new()
                .destination_prefix(dst, route.header.destination_prefix_length)
                .table_id(254)
                .build();
            self.route_handle
                .route()
                .del(del_route)
                .execute()
                .await
                .map_err(|e| DataPlaneError::Netlink {
                    source: Box::new(e),
                })?;
        }

        Ok(())
    }

    /// Look up the output interface index for a given IPv6 address by querying
    /// the kernel's routing table. Kernel 6.x requires an explicit `RTA_OIF`
    /// in `RTM_NEWROUTE` for non-link-local gateways; without it the kernel
    /// returns EINVAL instead of resolving the interface automatically.
    async fn resolve_oif(&self, addr: Ipv6Addr) -> Option<u32> {
        let query = RouteMessageBuilder::<Ipv6Addr>::new()
            .destination_prefix(addr, 128)
            .build();
        let mut stream = self.route_handle.route().get(query).execute();
        match stream.next().await {
            Some(Ok(msg)) => msg.attributes.iter().find_map(|attr| {
                if let RouteAttribute::Oif(idx) = attr {
                    Some(*idx)
                } else {
                    None
                }
            }),
            _ => None,
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn add_service_route(&self, svc: &ServiceRoute) -> Result<(), DataPlaneError> {
        let route = match svc.backends.len() {
            // Explicit table_id(254) adds RTA_TABLE as an NLA attribute in
            // addition to the header field; kernel 6.x validates this more
            // strictly and may return EINVAL when the attribute is absent.
            0 => RouteMessageBuilder::<Ipv6Addr>::new()
                .destination_prefix(svc.service_ip, 128)
                .kind(RouteType::BlackHole)
                .table_id(254)
                .build(),
            1 => {
                // Resolve the output interface from the kernel routing table.
                // Kernel 6.x no longer auto-resolves the interface for
                // global-scope (non-link-local) IPv6 gateways and returns
                // EINVAL when RTA_OIF is absent.
                let oif = self.resolve_oif(svc.backends[0]).await;
                let mut builder = RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .gateway(svc.backends[0])
                    .table_id(254);
                if let Some(idx) = oif {
                    builder = builder.output_interface(idx);
                }
                builder.build()
            }
            _ => {
                let mut nexthops = Vec::with_capacity(svc.backends.len());
                for &b in &svc.backends {
                    let oif = self.resolve_oif(b).await;
                    let mut nh = RouteNextHopBuilder::new_ipv6().via(IpAddr::V6(b)).unwrap();
                    if let Some(idx) = oif {
                        nh = nh.interface(idx);
                    }
                    nexthops.push(nh.build());
                }
                RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .multipath(nexthops)
                    .table_id(254)
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

fn output_chain() -> NfListObject<'static> {
    NfListObject::Chain(Chain {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(CHAIN_OUT),
        newname: None,
        handle: None,
        _type: Some(NfChainType::NAT),
        hook: Some(NfHook::Output),
        prio: Some(PRIO_DSTNAT),
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

/// Produces `fib daddr type == "local"` — matches packets whose destination
/// is a local address (loopback `::1`, or any of the host's own addresses).
/// Used in the `output` chain to catch host-originated traffic aimed at
/// ingress ports on any locally-bound address.
fn match_fib_daddr_local() -> Statement<'static> {
    use std::collections::HashSet;
    let mut flags = HashSet::new();
    flags.insert(FibFlag::Daddr);
    match_eq(
        Expression::Named(NamedExpression::Fib(Fib {
            result: FibResult::Type,
            flags,
        })),
        Expression::String(Cow::Borrowed("local")),
    )
}

/// Output-chain counterpart to [`ingress_rule_stmts`].
///
/// Identical DNAT target, but restricted to locally-destined packets
/// (`fib daddr type local`) so that host processes connecting to ingress
/// ports on `::1` or the host's own addresses are redirected to Caddy.
fn output_ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    let caddy_ip = match rule.caddy_addr.ip() {
        IpAddr::V6(ip) => ip.to_string(),
        IpAddr::V4(ip) => format!("::ffff:{ip}"),
    };
    let caddy_port = rule.caddy_addr.port();
    let ext_port = rule.external_port as u32;

    let make = |proto: &'static str| {
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_fib_daddr_local(),
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

#[cfg(test)]
mod tests {
    use std::net::{Ipv6Addr, SocketAddr};

    use crate::system::types::{ForwardProto, IngressRule};

    use super::{ingress_rule_stmts, output_ingress_rule_stmts};

    fn test_rule(port: u16, proto: ForwardProto) -> IngressRule {
        IngressRule {
            external_port: port,
            proto,
            caddy_addr: SocketAddr::new(
                std::net::IpAddr::V6(Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1)),
                8080,
            ),
        }
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_rules_have_fib_daddr_local_guard() {
        let rule = test_rule(80, ForwardProto::Tcp);
        let stmts_list = output_ingress_rule_stmts(&rule);
        assert_eq!(stmts_list.len(), 1, "tcp produces one rule vec");
        let stmts = &stmts_list[0];

        // The second statement must be a fib match (after the nfproto guard).
        let fib_stmt = &stmts[1];
        let json = serde_json::to_string(fib_stmt).expect("serialize");
        assert!(
            json.contains("\"fib\""),
            "second statement should be a fib expression, got: {json}"
        );
        assert!(
            json.contains("\"daddr\""),
            "fib should use daddr flag, got: {json}"
        );
        assert!(
            json.contains("local"),
            "fib match should compare against \"local\", got: {json}"
        );
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_rules_dnat_to_caddy() {
        let rule = test_rule(443, ForwardProto::Tcp);
        let stmts_list = output_ingress_rule_stmts(&rule);
        let stmts = &stmts_list[0];

        let dnat_stmt = stmts.last().expect("at least one statement");
        let json = serde_json::to_string(dnat_stmt).expect("serialize");
        assert!(json.contains("\"dnat\""), "last statement should be dnat");
        assert!(json.contains("8080"), "dnat should target caddy port 8080");
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_rules_both_proto_produces_two_vecs() {
        let rule = test_rule(80, ForwardProto::Both);
        let stmts_list = output_ingress_rule_stmts(&rule);
        assert_eq!(
            stmts_list.len(),
            2,
            "Both produces two rule vecs (tcp + udp)"
        );
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_and_prerouting_rules_have_same_dnat_target() {
        let rule = test_rule(80, ForwardProto::Tcp);
        let pre_stmts = &ingress_rule_stmts(&rule)[0];
        let out_stmts = &output_ingress_rule_stmts(&rule)[0];

        let pre_dnat = serde_json::to_string(pre_stmts.last().unwrap()).unwrap();
        let out_dnat = serde_json::to_string(out_stmts.last().unwrap()).unwrap();
        assert_eq!(
            pre_dnat, out_dnat,
            "prerouting and output rules must DNAT to the same target"
        );
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_rules_have_nfproto_ipv6_guard() {
        let rule = test_rule(80, ForwardProto::Tcp);
        let stmts = &output_ingress_rule_stmts(&rule)[0];

        let first = serde_json::to_string(&stmts[0]).unwrap();
        assert!(
            first.contains("nfproto"),
            "first statement must be the nfproto guard, got: {first}"
        );
        assert!(
            first.contains("10"),
            "nfproto guard must match IPv6 (10), got: {first}"
        );
    }

    // r[verify infra.dataplane.output-nat]
    #[test]
    fn output_rules_match_dport() {
        let rule = test_rule(8443, ForwardProto::Tcp);
        let stmts = &output_ingress_rule_stmts(&rule)[0];

        let json: String = stmts
            .iter()
            .map(|s| serde_json::to_string(s).unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            json.contains("8443"),
            "output rules must match on the external port (8443), got: {json}"
        );
    }
}
