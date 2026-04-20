use std::net::{Ipv6Addr, SocketAddr};

use ipnet::Ipv6Net;

use crate::system::types::{ForwardProto, IngressRule};

use super::nft::{
    ct_state_established_related_accept, ct_status_dnat_accept, dnat_lb,
    drop_unsolicited_inbound_stmts, ingress_rule_stmts, loopback_masquerade_stmts,
    output_ingress_rule_stmts, seedling_forward_stmts,
};

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

fn sample_prefix() -> Ipv6Net {
    "fd5e:1234:5678::/48".parse().expect("valid prefix")
}

// r[verify infra.dataplane.forward-policy]
#[test]
fn ct_state_established_related_accept_json() {
    let stmts = ct_state_established_related_accept();
    let json = serde_json::to_string(&stmts).expect("serialize");
    assert!(
        json.contains("\"ct\""),
        "must contain ct expression, got: {json}"
    );
    assert!(
        json.contains("\"state\""),
        "must contain state key, got: {json}"
    );
    assert!(
        json.contains("\"established\""),
        "must contain established, got: {json}"
    );
    assert!(
        json.contains("\"related\""),
        "must contain related, got: {json}"
    );
    assert!(json.contains("\"in\""), "must use IN operator, got: {json}");
}

// r[verify infra.dataplane.forward-policy]
#[test]
fn ct_status_dnat_accept_json() {
    let stmts = ct_status_dnat_accept();
    let json = serde_json::to_string(&stmts).expect("serialize");
    assert!(
        json.contains("\"ct\""),
        "must contain ct expression, got: {json}"
    );
    assert!(
        json.contains("\"status\""),
        "must contain status key, got: {json}"
    );
    assert!(json.contains("\"dnat\""), "must contain dnat, got: {json}");
    assert!(
        json.contains("\"&\""),
        "must contain bitwise AND expression, got: {json}"
    );
    assert!(
        json.contains("\"==\""),
        "must use EQ as the match operator, got: {json}"
    );
}

// r[verify infra.dataplane.forward-policy]
#[test]
fn drop_unsolicited_inbound_stmts_json() {
    let prefix = sample_prefix();
    let stmts = drop_unsolicited_inbound_stmts(&prefix);
    let json = serde_json::to_string(&stmts).expect("serialize");
    assert!(json.contains("\"daddr\""), "must match daddr, got: {json}");
    assert!(
        json.contains("\"fd5e:1234:5678::\""),
        "must contain node prefix addr, got: {json}"
    );
    assert!(
        json.contains("\"48\"") || json.contains("48"),
        "must contain prefix len 48, got: {json}"
    );
    assert!(json.contains("\"drop\""), "must contain drop, got: {json}");
    assert!(
        !json.contains("\"accept\""),
        "must NOT contain accept, got: {json}"
    );
}

// l[verify service.routing]
#[test]
fn dnat_lb_single_backend() {
    let backends = vec![(Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1), 8080)];
    let stmt = dnat_lb(&backends);
    let json = serde_json::to_string(&stmt).expect("serialize");
    assert!(json.contains("\"dnat\""), "must be a dnat, got: {json}");
    assert!(
        json.contains("fd5e::1"),
        "must contain backend addr, got: {json}"
    );
    assert!(
        json.contains("8080"),
        "must contain backend port, got: {json}"
    );
    assert!(
        !json.contains("\"numgen\""),
        "single backend must not use numgen, got: {json}"
    );
}

// l[verify service.routing]
#[test]
fn dnat_lb_multiple_backends_uniform_port() {
    let backends = vec![
        (Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1), 8080),
        (Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 2), 8080),
    ];
    let stmt = dnat_lb(&backends);
    let json = serde_json::to_string(&stmt).expect("serialize");
    assert!(
        json.contains("\"numgen\""),
        "multiple backends must use numgen round-robin, got: {json}"
    );
    assert!(
        json.contains("\"concat\""),
        "map values must be addr.port concatenations, got: {json}"
    );
    assert!(
        json.contains("fd5e::1"),
        "must contain first backend addr, got: {json}"
    );
    assert!(
        json.contains("fd5e::2"),
        "must contain second backend addr, got: {json}"
    );
}

// l[verify service.routing]
#[test]
fn dnat_lb_multiple_backends_mixed_ports() {
    let backends = vec![
        (Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1), 8080),
        (Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 2), 9090),
    ];
    let stmt = dnat_lb(&backends);
    let json = serde_json::to_string(&stmt).expect("serialize");
    assert!(
        json.contains("\"numgen\""),
        "multiple backends must use numgen round-robin, got: {json}"
    );
    assert!(
        json.contains("\"concat\""),
        "map values must be addr.port concatenations, got: {json}"
    );
    assert!(
        json.contains("8080"),
        "must contain first backend port, got: {json}"
    );
    assert!(
        json.contains("9090"),
        "must contain second backend port, got: {json}"
    );
    // The DNAT port field must be None (embedded in concat), not a top-level port.
    assert!(
        !json.contains("\"port\""),
        "port must not appear as a separate DNAT field when using concat, got: {json}"
    );
}

// r[verify infra.dataplane.forward-policy]
#[test]
fn seedling_forward_stmts_json() {
    let prefix = sample_prefix();
    let stmts = seedling_forward_stmts(&prefix);
    let json = serde_json::to_string(&stmts).expect("serialize");
    assert!(json.contains("\"saddr\""), "must match saddr, got: {json}");
    assert!(json.contains("\"daddr\""), "must match daddr, got: {json}");
    assert!(
        json.contains("\"accept\""),
        "must contain accept, got: {json}"
    );
}

#[test]
fn loopback_masquerade_scoped_to_dnat_connections() {
    let stmts_list = loopback_masquerade_stmts();
    assert_eq!(stmts_list.len(), 2, "one rule per address family");
    for stmts in &stmts_list {
        let json = serde_json::to_string(stmts).expect("serialize");
        assert!(
            json.contains("\"masquerade\""),
            "rule must masquerade, got: {json}"
        );
        assert!(
            json.contains("\"ct\"") && json.contains("\"status\"") && json.contains("\"dnat\""),
            "rule must be gated on ct status dnat so plain loopback traffic \
             (e.g. DNS to 127.0.0.53) is not masqueraded, got: {json}"
        );
    }
}
