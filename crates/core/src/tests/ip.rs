use super::*;

/// Evaluate `source` in the static context.
fn eval(source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    engine.eval_with_scope(&mut scope, source)
}

fn truth(source: &str) -> bool {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .as_bool()
        .unwrap_or_else(|t| panic!("{source}: expected a boolean, got {t}"))
}

fn text(source: &str) -> String {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .into_string()
        .unwrap_or_else(|t| panic!("{source}: expected a string, got {t}"))
}

fn int(source: &str) -> rhai::INT {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .as_int()
        .unwrap_or_else(|t| panic!("{source}: expected an integer, got {t}"))
}

/// The reason a script error gives.
fn refusal(source: &str) -> String {
    match *eval(source).expect_err(source) {
        rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
        rhai::EvalAltResult::ErrorInFunctionCall(_, _, ref inner, _) => match **inner {
            rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
            ref other => panic!("{source}: expected a script error, got {other:?}"),
        },
        other => panic!("{source}: expected a script error, got {other:?}"),
    }
}

fn addr(value: &str) -> String {
    format!("std::parse_ip_address({value:?})")
}

fn subnet(value: &str) -> String {
    format!("std::parse_ip_subnet({value:?})")
}

fn range(value: &str) -> String {
    format!("std::parse_ip_range({value:?})")
}

/// Check that `parsed` is accepted, comes back as `canonical`, and that its
/// canonical form parses again to an equal value.
fn assert_canonical(parse: fn(&str) -> String, value: &str, canonical: &str) {
    assert_eq!(
        text(&format!("{}.to_string()", parse(value))),
        canonical,
        "{value}"
    );
    assert!(
        truth(&format!("{} == {}", parse(value), parse(canonical))),
        "{value} and {canonical} parse to the same value"
    );
}

// spec: IP#addresses
// spec: IP#text-of-a-value
#[test]
fn an_address_is_accepted_in_ipv4_or_ipv6_text() {
    for (value, canonical) in [
        ("192.0.2.1", "192.0.2.1"),
        ("0.0.0.0", "0.0.0.0"),
        ("255.255.255.255", "255.255.255.255"),
        ("::", "::"),
        ("::1", "::1"),
        ("2001:DB8::1", "2001:db8::1"),
        ("2001:0db8:0000:0000:0000:0000:0000:0001", "2001:db8::1"),
        ("1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8"),
        ("64:ff9b::192.0.2.1", "64:ff9b::c000:201"),
        ("::1.2.3.4", "::102:304"),
    ] {
        assert_canonical(addr, value, canonical);
    }
}

// spec: IP#addresses
// spec: IP#decimal-number
#[test]
fn an_address_outside_the_text_forms_is_refused() {
    for value in [
        "",
        "1.2.3",
        "1.2.3.4.5",
        "256.0.0.1",
        "01.2.3.4",
        "1.2.3.04",
        "0x1.2.3.4",
        "+1.2.3.4",
        " 1.2.3.4",
        "1.2.3.4 ",
        "1.2.3.4/32",
        "[::1]",
        "fe80::1%eth0",
        "fe80::1%1",
        "::1.2.3.04",
        "::256.1.1.1",
        "1::2::3",
        "12345::",
        "g::",
        "1:2:3:4:5:6:7:8:9",
    ] {
        let reason = refusal(&addr(value));
        assert!(reason.contains("is not an IP address"), "{value}: {reason}");
    }
}

// spec: IP#addresses
#[test]
fn an_ipv4_mapped_address_is_refused() {
    for value in [
        "::ffff:1.2.3.4",
        "::FFFF:0102:0304",
        "0:0:0:0:0:ffff:1.2.3.4",
    ] {
        let reason = refusal(&addr(value));
        assert!(
            reason.contains("IPv4-mapped") && reason.contains("1.2.3.4"),
            "{value}: {reason}"
        );
    }
}

// spec: IP#text-of-a-value
#[test]
fn a_mapped_range_end_is_not_an_ip_address_value() {
    let end = format!("{}.end()", range("::/80"));
    assert_eq!(
        text(&format!("{end}.to_string()")),
        "::ffff:255.255.255.255"
    );
    let reason = refusal(&format!("std::parse_ip_address({end}.to_string())"));
    assert!(reason.contains("IPv4-mapped"), "{reason}");
}

// spec: IP#subnets
// spec: IP#text-of-a-value
#[test]
fn a_subnet_is_accepted_with_or_without_a_prefix_length() {
    for (value, canonical) in [
        ("10.0.0.0/8", "10.0.0.0/8"),
        ("0.0.0.0/0", "0.0.0.0/0"),
        ("192.0.2.1/32", "192.0.2.1/32"),
        ("10.0.0.5", "10.0.0.5/32"),
        ("2001:DB8::/32", "2001:db8::/32"),
        ("::/0", "::/0"),
        ("::1", "::1/128"),
    ] {
        assert_canonical(subnet, value, canonical);
    }
}

// spec: IP#subnets
#[test]
fn a_subnet_with_host_bits_set_is_refused_naming_its_network() {
    for (value, network) in [
        ("10.0.0.5/24", "10.0.0.0/24"),
        ("2001:db8::1/32", "2001:db8::/32"),
    ] {
        let reason = refusal(&subnet(value));
        assert!(
            reason.contains("host bits") && reason.contains(network),
            "{value}: {reason}"
        );
    }
}

// spec: IP#subnets
// spec: IP#decimal-number
#[test]
fn a_subnet_prefix_length_is_a_decimal_number_within_the_family() {
    for value in [
        "10.0.0.0/33",
        "::/129",
        "10.0.0.0/08",
        "10.0.0.0/00",
        "10.0.0.0/+8",
        "10.0.0.0/-1",
        "10.0.0.0/",
        "10.0.0.0/ 8",
        "10.0.0.0/8/8",
        "10.0.0.0/256",
    ] {
        let reason = refusal(&subnet(value));
        assert!(reason.contains("prefix length"), "{value}: {reason}");
    }
    for value in ["/8", "::ffff:0:0/96", "10.0.0.0 /8"] {
        let reason = refusal(&subnet(value));
        assert!(
            reason.contains("not an IP address") || reason.contains("IPv4-mapped"),
            "{value}: {reason}"
        );
    }
}

// spec: IP#ranges
// spec: IP#text-of-a-value
#[test]
fn a_range_is_accepted_as_two_addresses_a_subnet_or_an_address() {
    for (value, canonical) in [
        ("10.0.0.1-10.0.0.9", "10.0.0.1-10.0.0.9"),
        ("10.0.0.1-10.0.0.1", "10.0.0.1-10.0.0.1"),
        ("10.0.0.0/30", "10.0.0.0-10.0.0.3"),
        ("10.0.0.7", "10.0.0.7-10.0.0.7"),
        ("2001:DB8::1-2001:db8::FF", "2001:db8::1-2001:db8::ff"),
        ("::/80", "::-::ffff:255.255.255.255"),
        ("::ffff:1.2.3.4-::1:0:0:0", "::ffff:1.2.3.4-::1:0:0:0"),
    ] {
        assert_canonical(range, value, canonical);
    }
}

// spec: IP#ranges
#[test]
fn a_range_outside_its_forms_is_refused() {
    for (value, reason) in [
        ("10.0.0.9-10.0.0.1", "follows"),
        ("10.0.0.1-::1", "joins an IPv4 and an IPv6 address"),
        ("::ffff:1.2.3.4-::ffff:1.2.3.9", "IPv4-mapped"),
        ("::ffff:1.2.3.4", "IPv4-mapped"),
        ("10.0.0.1 - 10.0.0.9", "not an IP address"),
        ("10.0.0.1-", "not an IP address"),
        ("-10.0.0.1", "not an IP address"),
        ("10.0.0.1-10.0.0.2-10.0.0.3", "not an IP address"),
        ("10.0.0.0/24-10.0.1.0", "not an IP address"),
        ("10.0.0.5/24", "host bits"),
    ] {
        let got = refusal(&range(value));
        assert!(got.contains(reason), "{value}: {got}");
    }
}

// spec: IP#family-argument
#[test]
fn a_parser_given_a_family_refuses_the_other_one() {
    for (parse, v4, v6, what) in [
        (addr as fn(&str) -> String, "10.0.0.1", "::1", "address"),
        (subnet, "10.0.0.0/8", "2001:db8::/32", "subnet"),
        (range, "10.0.0.1-10.0.0.9", "::1-::9", "range"),
    ] {
        let with = |value: &str, family: i64| {
            let call = parse(value);
            format!("{}, {family})", call.strip_suffix(')').unwrap())
        };
        assert!(truth(&format!("{} == {}", with(v4, 4), parse(v4))));
        assert!(truth(&format!("{} == {}", with(v6, 6), parse(v6))));
        assert_eq!(
            refusal(&with(v6, 4)),
            format!("must be an IPv4 {what}, not an IPv6 one")
        );
        assert_eq!(
            refusal(&with(v4, 6)),
            format!("must be an IPv6 {what}, not an IPv4 one")
        );
    }
}

// spec: IP#family-argument
// spec: ACN#kind-parsers
#[test]
fn a_parser_given_an_invalid_family_throws() {
    for helper in ["parse_ip_address", "parse_ip_subnet", "parse_ip_range"] {
        assert_eq!(
            refusal(&format!(r#"std::{helper}("10.0.0.1", 5)"#)),
            "`family` must be 4 or 6, got 5"
        );
        for family in [r#""4""#, "4.0", "()"] {
            let reason = refusal(&format!(r#"std::{helper}("10.0.0.1", {family})"#));
            assert!(
                reason.starts_with("`family` must be the integer 4 or 6"),
                "{helper} {family}: {reason}"
            );
        }
    }
}

// spec: IP#address-values
// spec: IP#subnet-values
// spec: IP#range-values
#[test]
fn every_value_tells_its_family() {
    for (source, family) in [
        (addr("10.0.0.1"), 4),
        (addr("::1"), 6),
        (subnet("10.0.0.0/8"), 4),
        (subnet("::/0"), 6),
        (range("10.0.0.1-10.0.0.2"), 4),
        (range("::/80"), 6),
        (format!("{}.end()", range("::/80")), 6),
    ] {
        assert_eq!(int(&format!("{source}.family()")), family, "{source}");
    }
}

// spec: IP#address-values
#[test]
fn an_address_converts_to_a_subnet_or_a_range() {
    for (source, expected) in [
        (format!("{}.to_subnet()", addr("10.0.0.5")), "10.0.0.5/32"),
        (format!("{}.to_subnet()", addr("::1")), "::1/128"),
        (format!("{}.to_subnet(24)", addr("10.0.0.5")), "10.0.0.0/24"),
        (format!("{}.to_subnet(0)", addr("10.0.0.5")), "0.0.0.0/0"),
        (format!("{}.to_subnet(32)", addr("10.0.0.5")), "10.0.0.5/32"),
        (
            format!("{}.to_subnet(64)", addr("2001:db8::1")),
            "2001:db8::/64",
        ),
        (
            format!("{}.to_subnet(128)", addr("2001:db8::1")),
            "2001:db8::1/128",
        ),
        (
            format!("{}.to_range()", addr("10.0.0.5")),
            "10.0.0.5-10.0.0.5",
        ),
    ] {
        assert_eq!(text(&format!("{source}.to_string()")), expected, "{source}");
    }
    for (source, prefix_len) in [
        (addr("10.0.0.5"), "33"),
        (addr("10.0.0.5"), "-1"),
        (addr("::1"), "129"),
    ] {
        let reason = refusal(&format!("{source}.to_subnet({prefix_len})"));
        assert!(reason.contains("`prefix_len`"), "{prefix_len}: {reason}");
    }
}

// spec: IP#subnet-values
// spec: IP#range-values
#[test]
fn a_subnet_and_a_range_give_their_parts() {
    let net = subnet("10.0.0.0/8");
    assert!(truth(&format!("{net}.address() == {}", addr("10.0.0.0"))));
    assert_eq!(int(&format!("{net}.prefix_len()")), 8);
    assert_eq!(
        text(&format!("{net}.to_range().to_string()")),
        "10.0.0.0-10.255.255.255"
    );
    assert_eq!(
        text(&format!(
            "{}.to_range().to_string()",
            subnet("2001:db8::/32")
        )),
        "2001:db8::-2001:db8:ffff:ffff:ffff:ffff:ffff:ffff"
    );

    let pool = range("10.0.0.1-10.0.0.9");
    assert!(truth(&format!("{pool}.start() == {}", addr("10.0.0.1"))));
    assert!(truth(&format!("{pool}.end() == {}", addr("10.0.0.9"))));
}

// spec: IP#containment-and-overlap
#[test]
fn a_subnet_or_range_tests_containment_and_overlap() {
    let net = subnet("10.0.0.0/24");
    let pool = range("10.0.0.1-10.0.0.9");
    for (holder, other, contains, overlaps) in [
        (&net, addr("10.0.0.5"), true, true),
        (&net, addr("10.0.1.0"), false, false),
        (&net, subnet("10.0.0.128/25"), true, true),
        (&net, subnet("10.0.0.0/23"), false, true),
        (&net, subnet("10.0.0.0/24"), true, true),
        (&net, range("10.0.0.1-10.0.0.255"), true, true),
        (&net, range("10.0.0.250-10.0.1.5"), false, true),
        (&net, range("10.0.1.0-10.0.1.5"), false, false),
        (&pool, addr("10.0.0.9"), true, true),
        (&pool, addr("10.0.0.10"), false, false),
        (&pool, subnet("10.0.0.4/30"), true, true),
        (&pool, subnet("10.0.0.8/30"), false, true),
        (&pool, range("10.0.0.9-10.0.0.20"), false, true),
        (&pool, range("10.0.0.10-10.0.0.20"), false, false),
        (&pool, range("10.0.0.1-10.0.0.9"), true, true),
    ] {
        assert_eq!(
            truth(&format!("{holder}.contains({other})")),
            contains,
            "{holder} contains {other}"
        );
        assert_eq!(
            truth(&format!("{holder}.overlaps({other})")),
            overlaps,
            "{holder} overlaps {other}"
        );
    }
}

// spec: IP#containment-and-overlap
#[test]
fn containment_and_overlap_are_false_across_families() {
    let mapped = format!("{}.end()", range("::/80"));
    for (holder, other) in [
        (subnet("::/0"), addr("10.0.0.1")),
        (subnet("0.0.0.0/0"), addr("::1")),
        (subnet("0.0.0.0/0"), mapped.clone()),
        (range("::-::ffff:ffff:ffff"), range("0.0.0.0/0")),
    ] {
        assert!(!truth(&format!("{holder}.contains({other})")));
        assert!(!truth(&format!("{holder}.overlaps({other})")));
    }
    assert!(truth(&format!("{}.contains({mapped})", subnet("::/0"))));
}

// spec: IP#containment-and-overlap
#[test]
fn containment_and_overlap_take_only_ip_values() {
    for holder in [subnet("10.0.0.0/8"), range("10.0.0.1-10.0.0.9")] {
        for method in ["contains", "overlaps"] {
            for (other, type_name) in [
                (r#""10.0.0.1""#, "string"),
                ("5", "i64"),
                (&*a_version(), "Version"),
            ] {
                assert_eq!(
                    refusal(&format!("{holder}.{method}({other})")),
                    format!("`other` must be an IpAddress, IpSubnet or IpRange, got {type_name}"),
                );
            }
        }
    }
}

fn a_version() -> String {
    r#"std::parse_version("1.0")"#.to_owned()
}

// spec: IP#address-values
// spec: IP#address-order
#[test]
fn addresses_compare_in_address_order_within_a_family() {
    for (left, op, right, holds) in [
        ("10.0.0.1", "<", "10.0.0.2", true),
        ("10.0.0.9", "<", "10.0.0.10", true),
        ("10.0.0.10", "<=", "10.0.0.9", false),
        ("::2", ">", "::1", true),
        ("2001:db8::", ">=", "::fffe:ffff:ffff", true),
        ("2001:db8::1", "==", "2001:DB8:0::1", true),
        ("2001:db8::1", "!=", "2001:db8::2", true),
        ("10.0.0.1", "==", "::1", false),
        ("1.2.3.4", "==", "::1.2.3.4", false),
        ("10.0.0.1", "!=", "::1", true),
    ] {
        let source = format!("{} {op} {}", addr(left), addr(right));
        assert_eq!(truth(&source), holds, "{source}");
    }
    for op in ["<", "<=", ">", ">="] {
        assert_eq!(
            refusal(&format!("{} {op} {}", addr("10.0.0.1"), addr("::1"))),
            "cannot order an IPv4 address against an IPv6 one"
        );
    }
    let sorted = text(&format!(
        r#"let xs = [{}, {}, {}]; xs.sort(|a, b| a <= b); `${{xs[0]}} ${{xs[1]}} ${{xs[2]}}`"#,
        addr("10.0.0.10"),
        addr("10.0.0.9"),
        addr("9.255.255.255"),
    ));
    assert_eq!(sorted, "9.255.255.255 10.0.0.9 10.0.0.10");
}

// spec: IP#subnet-values
// spec: IP#range-values
#[test]
fn subnets_and_ranges_compare_for_equality_only() {
    for (source, holds) in [
        (
            format!("{} == {}", subnet("10.0.0.0/8"), subnet("10.0.0.0/8")),
            true,
        ),
        (
            format!("{} == {}", subnet("10.0.0.0/8"), subnet("10.0.0.0/16")),
            false,
        ),
        (
            format!("{} != {}", subnet("0.0.0.0/0"), subnet("::/0")),
            true,
        ),
        (
            format!("{} == {}", range("10.0.0.0/30"), range("10.0.0.0-10.0.0.3")),
            true,
        ),
        (
            format!("{} == {}", range("10.0.0.0/30"), range("10.0.0.0-10.0.0.4")),
            false,
        ),
        (format!("{} != {}", range("10.0.0.7"), range("::7")), true),
    ] {
        assert_eq!(truth(&source), holds, "{source}");
    }
    for op in ["<", "<=", ">", ">="] {
        assert_eq!(
            refusal(&format!(
                "{} {op} {}",
                subnet("10.0.0.0/8"),
                subnet("10.0.0.0/16")
            )),
            format!("subnets have no order, so {op} cannot compare them")
        );
        assert_eq!(
            refusal(&format!("{} {op} {}", range("10.0.0.1"), range("10.0.0.2"))),
            format!("ranges have no order, so {op} cannot compare them")
        );
    }
}

// spec: IP#mixing-types
#[test]
fn comparing_with_another_type_raises() {
    let values = [addr("10.0.0.1"), subnet("10.0.0.1"), range("10.0.0.1")];
    for op in ["==", "!=", "<", "<=", ">", ">="] {
        for (i, value) in values.iter().enumerate() {
            let mut others: Vec<String> = ["\"10.0.0.1\"", "1", "()", "true"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            others.push(a_version());
            others.extend(
                values
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(_, other)| other.clone()),
            );
            for other in &others {
                for source in [
                    format!("{value} {op} {other}"),
                    format!("{other} {op} {value}"),
                ] {
                    let reason = refusal(&source);
                    assert!(reason.contains("cannot compare"), "{source}: {reason}");
                }
            }
        }
    }
    for (source, names) in [
        (
            format!("{} == \"10.0.0.1\"", addr("10.0.0.1")),
            "IpAddress == string",
        ),
        (
            format!("{} < {}", subnet("10.0.0.1"), range("10.0.0.1")),
            "IpSubnet < IpRange",
        ),
        (
            format!("{} != {}", a_version(), range("10.0.0.1")),
            "Version != IpRange",
        ),
        (
            format!(
                "{} >= std::parse_version_semver(\"1.2.3\")",
                addr("10.0.0.1")
            ),
            "IpAddress >= SemVer",
        ),
    ] {
        let reason = refusal(&source);
        assert!(
            reason.starts_with(&format!("cannot compare {names}: ")),
            "{source}: {reason}"
        );
    }
}

// spec: IP#text-of-a-value
#[test]
fn values_print_as_their_canonical_text() {
    for (source, written) in [
        (addr("2001:DB8:0::1"), "2001:db8::1"),
        (subnet("10.0.0.5"), "10.0.0.5/32"),
        (range("10.0.0.0/31"), "10.0.0.0-10.0.0.1"),
    ] {
        assert_eq!(text(&format!("{source}.to_string()")), written);
        assert_eq!(text(&format!("let x = {source}; `${{x}}`")), written);
        assert_eq!(text(&format!("let x = {source}; \"\" + x")), written);
    }
}
