use std::{
    borrow::Cow,
    collections::HashSet,
    net::{IpAddr, Ipv6Addr},
};

use ipnet::Ipv6Net;

use nftables::{
    expr::{
        BinaryOperation, CT, Expression, Fib, FibFlag, FibResult, Map, Meta, MetaKey,
        NamedExpression, NgMode, Numgen, Payload, PayloadField, Prefix, SetItem,
    },
    schema::{
        Chain, Element, FlushObject, NfCmd, NfListObject, Rule, Set, SetFlag, SetType,
        SetTypeValue, Table,
    },
    stmt::{Match, NAT, NATFamily, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};

use seedling_core::system::{
    POD_BRIDGE_PREFIX, PROXY_BRIDGE,
    types::{ForwardProto, IngressRule, MountRule, ServiceDnatRule},
};

pub(super) const TABLE: &str = "seedling_net";
pub(super) const CHAIN_PRE: &str = "prerouting";
pub(super) const CHAIN_OUT: &str = "output";
pub(super) const CHAIN_POST: &str = "postrouting";
/// The host's IPv4 addresses, and the proxy and resolver IPv4 subnets, in
/// `64:ff9b::` form.
pub(super) const SET_HOST_V4_NAT64: &str = "host_v4_nat64";
pub(super) const PRIO_DSTNAT: i32 = -100;
pub(super) const PRIO_SRCNAT: i32 = 100;

/// Netfilter protocol number for IPv6 (`NFPROTO_IPV6`).
/// Used to guard inet-table ingress rules so they only match IPv6 packets.
pub(super) const NFPROTO_IPV6: u32 = 10;
pub(super) const NFPROTO_IPV4: u32 = 2;

pub(super) fn table(name: &'static str) -> Table<'static> {
    Table {
        family: NfFamily::INet,
        name: Cow::Borrowed(name),
        handle: None,
    }
}

pub(super) fn base_chain(
    table: &'static str,
    name: &'static str,
    kind: NfChainType,
    hook: NfHook,
    prio: i32,
) -> NfListObject<'static> {
    NfListObject::Chain(Chain {
        family: NfFamily::INet,
        table: Cow::Borrowed(table),
        name: Cow::Borrowed(name),
        newname: None,
        handle: None,
        _type: Some(kind),
        hook: Some(hook),
        prio: Some(prio),
        dev: None,
        policy: Some(NfChainPolicy::Accept),
    })
}

pub(super) fn prerouting_chain() -> NfListObject<'static> {
    base_chain(
        TABLE,
        CHAIN_PRE,
        NfChainType::NAT,
        NfHook::Prerouting,
        PRIO_DSTNAT,
    )
}

pub(super) fn output_chain() -> NfListObject<'static> {
    base_chain(
        TABLE,
        CHAIN_OUT,
        NfChainType::NAT,
        NfHook::Output,
        PRIO_DSTNAT,
    )
}

pub(super) fn postrouting_chain() -> NfListObject<'static> {
    base_chain(
        TABLE,
        CHAIN_POST,
        NfChainType::NAT,
        NfHook::Postrouting,
        PRIO_SRCNAT,
    )
}

pub(super) fn rule_obj(
    table: &'static str,
    chain: &'static str,
    stmts: Vec<Statement<'static>>,
) -> NfListObject<'static> {
    NfListObject::Rule(Rule {
        family: NfFamily::INet,
        table: Cow::Borrowed(table),
        chain: Cow::Borrowed(chain),
        expr: Cow::Owned(stmts),
        handle: None,
        index: None,
        comment: None,
    })
}

pub(super) fn payload_expr(protocol: &'static str, field: &'static str) -> Expression<'static> {
    Expression::Named(NamedExpression::Payload(Payload::PayloadField(
        PayloadField {
            protocol: Cow::Borrowed(protocol),
            field: Cow::Borrowed(field),
        },
    )))
}

pub(super) fn prefix_expr(addr: String, len: u8) -> Expression<'static> {
    Expression::Named(NamedExpression::Prefix(Prefix {
        addr: Box::new(Expression::String(Cow::Owned(addr))),
        len: len as u32,
    }))
}

pub(super) fn match_eq(
    left: Expression<'static>,
    right: Expression<'static>,
) -> Statement<'static> {
    Statement::Match(Match {
        left,
        right,
        op: Operator::EQ,
    })
}

/// Produces `meta nfproto <num>` — used to restrict inet-table rules to a
/// single address family without splitting the table by family.
pub(super) fn match_nfproto(proto_num: u32) -> Statement<'static> {
    match_eq(meta(MetaKey::Nfproto), Expression::Number(proto_num))
}

pub(super) fn meta(key: MetaKey) -> Expression<'static> {
    Expression::Named(NamedExpression::Meta(Meta { key }))
}

pub(super) fn match_meta_eq(key: MetaKey, value: &'static str) -> Statement<'static> {
    match_eq(meta(key), Expression::String(Cow::Borrowed(value)))
}

/// `iifname "sdp-*"` or `oifname "sdp-*"`: any pod network's bridge.
pub(super) fn match_pod_bridge(key: MetaKey) -> Statement<'static> {
    match_eq(
        meta(key),
        Expression::String(Cow::Owned(format!("{POD_BRIDGE_PREFIX}*"))),
    )
}

pub(super) fn dnat_ip6(addr: String, port: u16) -> Statement<'static> {
    Statement::DNAT(Some(NAT {
        addr: Some(Expression::String(Cow::Owned(addr))),
        family: Some(NATFamily::IP6),
        port: Some(Expression::Number(port as u32)),
        flags: None,
    }))
}

pub(super) fn dnat_ip4(addr: String, port: u16) -> Statement<'static> {
    Statement::DNAT(Some(NAT {
        addr: Some(Expression::String(Cow::Owned(addr))),
        family: Some(NATFamily::IP),
        port: Some(Expression::Number(port as u32)),
        flags: None,
    }))
}

pub(super) fn ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    let ext_port = rule.external_port as u32;
    let mut rules = ingress_rule_stmts_v6(rule);

    let make_v4 = |proto: &'static str, dnat: Statement<'static>| {
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_fib_daddr_local(),
            match_eq(payload_expr(proto, "dport"), Expression::Number(ext_port)),
            dnat,
        ]
    };

    // spec: INFRA#ingress-redirection
    let dnat4 = dnat_ip4(rule.caddy_v4.ip().to_string(), rule.caddy_v4.port());
    match rule.proto {
        ForwardProto::Tcp => rules.push(make_v4("tcp", dnat4)),
        ForwardProto::Udp => rules.push(make_v4("udp", dnat4)),
        ForwardProto::Both => {
            rules.push(make_v4("tcp", dnat4.clone()));
            rules.push(make_v4("udp", dnat4));
        }
    }

    rules
}

fn ingress_rule_stmts_v6(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    ingress_redirects(rule, |dport| {
        vec![match_nfproto(NFPROTO_IPV6), match_fib_daddr_local(), dport]
    })
}

/// Pod traffic to an ingress port on one of the host's IPv4 addresses, or
/// within the proxy or resolver network, by its NAT64 form, redirected to the
/// proxy's IPv6 address. It is rewritten before NAT64 would translate it, so
/// it never reaches the host over IPv4.
pub(super) fn nat64_ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    // spec: INFRA#pod-traffic-to-the-host
    ingress_redirects(rule, |dport| {
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_eq(
                payload_expr("ip6", "daddr"),
                Expression::String(Cow::Owned(format!("@{SET_HOST_V4_NAT64}"))),
            ),
            dport,
        ]
    })
}

/// One rule per protocol of `rule`, each the statements `guard` makes from
/// the ingress port's match, followed by the redirection to the proxy's IPv6
/// address.
fn ingress_redirects(
    rule: &IngressRule,
    guard: impl Fn(Statement<'static>) -> Vec<Statement<'static>>,
) -> Vec<Vec<Statement<'static>>> {
    let ext_port = rule.external_port as u32;
    let ip6 = match rule.caddy_v6.ip() {
        IpAddr::V6(ip) => ip.to_string(),
        IpAddr::V4(ip) => format!("::ffff:{ip}"),
    };
    let dnat6 = dnat_ip6(ip6, rule.caddy_v6.port());

    let make = |proto: &'static str| {
        let mut stmts = guard(match_eq(
            payload_expr(proto, "dport"),
            Expression::Number(ext_port),
        ));
        stmts.push(dnat6.clone());
        stmts
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

fn host_v4_nat64_set() -> Set<'static> {
    Set {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(SET_HOST_V4_NAT64),
        handle: None,
        set_type: SetTypeValue::Single(SetType::Ipv6Addr),
        policy: None,
        flags: Some([SetFlag::Interval].into_iter().collect()),
        elem: None,
        timeout: None,
        gc_interval: None,
        size: None,
        comment: None,
    }
}

fn host_v4_nat64_elements(elements: &[Ipv6Net]) -> Element<'static> {
    Element {
        family: NfFamily::INet,
        table: Cow::Borrowed(TABLE),
        name: Cow::Borrowed(SET_HOST_V4_NAT64),
        elem: Cow::Owned(
            elements
                .iter()
                .map(|net| match net.prefix_len() {
                    128 => Expression::String(Cow::Owned(net.addr().to_string())),
                    len => prefix_expr(net.network().to_string(), len),
                })
                .collect(),
        ),
    }
}

/// The commands that leave the `host_v4_nat64` set holding exactly
/// `elements`, creating it if it does not exist. Flushing the table leaves
/// its sets' elements in place, so the set is flushed on its own.
pub(super) fn replace_host_v4_nat64(elements: &[Ipv6Net]) -> [NfCmd<'static>; 3] {
    [
        NfCmd::Add(NfListObject::Set(Box::new(host_v4_nat64_set()))),
        NfCmd::Flush(FlushObject::Set(Box::new(host_v4_nat64_set()))),
        NfCmd::Add(NfListObject::Element(host_v4_nat64_elements(elements))),
    ]
}

/// Produces `fib daddr type == "local"` — matches packets whose destination
/// is a local address (loopback `::1`, or any of the host's own addresses).
/// Used in the `output` chain to catch host-originated traffic aimed at
/// ingress ports on any locally-bound address.
pub(super) fn match_fib_daddr_local() -> Statement<'static> {
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
/// The same ports, protocols and IPv6 DNAT target, restricted to
/// locally-destined packets (`fib daddr type local`) so that host processes
/// connecting to ingress ports on `::1` or the host's own IPv6 addresses are
/// redirected. IPv4 traffic from the host is not redirected.
pub(super) fn output_ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    ingress_rule_stmts_v6(rule)
}

pub(super) fn mount_rule_stmts(rule: &MountRule) -> Vec<Vec<Statement<'static>>> {
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

/// Service translation for traffic arriving from the proxy network, for the
/// prerouting chain.
pub(super) fn service_dnat_prerouting_stmts(
    rule: &ServiceDnatRule,
) -> Vec<Vec<Statement<'static>>> {
    // spec: INFRA#service-translation
    // Only the proxy reaches a service by its address; a pod reaches it
    // through its mount endpoint.
    service_dnat_stmts(rule, &[match_meta_eq(MetaKey::Iifname, PROXY_BRIDGE)])
}

/// Service translation for traffic originating on the host, for the output
/// chain.
pub(super) fn service_dnat_output_stmts(rule: &ServiceDnatRule) -> Vec<Vec<Statement<'static>>> {
    // spec: INFRA#service-translation
    service_dnat_stmts(rule, &[])
}

/// One rule per protocol translating `rule`'s service address and port, each
/// starting with `guard`.
fn service_dnat_stmts(
    rule: &ServiceDnatRule,
    guard: &[Statement<'static>],
) -> Vec<Vec<Statement<'static>>> {
    if rule.backends.is_empty() {
        return vec![];
    }

    let svc_ip = rule.service_ip.to_string();
    let svc_port = rule.service_port as u32;

    let make = |proto: &'static str| {
        let mut stmts = guard.to_vec();
        stmts.extend([
            match_nfproto(NFPROTO_IPV6),
            match_eq(
                payload_expr("ip6", "daddr"),
                Expression::String(Cow::Owned(svc_ip.clone())),
            ),
            match_eq(payload_expr(proto, "dport"), Expression::Number(svc_port)),
            dnat_lb(&rule.backends),
        ]);
        stmts
    };

    match rule.proto {
        ForwardProto::Tcp => vec![make("tcp")],
        ForwardProto::Udp => vec![make("udp")],
        ForwardProto::Both => vec![make("tcp"), make("udp")],
    }
}

/// DNAT to one or more backends. Single backend: plain DNAT. Multiple
/// backends: round-robin via `numgen inc mod N` mapping to `addr . port`
/// concatenations so each backend can have its own pod-side port.
// spec: SVC#routing
pub(super) fn dnat_lb(backends: &[(Ipv6Addr, u16)]) -> Statement<'static> {
    assert!(!backends.is_empty(), "dnat_lb called with no backends");

    if backends.len() == 1 {
        let (ip, port) = &backends[0];
        return dnat_ip6(ip.to_string(), *port);
    }

    let numgen = Expression::Named(NamedExpression::Numgen(Numgen {
        mode: NgMode::Inc,
        ng_mod: backends.len() as u32,
        offset: None,
    }));

    let mapping_set: Vec<SetItem<'_>> = backends
        .iter()
        .enumerate()
        .map(|(i, (ip, port))| {
            SetItem::Mapping(
                Expression::Number(i as u32),
                Expression::Named(NamedExpression::Concat(vec![
                    Expression::String(Cow::Owned(ip.to_string())),
                    Expression::Number(*port as u32),
                ])),
            )
        })
        .collect();

    let mapped_target = Expression::Named(NamedExpression::Map(Box::new(Map {
        key: numgen,
        data: Expression::Named(NamedExpression::Set(mapping_set)),
    })));

    Statement::DNAT(Some(NAT {
        addr: Some(mapped_target),
        family: Some(NATFamily::IP6),
        port: None,
        flags: None,
    }))
}

/// MASQUERADE rules for loopback-sourced traffic that was DNAT'd to a
/// container. Without this, a packet arriving at Caddy with src=127.0.0.1
/// causes Caddy to respond to its own loopback — the reply never leaves
/// the container. MASQUERADE rewrites the source to the bridge gateway IP
/// so the response comes back through the bridge and conntrack reverses it.
///
/// Scoped to `ct status & dnat == dnat` so it only fires on connections
/// that actually hit a DNAT rule. Without this guard the masquerade also
/// caught plain loopback traffic (e.g. DNS queries to 127.0.0.53), whose
/// rewritten source then got dropped by systemd-resolved's BPF filter.
pub(super) fn loopback_masquerade_stmts() -> Vec<Vec<Statement<'static>>> {
    vec![
        // IPv4: src 127.0.0.0/8, connection was DNAT'd → masquerade
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_eq(
                payload_expr("ip", "saddr"),
                prefix_expr("127.0.0.0".to_owned(), 8),
            ),
            match_ct_status_dnat(),
            Statement::Masquerade(None),
        ],
        // IPv6: src ::1, connection was DNAT'd → masquerade
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_eq(
                payload_expr("ip6", "saddr"),
                Expression::String(Cow::Borrowed("::1")),
            ),
            match_ct_status_dnat(),
            Statement::Masquerade(None),
        ],
    ]
}

/// Produces `ct status & dnat == dnat` — matches packets belonging to
/// connections that had a DNAT rule applied.
pub(super) fn match_ct_status_dnat() -> Statement<'static> {
    let ct_expr = Expression::Named(NamedExpression::CT(CT {
        key: Cow::Borrowed("status"),
        family: None,
        dir: None,
    }));
    Statement::Match(Match {
        left: Expression::BinaryOperation(Box::new(BinaryOperation::AND(
            ct_expr,
            Expression::String(Cow::Borrowed("dnat")),
        ))),
        right: Expression::String(Cow::Borrowed("dnat")),
        op: Operator::EQ,
    })
}
