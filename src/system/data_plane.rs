use std::{
    borrow::Cow,
    net::{IpAddr, Ipv6Addr},
};

use nftables::{
    batch::Batch,
    expr::{
        Expression, Fib, FibFlag, FibResult, Map, Meta, MetaKey, NamedExpression, NgMode, Numgen,
        Payload, PayloadField, Prefix, SetItem,
    },
    helper,
    schema::{Chain, FlushObject, NfCmd, NfListObject, Rule, Table},
    stmt::{Match, NAT, NATFamily, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};
use snafu::Snafu;
use tracing::error;

use crate::system::{
    BoxError, BoxFuture, DataPlane,
    types::{DataPlaneRules, ForwardProto, IngressRule, MountRule, ServiceDnatRule, ServiceRoute},
};

const TABLE: &str = "seedling_net";
const CHAIN_PRE: &str = "prerouting";
const CHAIN_OUT: &str = "output";
const CHAIN_POST: &str = "postrouting";
const CHAIN_FWD: &str = "forward";
const PRIO_DSTNAT: i32 = -100;
const PRIO_SRCNAT: i32 = 100;
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

pub(crate) struct NftablesDataPlane {}

impl NftablesDataPlane {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self {})
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
        batch.add(postrouting_chain());
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

        for rule in &rules.service_dnat {
            for stmts in service_dnat_rule_stmts(rule) {
                batch.add(rule_obj(CHAIN_PRE, stmts));
            }
        }

        for stmts in loopback_masquerade_stmts() {
            batch.add(rule_obj(CHAIN_POST, stmts));
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
    /// Delete all seedling-managed IPv6 static /128 routes in the fd5e::/16
    /// range using the `ip` CLI. Using the CLI instead of rtnetlink directly
    /// allows us to isolate whether EINVAL is a library/message-construction
    /// issue or genuine kernel behaviour.
    #[tracing::instrument(level = "trace", skip(self))]
    async fn delete_managed_routes(&self) -> Result<(), DataPlaneError> {
        // `ip -j route show` emits JSON; each element has a "dst" key.
        // For IPv6 host (/128) routes, iproute2 may omit the prefix length.
        let out = tokio::process::Command::new("ip")
            .args([
                "-6", "-j", "route", "show", "proto", "static", "table", "main",
            ])
            .output()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: Box::new(e),
            })?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DataPlaneError::Netlink {
                source: format!("ip route show failed: {}", stderr.trim()).into(),
            });
        }

        let routes: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).unwrap_or_default();

        for route in routes {
            let dst = match route["dst"].as_str() {
                Some(d) => d,
                None => continue,
            };

            // Only touch /128 routes in our managed range.
            // Host routes may appear without the "/128" suffix.
            let addr_part = dst.split('/').next().unwrap_or(dst);
            let prefix_len = dst
                .split('/')
                .nth(1)
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(128); // no slash → host route → /128

            if prefix_len != 128 {
                continue;
            }
            if !addr_part.starts_with("fd5e") {
                continue;
            }

            let del = tokio::process::Command::new("ip")
                .args([
                    "-6", "route", "del", dst, "proto", "static", "table", "main",
                ])
                .output()
                .await
                .map_err(|e| DataPlaneError::Netlink {
                    source: Box::new(e),
                })?;

            if !del.status.success() {
                let stderr = String::from_utf8_lossy(&del.stderr);
                return Err(DataPlaneError::Netlink {
                    source: format!(
                        "ip route del {} failed (exit {:?}): {}",
                        dst,
                        del.status.code(),
                        stderr.trim()
                    )
                    .into(),
                });
            }
        }

        Ok(())
    }

    /// Add or replace a service route using the `ip` CLI.
    #[tracing::instrument(level = "trace", skip(self))]
    async fn add_service_route(&self, svc: &ServiceRoute) -> Result<(), DataPlaneError> {
        let dst = format!("{}/128", svc.service_ip);

        // Build the argument list for: ip -6 route replace <args>
        let mut args: Vec<String> = vec!["route".into(), "replace".into()];

        match svc.backends.len() {
            0 => {
                args.push("blackhole".into());
                args.push(dst);
            }
            1 => {
                args.push(dst);
                args.extend(["via".into(), svc.backends[0].to_string()]);
            }
            _ => {
                args.push(dst);
                for b in &svc.backends {
                    args.extend(["nexthop".into(), "via".into(), b.to_string()]);
                }
            }
        }

        args.extend([
            "proto".into(),
            "static".into(),
            "table".into(),
            "main".into(),
        ]);

        let out = tokio::process::Command::new("ip")
            .arg("-6")
            .args(&args)
            .output()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: Box::new(e),
            })?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(DataPlaneError::Netlink {
                source: format!(
                    "ip -6 {} failed (exit {:?}): {}",
                    args.join(" "),
                    out.status.code(),
                    stderr.trim()
                )
                .into(),
            });
        }

        Ok(())
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

fn postrouting_chain() -> NfListObject<'static> {
    NfListObject::Chain(Chain {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(CHAIN_POST),
        newname: None,
        handle: None,
        _type: Some(NfChainType::NAT),
        hook: Some(NfHook::Postrouting),
        prio: Some(PRIO_SRCNAT),
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

fn dnat_ip4(addr: String, port: u16) -> Statement<'static> {
    Statement::DNAT(Some(NAT {
        addr: Some(Expression::String(Cow::Owned(addr))),
        family: Some(NATFamily::IP),
        port: Some(Expression::Number(port as u32)),
        flags: None,
    }))
}

const NFPROTO_IPV4: u32 = 2;

fn ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    let ext_port = rule.external_port as u32;

    let make_v6 = |proto: &'static str, dnat: Statement<'static>| {
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_fib_daddr_local(),
            match_eq(payload_expr(proto, "dport"), Expression::Number(ext_port)),
            dnat,
        ]
    };

    let make_v4 = |proto: &'static str, dnat: Statement<'static>| {
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_fib_daddr_local(),
            match_eq(payload_expr(proto, "dport"), Expression::Number(ext_port)),
            dnat,
        ]
    };

    let ip6 = match rule.caddy_v6.ip() {
        IpAddr::V6(ip) => ip.to_string(),
        IpAddr::V4(ip) => format!("::ffff:{ip}"),
    };
    let dnat6 = dnat_ip6(ip6, rule.caddy_v6.port());

    let mut rules = Vec::new();
    match rule.proto {
        ForwardProto::Tcp => rules.push(make_v6("tcp", dnat6)),
        ForwardProto::Udp => rules.push(make_v6("udp", dnat6)),
        ForwardProto::Both => {
            rules.push(make_v6("tcp", dnat6.clone()));
            rules.push(make_v6("udp", dnat6));
        }
    }

    if let Some(v4) = &rule.caddy_v4 {
        let ip4 = match v4.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => ip.to_string(),
        };
        let dnat4 = dnat_ip4(ip4, v4.port());
        match rule.proto {
            ForwardProto::Tcp => rules.push(make_v4("tcp", dnat4)),
            ForwardProto::Udp => rules.push(make_v4("udp", dnat4)),
            ForwardProto::Both => {
                rules.push(make_v4("tcp", dnat4.clone()));
                rules.push(make_v4("udp", dnat4));
            }
        }
    }

    rules
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
/// Identical DNAT targets, restricted to locally-destined packets
/// (`fib daddr type local`) so that host processes connecting to ingress
/// ports on `::1`, `127.0.0.1`, or the host's own addresses are redirected.
fn output_ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    // Identical to ingress_rule_stmts — both chains need the same rules.
    ingress_rule_stmts(rule)
}

fn mount_rule_stmts(rule: &MountRule) -> Vec<Vec<Statement<'static>>> {
    if rule.backends.is_empty() {
        return vec![];
    }

    let pod_addr = rule.pod_prefix.network().to_string();
    let pod_len = rule.pod_prefix.prefix_len();
    let mount_addr = rule.mount_addr.to_string();
    let mount_port = rule.mount_port as u32;

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
            dnat_lb(&rule.backends),
        ]
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

// r[impl infra.dataplane.service-dnat]
fn service_dnat_rule_stmts(rule: &ServiceDnatRule) -> Vec<Vec<Statement<'static>>> {
    if rule.backends.is_empty() {
        return vec![];
    }

    let svc_ip = rule.service_ip.to_string();
    let svc_port = rule.service_port as u32;

    let make = |proto: &'static str| {
        let mut stmts = vec![
            match_nfproto(NFPROTO_IPV6),
            match_eq(
                payload_expr("ip6", "daddr"),
                Expression::String(Cow::Owned(svc_ip.clone())),
            ),
            match_eq(payload_expr(proto, "dport"), Expression::Number(svc_port)),
        ];
        stmts.push(dnat_lb(&rule.backends));
        stmts
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

/// DNAT to one or more backends. Single backend: plain DNAT. Multiple
/// backends: round-robin via `numgen inc mod N` mapping to addresses.
fn dnat_lb(backends: &[(Ipv6Addr, u16)]) -> Statement<'static> {
    assert!(!backends.is_empty(), "dnat_lb called with no backends");

    if backends.len() == 1 {
        let (ip, port) = &backends[0];
        return dnat_ip6(ip.to_string(), *port);
    }

    let port = backends[0].1;

    let numgen = Expression::Named(NamedExpression::Numgen(Numgen {
        mode: NgMode::Inc,
        ng_mod: backends.len() as u32,
        offset: None,
    }));

    let mapping_set: Vec<SetItem<'_>> = backends
        .iter()
        .enumerate()
        .map(|(i, (ip, _))| {
            SetItem::Mapping(
                Expression::Number(i as u32),
                Expression::String(Cow::Owned(ip.to_string())),
            )
        })
        .collect();

    let mapped_addr = Expression::Named(NamedExpression::Map(Box::new(Map {
        key: numgen,
        data: Expression::Named(NamedExpression::Set(mapping_set)),
    })));

    Statement::DNAT(Some(NAT {
        addr: Some(mapped_addr),
        family: Some(NATFamily::IP6),
        port: Some(Expression::Number(port as u32)),
        flags: None,
    }))
}

/// MASQUERADE rules for loopback-sourced traffic that was DNAT'd to a
/// container. Without this, a packet arriving at Caddy with src=127.0.0.1
/// causes Caddy to respond to its own loopback — the reply never leaves
/// the container. MASQUERADE rewrites the source to the bridge gateway IP
/// so the response comes back through the bridge and conntrack reverses it.
fn loopback_masquerade_stmts() -> Vec<Vec<Statement<'static>>> {
    vec![
        // IPv4: src 127.0.0.0/8 → masquerade
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_eq(
                payload_expr("ip", "saddr"),
                prefix_expr("127.0.0.0".to_owned(), 8),
            ),
            Statement::Masquerade(None),
        ],
        // IPv6: src ::1/128 → masquerade
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_eq(
                payload_expr("ip6", "saddr"),
                Expression::String(Cow::Borrowed("::1")),
            ),
            Statement::Masquerade(None),
        ],
    ]
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
            caddy_v6: SocketAddr::new(
                std::net::IpAddr::V6(Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1)),
                8080,
            ),
            caddy_v4: None,
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
