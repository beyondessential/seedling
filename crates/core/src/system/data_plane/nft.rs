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
    schema::{Chain, NfListObject, Rule, Table},
    stmt::{Match, NAT, NATFamily, Operator, Statement},
    types::{NfChainPolicy, NfChainType, NfFamily, NfHook},
};

use crate::system::types::{ForwardProto, IngressRule, MountRule, ServiceDnatRule};

pub(super) const TABLE: &str = "seedling_net";
pub(super) const CHAIN_PRE: &str = "prerouting";
pub(super) const CHAIN_OUT: &str = "output";
pub(super) const CHAIN_POST: &str = "postrouting";
pub(super) const CHAIN_FWD: &str = "forward";
pub(super) const PRIO_DSTNAT: i32 = -100;
pub(super) const PRIO_SRCNAT: i32 = 100;
pub(super) const PRIO_FILTER: i32 = 0;

/// Netfilter protocol number for IPv6 (`NFPROTO_IPV6`).
/// Used to guard inet-table ingress rules so they only match IPv6 packets.
pub(super) const NFPROTO_IPV6: u32 = 10;
pub(super) const NFPROTO_IPV4: u32 = 2;

pub(super) fn table() -> Table<'static> {
    Table {
        family: NfFamily::INet,
        name: Cow::Borrowed(TABLE),
        handle: None,
    }
}

pub(super) fn nft_table() -> NfListObject<'static> {
    NfListObject::Table(table())
}

pub(super) fn prerouting_chain() -> NfListObject<'static> {
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

pub(super) fn forward_chain() -> NfListObject<'static> {
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

pub(super) fn output_chain() -> NfListObject<'static> {
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

pub(super) fn postrouting_chain() -> NfListObject<'static> {
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

pub(super) fn rule_obj(
    chain: &'static str,
    stmts: Vec<Statement<'static>>,
) -> NfListObject<'static> {
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
    match_eq(
        Expression::Named(NamedExpression::Meta(Meta {
            key: MetaKey::Nfproto,
        })),
        Expression::Number(proto_num),
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
/// Identical DNAT targets, restricted to locally-destined packets
/// (`fib daddr type local`) so that host processes connecting to ingress
/// ports on `::1`, `127.0.0.1`, or the host's own addresses are redirected.
pub(super) fn output_ingress_rule_stmts(rule: &IngressRule) -> Vec<Vec<Statement<'static>>> {
    ingress_rule_stmts(rule)
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

// r[impl infra.dataplane.service-dnat]
pub(super) fn service_dnat_rule_stmts(rule: &ServiceDnatRule) -> Vec<Vec<Statement<'static>>> {
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
/// backends: round-robin via `numgen inc mod N` mapping to `addr . port`
/// concatenations so each backend can have its own pod-side port.
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
pub(super) fn loopback_masquerade_stmts() -> Vec<Vec<Statement<'static>>> {
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

// r[impl infra.dataplane.forward-policy]
pub(super) fn ct_state_established_related_accept() -> Vec<Statement<'static>> {
    let ct_expr = Expression::Named(NamedExpression::CT(CT {
        key: Cow::Borrowed("state"),
        family: None,
        dir: None,
    }));
    let states = Expression::List(vec![
        Expression::String(Cow::Borrowed("established")),
        Expression::String(Cow::Borrowed("related")),
    ]);
    vec![
        Statement::Match(Match {
            left: ct_expr,
            right: states,
            op: Operator::IN,
        }),
        Statement::Accept(None),
    ]
}

// r[impl infra.dataplane.forward-policy]
pub(super) fn ct_status_dnat_accept() -> Vec<Statement<'static>> {
    let ct_expr = Expression::Named(NamedExpression::CT(CT {
        key: Cow::Borrowed("status"),
        family: None,
        dir: None,
    }));
    vec![
        Statement::Match(Match {
            left: Expression::BinaryOperation(Box::new(BinaryOperation::AND(
                ct_expr,
                Expression::String(Cow::Borrowed("dnat")),
            ))),
            right: Expression::String(Cow::Borrowed("dnat")),
            op: Operator::EQ,
        }),
        Statement::Accept(None),
    ]
}

// r[impl infra.dataplane.forward-policy]
pub(super) fn seedling_forward_stmts(node_prefix: &Ipv6Net) -> Vec<Statement<'static>> {
    let pfx = prefix_expr(node_prefix.network().to_string(), node_prefix.prefix_len());
    vec![
        match_eq(payload_expr("ip6", "saddr"), pfx.clone()),
        match_eq(payload_expr("ip6", "daddr"), pfx),
        Statement::Accept(None),
    ]
}

// r[impl infra.dataplane.forward-policy]
pub(super) fn drop_unsolicited_inbound_stmts(node_prefix: &Ipv6Net) -> Vec<Statement<'static>> {
    let pfx = prefix_expr(node_prefix.network().to_string(), node_prefix.prefix_len());
    vec![
        match_eq(payload_expr("ip6", "daddr"), pfx),
        Statement::Drop(None),
    ]
}
