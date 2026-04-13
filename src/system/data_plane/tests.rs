use std::net::{Ipv6Addr, SocketAddr};

use crate::system::types::{ForwardProto, IngressRule};

use super::nft::{ingress_rule_stmts, output_ingress_rule_stmts};

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
