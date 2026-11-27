//! The isolation table: the packet rules that keep pods apart from one another
//! and from the host.
//!
//! Every rule here depends only on the host prefix, the fixed bridge names and
//! the NAT64 prefix, never on what any app contributes, so the table is built
//! from the host prefix alone. It is replaced wholesale in one batch, which
//! the kernel applies atomically, so the rules never lapse during a replace.

use std::borrow::Cow;

use ipnet::Ipv6Net;
use nftables::{
    batch::Batch,
    expr::{BinaryOperation, CT, Expression, Fib, FibFlag, FibResult, Meta, MetaKey, NamedExpression, SetItem},
    schema::{FlushObject, NfCmd, NfListObject},
    stmt::{Mangle, Match, Operator, Statement},
    types::{NfChainType, NfHook},
};

use seedling_core::{
    runtime::nat64_prefix::NAT64_PREFIX_NET,
    system::{POD_BRIDGE_PREFIX, PROXY_BRIDGE, RESOLVER_BRIDGE, resolver::resolver_addr},
};

use super::nft::{
    NFPROTO_IPV4, NFPROTO_IPV6, base_chain, match_ct_status_dnat, match_eq, match_nfproto,
    payload_expr, prefix_expr, rule_obj, table,
};

pub(super) const TABLE: &str = "seedling_isolation";
pub(super) const CHAIN_SOURCES: &str = "pod_sources";
pub(super) const CHAIN_MARKS: &str = "nat64_marks";
pub(super) const CHAIN_FORWARD: &str = "forwarding";
pub(super) const CHAIN_TO_HOST: &str = "to_host";
pub(super) const CHAIN_NAT64_DROPS: &str = "nat64_drops";

const PRIO_RAW: i32 = -300;
/// After the translation table's destination NAT (-100), so a mount
/// translation has already happened, and before Jool translates IPv6 to IPv4
/// (`dstnat + 25`).
const PRIO_MARKS: i32 = -90;
const PRIO_FILTER: i32 = 0;

/// Mark bit set on a pod's packets to the NAT64 prefix. Jool copies the mark
/// onto the IPv4 packet it translates to, so the bit still identifies a pod's
/// traffic after translation.
///
/// The bits sit above the `0xff0000` mask Tailscale routes by and the low 16
/// bits other host tools use, and are ORed in, leaving every other bit alone.
pub(super) const MARK_POD_NAT64: u32 = 0x0100_0000;
/// Mark bit set alongside [`MARK_POD_NAT64`] where a mount translation
/// rewrote the packet's destination.
pub(super) const MARK_MOUNT: u32 = 0x0200_0000;
/// Both isolation mark bits.
pub(super) const MARK_MASK: u32 = MARK_POD_NAT64 | MARK_MOUNT;

/// The batch that installs the isolation table, or replaces it with the same
/// rules.
pub(super) fn isolation_batch(host_prefix: &Ipv6Net) -> Batch<'static> {
    let mut batch = Batch::new();
    batch.add(NfListObject::Table(table(TABLE)));
    batch.add_cmd(NfCmd::Flush(FlushObject::Table(table(TABLE))));
    let chains: [(&'static str, NfHook, i32, Vec<Vec<Statement<'static>>>); 5] = [
        (
            CHAIN_SOURCES,
            NfHook::Prerouting,
            PRIO_RAW,
            pod_source_rules(host_prefix),
        ),
        (CHAIN_MARKS, NfHook::Prerouting, PRIO_MARKS, nat64_mark_rules()),
        (
            CHAIN_FORWARD,
            NfHook::Forward,
            PRIO_FILTER,
            forwarding_rules(host_prefix),
        ),
        (CHAIN_TO_HOST, NfHook::Input, PRIO_FILTER, to_host_rules()),
        (
            CHAIN_NAT64_DROPS,
            NfHook::Postrouting,
            PRIO_FILTER,
            nat64_drop_rules(),
        ),
    ];
    for (name, hook, prio, rules) in chains {
        batch.add(base_chain(TABLE, name, NfChainType::Filter, hook, prio));
        for stmts in rules {
            batch.add(rule_obj(TABLE, name, stmts));
        }
    }
    batch
}

pub(super) fn pod_source_rules(host_prefix: &Ipv6Net) -> Vec<Vec<Statement<'static>>> {
    vec![
        // spec: INFRA#pod-network-sources
        // A host-prefix source must arrive on the interface its route points
        // at, which puts each pod network's prefix on its own bridge.
        vec![
            match_meta_neq(MetaKey::Iifname, "lo"),
            match_eq(payload_expr("ip6", "saddr"), host_prefix_expr(host_prefix)),
            match_eq(
                Expression::Named(NamedExpression::Fib(Fib {
                    result: FibResult::Oif,
                    flags: [FibFlag::Saddr, FibFlag::Iif].into_iter().collect(),
                })),
                Expression::Boolean(false),
            ),
            Statement::Drop(None),
        ],
        // spec: INFRA#pod-network-sources
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_nfproto(NFPROTO_IPV4),
            Statement::Drop(None),
        ],
        // spec: INFRA#pod-network-sources
        vec![
            match_pod_bridge(MetaKey::Iifname),
            Statement::Match(Match {
                left: payload_expr("ip6", "saddr"),
                right: Expression::Named(NamedExpression::Set(vec![
                    SetItem::Element(host_prefix_expr(host_prefix)),
                    SetItem::Element(Expression::String(Cow::Borrowed("::"))),
                    SetItem::Element(prefix_expr("fe80::".to_owned(), 10)),
                ])),
                op: Operator::NEQ,
            }),
            Statement::Drop(None),
        ],
    ]
}

// spec: INFRA#pod-traffic-to-the-host
pub(super) fn nat64_mark_rules() -> Vec<Vec<Statement<'static>>> {
    let to_nat64 = || {
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_eq(
                payload_expr("ip6", "daddr"),
                prefix_expr(
                    NAT64_PREFIX_NET.network().to_string(),
                    NAT64_PREFIX_NET.prefix_len(),
                ),
            ),
        ]
    };
    let mut pod = to_nat64();
    pod.push(or_mark(MARK_POD_NAT64));
    let mut mount = to_nat64();
    mount.extend([match_ct_status_dnat(), or_mark(MARK_MOUNT)]);
    vec![pod, mount]
}

pub(super) fn forwarding_rules(host_prefix: &Ipv6Net) -> Vec<Vec<Statement<'static>>> {
    let resolver = resolver_addr(host_prefix).to_string();
    let to_resolver = |proto: &'static str| {
        vec![
            match_eq(payload_expr("ip6", "saddr"), host_prefix_expr(host_prefix)),
            match_eq(
                payload_expr("ip6", "daddr"),
                Expression::String(Cow::Owned(resolver.clone())),
            ),
            match_eq(payload_expr(proto, "dport"), Expression::Number(53)),
            Statement::Accept(None),
        ]
    };
    vec![
        // spec: INFRA#forwarding-policy
        vec![match_ct_state_established_related(), Statement::Accept(None)],
        // spec: INFRA#forwarding-policy
        vec![match_ct_status_dnat(), Statement::Accept(None)],
        // spec: INFRA#forwarding-policy
        vec![
            match_meta_eq(MetaKey::Iifname, PROXY_BRIDGE),
            Statement::Accept(None),
        ],
        // spec: INFRA#forwarding-policy
        to_resolver("tcp"),
        to_resolver("udp"),
        // spec: INFRA#forwarding-policy
        vec![
            match_eq(payload_expr("ip6", "daddr"), host_prefix_expr(host_prefix)),
            Statement::Drop(None),
        ],
    ]
}

pub(super) fn to_host_rules() -> Vec<Vec<Statement<'static>>> {
    vec![
        // spec: INFRA#pod-traffic-to-the-host
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_ct_state_established_related(),
            Statement::Accept(None),
        ],
        // spec: INFRA#pod-traffic-to-the-host
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_ct_status_dnat(),
            Statement::Accept(None),
        ],
        // spec: INFRA#pod-traffic-to-the-host
        vec![
            match_pod_bridge(MetaKey::Iifname),
            match_eq(
                payload_expr("icmpv6", "type"),
                Expression::Named(NamedExpression::Set(
                    // Router solicitation, neighbour solicitation and
                    // neighbour advertisement.
                    [133, 135, 136]
                        .into_iter()
                        .map(|t| SetItem::Element(Expression::Number(t)))
                        .collect(),
                )),
            ),
            Statement::Accept(None),
        ],
        // spec: INFRA#pod-traffic-to-the-host
        vec![match_pod_bridge(MetaKey::Iifname), Statement::Drop(None)],
        // spec: INFRA#pod-traffic-to-the-host
        // A pod's traffic that NAT64 translated to one of the host's IPv4
        // addresses is delivered here, on loopback and with no connection
        // tracked, so only the marks tell it apart.
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_mark(MARK_MASK, MARK_POD_NAT64),
            Statement::Drop(None),
        ],
    ]
}

pub(super) fn nat64_drop_rules() -> Vec<Vec<Statement<'static>>> {
    vec![
        // spec: INFRA#pod-traffic-to-the-host
        vec![
            match_nfproto(NFPROTO_IPV4),
            match_eq(
                meta(MetaKey::Oifname),
                Expression::Named(NamedExpression::Set(vec![
                    SetItem::Element(Expression::String(Cow::Borrowed(PROXY_BRIDGE))),
                    SetItem::Element(Expression::String(Cow::Borrowed(RESOLVER_BRIDGE))),
                ])),
            ),
            match_mark(MARK_MASK, MARK_POD_NAT64),
            Statement::Drop(None),
        ],
        // spec: INFRA#pod-traffic-to-the-host
        // Only Jool hairpinning a pod's NAT64 traffic into another session
        // brings a pod's mark back out towards a pod.
        vec![
            match_nfproto(NFPROTO_IPV6),
            match_pod_bridge(MetaKey::Oifname),
            match_mark(MARK_POD_NAT64, MARK_POD_NAT64),
            Statement::Drop(None),
        ],
    ]
}

fn host_prefix_expr(host_prefix: &Ipv6Net) -> Expression<'static> {
    prefix_expr(host_prefix.network().to_string(), host_prefix.prefix_len())
}

fn meta(key: MetaKey) -> Expression<'static> {
    Expression::Named(NamedExpression::Meta(Meta { key }))
}

fn match_meta_eq(key: MetaKey, value: &'static str) -> Statement<'static> {
    match_eq(meta(key), Expression::String(Cow::Borrowed(value)))
}

fn match_meta_neq(key: MetaKey, value: &'static str) -> Statement<'static> {
    Statement::Match(Match {
        left: meta(key),
        right: Expression::String(Cow::Borrowed(value)),
        op: Operator::NEQ,
    })
}

/// `iifname "sdp-*"` or `oifname "sdp-*"`: any pod network's bridge.
fn match_pod_bridge(key: MetaKey) -> Statement<'static> {
    match_eq(
        meta(key),
        Expression::String(Cow::Owned(format!("{POD_BRIDGE_PREFIX}*"))),
    )
}

/// `meta mark & <mask> == <value>`.
fn match_mark(mask: u32, value: u32) -> Statement<'static> {
    match_eq(
        Expression::BinaryOperation(Box::new(BinaryOperation::AND(
            meta(MetaKey::Mark),
            Expression::Number(mask),
        ))),
        Expression::Number(value),
    )
}

/// `meta mark set meta mark | <bit>`, keeping every other bit of the mark.
fn or_mark(bit: u32) -> Statement<'static> {
    Statement::Mangle(Mangle {
        key: meta(MetaKey::Mark),
        value: Expression::BinaryOperation(Box::new(BinaryOperation::OR(vec![
            meta(MetaKey::Mark),
            Expression::Number(bit),
        ]))),
    })
}

/// `ct state { established, related }`.
pub(super) fn match_ct_state_established_related() -> Statement<'static> {
    Statement::Match(Match {
        left: Expression::Named(NamedExpression::CT(CT {
            key: Cow::Borrowed("state"),
            family: None,
            dir: None,
        })),
        right: Expression::List(vec![
            Expression::String(Cow::Borrowed("established")),
            Expression::String(Cow::Borrowed("related")),
        ]),
        op: Operator::IN,
    })
}
