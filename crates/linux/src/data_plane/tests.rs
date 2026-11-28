use std::net::{Ipv6Addr, SocketAddr};

use ipnet::Ipv6Net;
use serde_json::json;

use seedling_core::system::types::{ForwardProto, IngressRule, ServiceDnatRule};

use super::{
    isolation::{
        MARK_MASK, MARK_MOUNT, MARK_POD_NAT64, forwarding_rules, isolation_batch, nat64_drop_rules,
        nat64_mark_rules, pod_source_rules, to_host_rules,
    },
    nft::{
        dnat_lb, ingress_rule_stmts, loopback_masquerade_stmts, output_ingress_rule_stmts,
        service_dnat_output_stmts, service_dnat_prerouting_stmts,
    },
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

// spec: INFRA#host-originated-traffic
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

// spec: INFRA#host-originated-traffic
#[test]
fn output_rules_are_ipv6_only_though_the_proxy_has_an_ipv4_address() {
    let mut rule = test_rule(80, ForwardProto::Both);
    rule.caddy_v4 = Some(SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2)),
        8080,
    ));

    let pre = ingress_rule_stmts(&rule);
    assert_eq!(pre.len(), 4, "prerouting covers IPv6 and IPv4");

    let out = output_ingress_rule_stmts(&rule);
    assert_eq!(out.len(), 2, "output covers the same ports and protocols");
    for stmts in &out {
        let json = serde_json::to_string(&stmts[0]).unwrap();
        assert!(json.contains("nfproto") && json.contains("10"), "{json}");
    }
}

// spec: INFRA#host-originated-traffic
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

// spec: INFRA#host-originated-traffic
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

// spec: INFRA#host-originated-traffic
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

// spec: INFRA#host-originated-traffic
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

// spec: INFRA#host-originated-traffic
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

// spec: SVC#routing
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

// spec: SVC#routing
// spec: INFRA#service-translation
// spec: INFRA#mount-endpoint-addressing
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

// spec: SVC#routing
// spec: INFRA#service-translation
// spec: INFRA#mount-endpoint-addressing
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

fn rule_json(stmts: &[nftables::stmt::Statement<'static>]) -> serde_json::Value {
    serde_json::to_value(stmts).expect("serialize")
}

fn iifname_pods() -> serde_json::Value {
    json!({"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "sdp-*"}})
}

fn host_prefix_json() -> serde_json::Value {
    json!({"prefix": {"addr": "fd5e:1234:5678::", "len": 48}})
}

fn nfproto(n: u32) -> serde_json::Value {
    json!({"match": {"op": "==", "left": {"meta": {"key": "nfproto"}}, "right": n}})
}

fn ct_established_related() -> serde_json::Value {
    json!({"match": {"op": "in", "left": {"ct": {"key": "state"}}, "right": ["established", "related"]}})
}

fn ct_status_dnat() -> serde_json::Value {
    json!({"match": {"op": "==", "left": {"&": [{"ct": {"key": "status"}}, "dnat"]}, "right": "dnat"}})
}

fn mark_is(mask: u32, value: u32) -> serde_json::Value {
    json!({"match": {"op": "==", "left": {"&": [{"meta": {"key": "mark"}}, mask]}, "right": value}})
}

// spec: INFRA#pod-network-sources
#[test]
fn host_prefix_sources_must_arrive_where_their_route_points_on_every_interface_but_loopback() {
    let rule = rule_json(&pod_source_rules(&sample_prefix())[0]);
    assert_eq!(
        rule[0],
        json!({"match": {"op": "!=", "left": {"meta": {"key": "iifname"}}, "right": "lo"}})
    );
    assert_eq!(
        rule[1],
        json!({"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "saddr"}}, "right": host_prefix_json()}})
    );
    // `fib saddr . iif oif missing`: the flags are a set, so their order is
    // not fixed.
    let fib = &rule[2]["match"];
    assert_eq!(fib["op"], "==");
    assert_eq!(fib["left"]["fib"]["result"], "oif");
    let mut flags: Vec<&str> = fib["left"]["fib"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    flags.sort_unstable();
    assert_eq!(flags, ["iif", "saddr"]);
    assert_eq!(fib["right"], false);
    assert_eq!(rule[3], json!({"drop": null}));
    assert_eq!(rule.as_array().unwrap().len(), 4);
}

// spec: INFRA#pod-network-sources
#[test]
fn a_pod_bridge_passes_no_ipv4_and_no_ipv6_source_outside_the_host_prefix() {
    let rules = pod_source_rules(&sample_prefix());
    assert_eq!(rules.len(), 3);
    assert_eq!(
        rule_json(&rules[1]),
        json!([iifname_pods(), nfproto(2), {"drop": null}])
    );
    assert_eq!(
        rule_json(&rules[2]),
        json!([
            iifname_pods(),
            {"match": {
                "op": "!=",
                "left": {"payload": {"protocol": "ip6", "field": "saddr"}},
                "right": {"set": [host_prefix_json(), "::", {"prefix": {"addr": "fe80::", "len": 10}}]},
            }},
            {"drop": null},
        ])
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn pod_traffic_to_the_nat64_prefix_is_marked_and_mounted_traffic_doubly() {
    let to_nat64 = || {
        vec![
            iifname_pods(),
            json!({"match": {
                "op": "==",
                "left": {"payload": {"protocol": "ip6", "field": "daddr"}},
                "right": {"prefix": {"addr": "64:ff9b::", "len": 96}},
            }}),
        ]
    };
    let or_mark = |bit: u32| json!({"mangle": {"key": {"meta": {"key": "mark"}}, "value": {"|": [{"meta": {"key": "mark"}}, bit]}}});
    let mut pod = to_nat64();
    pod.push(or_mark(MARK_POD_NAT64));
    let mut mount = to_nat64();
    mount.extend([ct_status_dnat(), or_mark(MARK_MOUNT)]);

    let rules = nat64_mark_rules();
    assert_eq!(rules.len(), 2);
    assert_eq!(rule_json(&rules[0]), json!(pod));
    assert_eq!(rule_json(&rules[1]), json!(mount));
}

#[test]
fn the_marks_keep_clear_of_the_bits_host_tools_route_by() {
    assert_eq!(MARK_MASK, MARK_POD_NAT64 | MARK_MOUNT);
    assert_eq!(MARK_POD_NAT64 & MARK_MOUNT, 0);
    assert_eq!(
        MARK_MASK & 0x00ff_ffff,
        0,
        "Tailscale's 0xff0000 and the low 16 bits"
    );
}

// spec: INFRA#forwarding-policy
#[test]
fn forwarding_to_the_host_prefix_is_dropped_but_for_the_exceptions() {
    let resolver_dns = |proto: &str| {
        json!([
            {"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "saddr"}}, "right": host_prefix_json()}},
            {"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "daddr"}}, "right": "fd5e:1234:5678:fd00::53"}},
            {"match": {"op": "==", "left": {"payload": {"protocol": proto, "field": "dport"}}, "right": 53}},
            {"accept": null},
        ])
    };
    let rules: Vec<_> = forwarding_rules(&sample_prefix())
        .iter()
        .map(|r| rule_json(r))
        .collect();
    assert_eq!(
        rules,
        [
            json!([ct_established_related(), {"accept": null}]),
            json!([ct_status_dnat(), {"accept": null}]),
            json!([
                {"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "sdl-proxy"}},
                {"accept": null},
            ]),
            resolver_dns("tcp"),
            resolver_dns("udp"),
            json!([
                {"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "daddr"}}, "right": host_prefix_json()}},
                {"drop": null},
            ]),
        ]
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn a_pod_reaches_the_host_only_by_accepted_or_mounted_connections_and_neighbour_discovery() {
    let rules: Vec<_> = to_host_rules().iter().map(|r| rule_json(r)).collect();
    assert_eq!(
        rules,
        [
            json!([iifname_pods(), ct_established_related(), {"accept": null}]),
            json!([iifname_pods(), ct_status_dnat(), {"accept": null}]),
            json!([
                iifname_pods(),
                {"match": {
                    "op": "==",
                    "left": {"payload": {"protocol": "icmpv6", "field": "type"}},
                    "right": {"set": [133, 135, 136]},
                }},
                {"accept": null},
            ]),
            json!([iifname_pods(), {"drop": null}]),
            json!([nfproto(2), mark_is(MARK_MASK, MARK_POD_NAT64), {"drop": null}]),
        ]
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn translated_pod_traffic_is_dropped_towards_the_proxy_resolver_and_pods() {
    let rules: Vec<_> = nat64_drop_rules().iter().map(|r| rule_json(r)).collect();
    assert_eq!(
        rules,
        [
            json!([
                nfproto(2),
                {"match": {
                    "op": "==",
                    "left": {"meta": {"key": "oifname"}},
                    "right": {"set": ["sdl-proxy", "sdl-resolver"]},
                }},
                mark_is(MARK_MASK, MARK_POD_NAT64),
                {"drop": null},
            ]),
            json!([
                nfproto(10),
                {"match": {"op": "==", "left": {"meta": {"key": "oifname"}}, "right": "sdp-*"}},
                mark_is(MARK_POD_NAT64, MARK_POD_NAT64),
                {"drop": null},
            ]),
        ]
    );
}

// spec: INFRA#pod-network-sources
// spec: INFRA#forwarding-policy
// spec: INFRA#pod-traffic-to-the-host
#[test]
fn the_isolation_table_is_replaced_whole_in_one_batch() {
    let batch = serde_json::to_value(isolation_batch(&sample_prefix()).to_nftables()).unwrap();
    let cmds = batch["nftables"].as_array().unwrap();
    let table = json!({"family": "inet", "name": "seedling_isolation"});
    assert_eq!(cmds[0], json!({"add": {"table": table}}));
    assert_eq!(cmds[1], json!({"flush": {"table": table}}));

    let chains: Vec<(String, String, i64)> = cmds
        .iter()
        .filter_map(|c| c["add"]["chain"].as_object())
        .map(|c| {
            assert_eq!(c["type"], "filter");
            assert_eq!(c["policy"], "accept");
            (
                c["name"].as_str().unwrap().to_owned(),
                c["hook"].as_str().unwrap().to_owned(),
                c["prio"].as_i64().unwrap(),
            )
        })
        .collect();
    let chain = |name: &str, hook: &str, prio: i64| (name.to_owned(), hook.to_owned(), prio);
    assert_eq!(
        chains,
        [
            chain("pod_sources", "prerouting", -300),
            chain("nat64_marks", "prerouting", -90),
            chain("forwarding", "forward", 0),
            chain("to_host", "input", 0),
            chain("nat64_drops", "postrouting", 0),
        ]
    );
    assert!(
        cmds.iter()
            .filter_map(|c| c["add"]["rule"].as_object())
            .all(|r| r["table"] == "seedling_isolation"),
        "every rule is in the isolation table"
    );
}

fn service_rule(proto: ForwardProto) -> ServiceDnatRule {
    ServiceDnatRule {
        service_ip: "fd5e:1234:5678:fc00::7".parse().unwrap(),
        service_port: 5432,
        proto,
        backends: vec![(Ipv6Addr::new(0xfd5e, 0x1234, 0x5678, 1, 0, 0, 0, 2), 5433)],
    }
}

fn service_translation(proto: &str) -> Vec<serde_json::Value> {
    vec![
        nfproto(10),
        json!({"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "daddr"}}, "right": "fd5e:1234:5678:fc00::7"}}),
        json!({"match": {"op": "==", "left": {"payload": {"protocol": proto, "field": "dport"}}, "right": 5432}}),
        json!({"dnat": {"addr": "fd5e:1234:5678:1::2", "family": "ip6", "port": 5433}}),
    ]
}

// spec: INFRA#service-translation
#[test]
fn service_traffic_arriving_from_outside_the_host_is_translated_only_from_the_proxy_network() {
    let rules: Vec<_> = service_dnat_prerouting_stmts(&service_rule(ForwardProto::Both))
        .iter()
        .map(|r| rule_json(r))
        .collect();
    let from_proxy = |proto: &str| {
        let mut stmts = vec![
            json!({"match": {"op": "==", "left": {"meta": {"key": "iifname"}}, "right": "sdl-proxy"}}),
        ];
        stmts.extend(service_translation(proto));
        json!(stmts)
    };
    assert_eq!(rules, [from_proxy("tcp"), from_proxy("udp")]);
}

// spec: INFRA#service-translation
#[test]
fn service_traffic_originating_on_the_host_is_translated_from_any_interface() {
    let rules: Vec<_> = service_dnat_output_stmts(&service_rule(ForwardProto::Tcp))
        .iter()
        .map(|r| rule_json(r))
        .collect();
    assert_eq!(rules, [json!(service_translation("tcp"))]);
}

// spec: INFRA#service-translation
#[test]
fn a_service_without_backends_is_not_translated() {
    let mut rule = service_rule(ForwardProto::Tcp);
    rule.backends.clear();
    assert!(service_dnat_prerouting_stmts(&rule).is_empty());
    assert!(service_dnat_output_stmts(&rule).is_empty());
}
