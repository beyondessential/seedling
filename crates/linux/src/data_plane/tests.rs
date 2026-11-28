use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};

use ipnet::Ipv6Net;
use serde_json::json;

use seedling_core::system::types::{ForwardProto, IngressRule, ServiceDnatRule};

use super::{
    host_addresses::{AddrEvent, ElementChange, HostAddrs, Notice, held_v4, notice, set_elements},
    isolation::{
        MARK_MASK, MARK_MOUNT, MARK_POD_NAT64, forwarding_rules, isolation_batch, nat64_drop_rules,
        nat64_mark_rules, pod_source_rules, to_host_rules,
    },
    nft::{
        dnat_lb, ingress_rule_stmts, loopback_masquerade_stmts, nat64_ingress_rule_stmts,
        output_ingress_rule_stmts, service_dnat_output_stmts, service_dnat_prerouting_stmts,
    },
    translation_batch,
};

fn test_rule(port: u16, proto: ForwardProto) -> IngressRule {
    IngressRule {
        external_port: port,
        proto,
        caddy_v6: SocketAddr::new(
            std::net::IpAddr::V6(Ipv6Addr::new(0xfd5e, 0, 0, 0, 0, 0, 0, 1)),
            8080,
        ),
        caddy_v4: SocketAddrV4::new(Ipv4Addr::new(10, 89, 255, 2), 8080),
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
    let rule = test_rule(80, ForwardProto::Both);

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

// spec: INFRA#ingress-redirection
#[test]
fn ingress_traffic_of_either_family_is_redirected_to_the_proxys_address_of_that_family() {
    let rules: Vec<_> = ingress_rule_stmts(&test_rule(443, ForwardProto::Tcp))
        .iter()
        .map(|r| rule_json(r))
        .collect();
    let local = json!({"match": {"op": "==", "left": {"fib": {"result": "type", "flags": ["daddr"]}}, "right": "local"}});
    let dport = json!({"match": {"op": "==", "left": {"payload": {"protocol": "tcp", "field": "dport"}}, "right": 443}});
    assert_eq!(
        rules,
        [
            json!([nfproto(10), local, dport, {"dnat": {"addr": "fd5e::1", "family": "ip6", "port": 8080}}]),
            json!([nfproto(2), local, dport, {"dnat": {"addr": "10.89.255.2", "family": "ip", "port": 8080}}]),
        ]
    );
}

fn nets(nets: &[&str]) -> Vec<Ipv6Net> {
    nets.iter().map(|n| n.parse().unwrap()).collect()
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn the_nat64_set_holds_the_proxy_and_resolver_subnets_whatever_the_host_holds() {
    assert_eq!(
        set_elements([]),
        nets(&["64:ff9b::a59:fe00/120", "64:ff9b::a59:ff00/120"])
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn the_nat64_set_holds_each_host_address_once_outside_the_infrastructure_subnets() {
    let addrs = [
        "192.0.2.10",
        "10.89.255.1",
        "10.89.254.1",
        "127.0.0.1",
        "192.0.2.10",
    ]
    .map(|a| a.parse::<Ipv4Addr>().unwrap());
    assert_eq!(
        set_elements(addrs),
        nets(&[
            "64:ff9b::a59:fe00/120",
            "64:ff9b::a59:ff00/120",
            "64:ff9b::7f00:1/128",
            "64:ff9b::c000:20a/128",
        ])
    );
}

fn address_message(
    family: rtnetlink::packet_route::AddressFamily,
    attributes: Vec<rtnetlink::packet_route::address::AddressAttribute>,
) -> rtnetlink::packet_route::address::AddressMessage {
    let mut msg = rtnetlink::packet_route::address::AddressMessage::default();
    msg.header.family = family;
    msg.header.index = 3;
    msg.attributes = attributes;
    msg
}

#[test]
fn a_host_address_is_the_local_one_where_the_kernel_reports_a_peer() {
    use rtnetlink::packet_route::{AddressFamily, address::AddressAttribute};

    let local: Ipv4Addr = "192.0.2.10".parse().unwrap();
    let peer: Ipv4Addr = "192.0.2.1".parse().unwrap();
    let ptp = address_message(
        AddressFamily::Inet,
        vec![
            AddressAttribute::Address(peer.into()),
            AddressAttribute::Local(local.into()),
        ],
    );
    assert_eq!(held_v4(&ptp).unwrap(), Some((3, local)));

    let plain = address_message(
        AddressFamily::Inet,
        vec![AddressAttribute::Address(local.into())],
    );
    assert_eq!(held_v4(&plain).unwrap(), Some((3, local)));

    let v6 = address_message(
        AddressFamily::Inet6,
        vec![AddressAttribute::Address(Ipv6Addr::LOCALHOST.into())],
    );
    assert_eq!(held_v4(&v6).unwrap(), None);
}

#[test]
fn an_ipv4_address_report_without_the_address_is_an_error_not_an_absence() {
    let bare = address_message(rtnetlink::packet_route::AddressFamily::Inet, Vec::new());
    assert!(held_v4(&bare).is_err());
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn pod_traffic_to_the_host_over_nat64_at_an_ingress_port_is_redirected_to_the_proxy() {
    let rules: Vec<_> = nat64_ingress_rule_stmts(&test_rule(443, ForwardProto::Both))
        .iter()
        .map(|r| rule_json(r))
        .collect();
    let redirect = |proto: &str| {
        json!([
            iifname_pods(),
            {"match": {"op": "==", "left": {"payload": {"protocol": "ip6", "field": "daddr"}}, "right": "@host_v4_nat64"}},
            {"match": {"op": "==", "left": {"payload": {"protocol": proto, "field": "dport"}}, "right": 443}},
            {"dnat": {"addr": "fd5e::1", "family": "ip6", "port": 8080}},
        ])
    };
    assert_eq!(rules, [redirect("tcp"), redirect("udp")]);
}

// spec: INFRA#pod-traffic-to-the-host
// spec: INFRA#service-translation
#[test]
fn the_translation_table_is_replaced_whole_with_its_nat64_set_holding_only_what_is_given() {
    let rules = seedling_core::system::types::DataPlaneRules {
        ingress: vec![test_rule(443, ForwardProto::Tcp)],
        ..Default::default()
    };
    let elements = nets(&["64:ff9b::a59:ff00/120", "64:ff9b::c000:20a/128"]);
    let batch = serde_json::to_value(translation_batch(&rules, &elements).to_nftables()).unwrap();
    let cmds = batch["nftables"].as_array().unwrap();

    let table = json!({"family": "inet", "name": "seedling_net"});
    assert_eq!(cmds[0], json!({"add": {"table": table}}));
    assert_eq!(cmds[1], json!({"flush": {"table": table}}));
    let set = json!({
        "family": "inet",
        "table": "seedling_net",
        "name": "host_v4_nat64",
        "type": "ipv6_addr",
        "flags": ["interval"],
    });
    let at = |cmd: serde_json::Value| cmds.iter().position(|c| *c == cmd).expect("present");
    let add_set = at(json!({"add": {"set": set}}));
    let flush_set = at(json!({"flush": {"set": set}}));
    let add_elements = at(json!({"add": {"element": {
        "family": "inet",
        "table": "seedling_net",
        "name": "host_v4_nat64",
        "elem": [{"prefix": {"addr": "64:ff9b::a59:ff00", "len": 120}}, "64:ff9b::c000:20a"],
    }}}));
    let first_rule = cmds
        .iter()
        .position(|c| c["add"]["rule"].is_object())
        .unwrap();
    assert!(add_set < flush_set && flush_set < add_elements && add_elements < first_rule);
    assert!(
        cmds.iter()
            .filter_map(|c| c["add"]["rule"].as_object())
            .any(|r| r["expr"].to_string().contains("@host_v4_nat64")),
        "the NAT64 ingress redirect is in the batch"
    );
}

fn v4(addr: &str) -> Ipv4Addr {
    addr.parse().unwrap()
}

fn net(net: &str) -> Ipv6Net {
    net.parse().unwrap()
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn an_address_the_host_gains_or_loses_adds_or_deletes_its_element() {
    let mut addrs = HostAddrs::from_held([(2, v4("192.0.2.10"))]);
    assert_eq!(
        addrs.record(AddrEvent::Held {
            index: 2,
            addr: v4("198.51.100.7")
        }),
        Some(ElementChange::Add(net("64:ff9b::c633:6407/128")))
    );
    assert_eq!(
        addrs.record(AddrEvent::Released {
            index: 2,
            addr: v4("192.0.2.10")
        }),
        Some(ElementChange::Delete(net("64:ff9b::c000:20a/128")))
    );
    assert_eq!(addrs.elements(), set_elements([v4("198.51.100.7")]));
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn an_address_held_on_two_interfaces_keeps_its_element_until_both_release_it() {
    let addr = v4("192.0.2.10");
    let mut addrs = HostAddrs::from_held([(2, addr)]);
    assert_eq!(addrs.record(AddrEvent::Held { index: 3, addr }), None);
    assert_eq!(addrs.record(AddrEvent::Released { index: 2, addr }), None);
    assert_eq!(
        addrs.record(AddrEvent::Released { index: 3, addr }),
        Some(ElementChange::Delete(net("64:ff9b::c000:20a/128")))
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn a_notification_already_reflected_changes_nothing() {
    let addr = v4("192.0.2.10");
    let mut addrs = HostAddrs::from_held([(2, addr)]);
    assert_eq!(addrs.record(AddrEvent::Held { index: 2, addr }), None);
    assert_eq!(
        addrs.record(AddrEvent::Released {
            index: 4,
            addr: v4("203.0.113.1")
        }),
        None
    );
    assert_eq!(addrs, HostAddrs::from_held([(2, addr)]));
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn an_address_within_the_proxy_or_resolver_subnets_never_changes_the_set() {
    let mut addrs = HostAddrs::default();
    for addr in [v4("10.89.255.1"), v4("10.89.254.1")] {
        assert_eq!(addrs.record(AddrEvent::Held { index: 5, addr }), None);
        assert_eq!(addrs.record(AddrEvent::Released { index: 5, addr }), None);
    }
}

#[test]
fn address_notifications_and_overruns_are_told_apart_from_the_rest() {
    use rtnetlink::packet_core::{NetlinkMessage, NetlinkPayload};
    use rtnetlink::packet_route::{
        AddressFamily, RouteNetlinkMessage, address::AddressAttribute, link::LinkMessage,
    };

    let addr = v4("192.0.2.10");
    let msg = || {
        address_message(
            AddressFamily::Inet,
            vec![AddressAttribute::Local(addr.into())],
        )
    };
    let wrap = |payload| NetlinkMessage::new(Default::default(), payload);
    assert_eq!(
        notice(wrap(NetlinkPayload::InnerMessage(
            RouteNetlinkMessage::NewAddress(msg())
        )))
        .unwrap(),
        Some(Notice::Address(AddrEvent::Held { index: 3, addr }))
    );
    assert_eq!(
        notice(wrap(NetlinkPayload::InnerMessage(
            RouteNetlinkMessage::DelAddress(msg())
        )))
        .unwrap(),
        Some(Notice::Address(AddrEvent::Released { index: 3, addr }))
    );
    assert_eq!(
        notice(wrap(NetlinkPayload::Overrun(Vec::new()))).unwrap(),
        Some(Notice::Overrun)
    );
    assert_eq!(
        notice(wrap(NetlinkPayload::InnerMessage(
            RouteNetlinkMessage::NewLink(LinkMessage::default())
        )))
        .unwrap(),
        None
    );
}

// spec: INFRA#pod-traffic-to-the-host
#[test]
fn a_single_element_is_added_and_deleted_by_itself() {
    let cmd = |change: ElementChange| serde_json::to_value(change.cmd()).unwrap();
    let element = json!({
        "family": "inet",
        "table": "seedling_net",
        "name": "host_v4_nat64",
        "elem": ["64:ff9b::c000:20a"],
    });
    assert_eq!(
        cmd(ElementChange::Add(net("64:ff9b::c000:20a/128"))),
        json!({"add": {"element": element}})
    );
    assert_eq!(
        cmd(ElementChange::Delete(net("64:ff9b::c000:20a/128"))),
        json!({"delete": {"element": element}})
    );
}
