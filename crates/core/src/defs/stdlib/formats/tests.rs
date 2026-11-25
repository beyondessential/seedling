use rhai::{Dynamic, Engine, EvalAltResult};

use super::super::{HelperLimits, module};

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_static_module("std", module().into());
    engine.build_type::<crate::defs::version::Version>();
    engine.set_default_tag(Dynamic::from(HelperLimits {
        derived_values: 0,
        password_hashes: 0,
    }));
    engine
}

fn eval(src: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    engine().eval::<Dynamic>(src)
}

fn text(src: &str) -> String {
    eval(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
        .into_string()
        .unwrap()
}

fn throws(src: &str) -> String {
    eval(src).expect_err(src).to_string()
}

/// A script's value as JSON, to compare decoded values by.
fn json(src: &str) -> serde_json::Value {
    crate::defs::json::from_dynamic(eval(src).unwrap_or_else(|e| panic!("{src}: {e}"))).unwrap()
}

/// A string literal holding `s`, for splicing into a script.
fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

const SAMPLE: &str = r#"#{ name: "web", port: 8080, ratio: 0.5, on: true, none: (), tags: ["a", "b"], nested: #{ z: 1, a: [] } }"#;

// spec: FMT#json
// spec: FMT#key-order
// spec: FMT#layout
#[test]
fn json_is_indented_by_two_and_sorted() {
    assert_eq!(
        text(&format!("std::encode_json({SAMPLE})")),
        r#"{
  "name": "web",
  "nested": {
    "a": [],
    "z": 1
  },
  "none": null,
  "on": true,
  "port": 8080,
  "ratio": 0.5,
  "tags": [
    "a",
    "b"
  ]
}
"#
    );
}

// spec: FMT#json
#[test]
fn json_takes_any_value_and_escapes_only_what_it_must() {
    assert_eq!(text("std::encode_json(1)"), "1\n");
    assert_eq!(text("std::encode_json(1.0)"), "1.0\n");
    assert_eq!(text("std::encode_json(1e300)"), "1e+300\n");
    assert_eq!(text("std::encode_json([])"), "[]\n");
    assert_eq!(text("std::encode_json(#{})"), "{}\n");
    assert_eq!(
        text(r#"std::encode_json("é\"\\\n\u0001/")"#),
        "\"é\\\"\\\\\\n\\u0001/\"\n"
    );
}

// spec: FMT#values-each-format-holds
#[test]
fn a_version_is_written_as_its_text() {
    let mut engine = engine();
    engine.register_fn("v", |s: &str| {
        crate::defs::version::Version::parse(s).unwrap()
    });
    let out = engine
        .eval::<String>(r#"std::encode_json(#{ v: v("1.2.3") })"#)
        .unwrap();
    assert_eq!(out, "{\n  \"v\": \"1.2.3\"\n}\n");
}

// spec: FMT#refusal
#[test]
fn a_refusal_names_the_position_and_the_reason() {
    let err = throws(r#"std::encode_json(#{ a: [1, #{ "b c": 1.0 / 0.0 }] })"#);
    assert!(
        err.contains(
            r#"`value["a"][1]["b c"]` cannot be encoded: JSON has no infinite or NaN number"#
        ),
        "{err}"
    );
    let err = throws(r#"std::encode_toml(#{ a: #{ b: () } })"#);
    assert!(
        err.contains(r#"`value["a"]["b"]` cannot be encoded: TOML has no null"#),
        "{err}"
    );
    let err = throws(r#"std::encode_yaml([blob(2)])"#);
    assert!(
        err.contains("`value[0]` cannot be encoded: YAML has no type for blob"),
        "{err}"
    );
    let err = throws(r#"std::encode_env(#{ A: [1] })"#);
    assert!(
        err.contains(r#"`value["A"]` cannot be encoded: an env file has no arrays"#),
        "{err}"
    );
    let err = throws(r#"std::encode_env(#{ A: #{} })"#);
    assert!(err.contains("an env file has no nested maps"), "{err}");
    let err = throws(r#"std::encode_env(#{ A: () })"#);
    assert!(err.contains("an env file has no null"), "{err}");
}

// spec: FMT#helpers
#[test]
fn toml_and_env_take_only_a_map() {
    for helper in ["encode_toml", "encode_env"] {
        let err = throws(&format!("std::{helper}([1])"));
        assert!(err.contains("`value` must be a map, got array"), "{err}");
    }
}

// spec: FMT#helpers
#[test]
fn nesting_deeper_than_128_levels_throws() {
    let nested = |levels: usize| format!("{}1{}", "[".repeat(levels), "]".repeat(levels));
    // A literal that deep is beyond the engine's own expression depth.
    let encode = |levels: usize| {
        eval(&format!(
            "let v = 1; for i in 0..{levels} {{ v = [v]; }} std::encode_json(v)"
        ))
    };
    let _ = encode(128).unwrap();
    let err = encode(129).expect_err("129 levels").to_string();
    assert!(err.contains("more than 128 levels deep"), "{err}");

    let yaml = |levels: usize| lit(&nested(levels));
    let _ = eval(&format!("std::decode_yaml({})", yaml(128))).unwrap();
    let err = throws(&format!("std::decode_yaml({})", yaml(129)));
    assert!(err.contains("more than 128 levels deep"), "{err}");
}

// spec: FMT#yaml
#[test]
fn yaml_is_block_style_indented_by_two() {
    assert_eq!(
        text(&format!("std::encode_yaml({SAMPLE})")),
        "name: web
nested:
  a: []
  z: 1
none: null
\"on\": true
port: 8080
ratio: 0.5
tags:
  - a
  - b
"
    );
    assert_eq!(
        text("std::encode_yaml([#{ a: 1, b: [2] }, []])"),
        "- a: 1\n  b:\n    - 2\n- []\n"
    );
    assert_eq!(text("std::encode_yaml(#{})"), "{}\n");
    assert_eq!(text("std::encode_yaml(1.0)"), "1.0\n");
    assert_eq!(text("std::encode_yaml(-1.0 / 0.0)"), "-.inf\n");
}

// spec: FMT#yaml
#[test]
fn yaml_quotes_strings_a_reader_could_misread() {
    for s in [
        "", " a", "a ", "\ta", "~", "no", "No", "NO", "yes", "on", "Off", "true", "False", "null",
        "NULL", "0755", "1e3", "0x1F", "0o17", "-1", "+1", ".5", ".inf", "1.5", "a: b", "- a",
        "#a", "a #b", "[a]", "{a}", "&a", "*a", "!a", "|a", ">a", "%a", "@a", "`a", "'a", "\"a",
        "a\nb",
    ] {
        let out = text(&format!("std::encode_yaml({})", lit(s)));
        assert!(out.starts_with('"'), "{s:?} written as {out:?}");
        let back = text(&format!("std::decode_yaml({})", lit(&out)));
        assert_eq!(back, s, "{out:?}");
    }
    for s in ["web", "y", "n", "a b", "1.2.3", "v1", "a/b"] {
        let out = text(&format!("std::encode_yaml({})", lit(s)));
        assert_eq!(out, format!("{s}\n"));
    }
}

// spec: FMT#yaml
#[test]
fn yaml_quotes_keys_by_the_same_rules() {
    assert_eq!(
        text(r#"std::encode_yaml(#{ on: 1, "": 2, web: 3 })"#),
        "\"\": 2\n\"on\": 1\nweb: 3\n"
    );
}

// spec: FMT#yaml
#[test]
fn yaml_escapes_characters_no_plain_scalar_may_hold() {
    let s = "a\u{85}b\u{9f}\u{fffe}";
    let out = text(&format!("std::encode_yaml({})", lit(s)));
    assert_eq!(out, "\"a\\u0085b\\u009f\\ufffe\"\n");
    assert_eq!(text(&format!("std::decode_yaml({})", lit(&out))), s);
}

// spec: FMT#toml
#[test]
fn toml_writes_tables_after_key_value_lines() {
    assert_eq!(
        text(
            r#"std::encode_toml(#{
                title: "x",
                servers: [#{ name: "a", meta: #{ k: 1 } }, #{ name: "b" }],
                db: #{ port: 5432, opts: #{ ssl: true } },
                list: [1, "two", #{ three: 3 }, []],
                empty: [],
                "a key": 1,
                nan: 0.0 / 0.0,
                inf: -1.0 / 0.0,
                big: 1e300,
                none_here: #{},
            })"#
        ),
        r#""a key" = 1
big = 1e+300
empty = []
inf = -inf
list = [1, "two", { three = 3 }, []]
nan = nan
title = "x"

[db]
port = 5432

[db.opts]
ssl = true

[none_here]

[[servers]]
name = "a"

[servers.meta]
k = 1

[[servers]]
name = "b"
"#
    );
    assert_eq!(text("std::encode_toml(#{})"), "");
}

// spec: FMT#toml
#[test]
fn toml_strings_escape_what_toml_1_0_requires() {
    assert_eq!(
        text(r#"std::encode_toml(#{ s: "\"\\\u0008\t\n\u000c\r\u001b\u007fé" })"#),
        "s = \"\\\"\\\\\\b\\t\\n\\f\\r\\u001B\\u007Fé\"\n"
    );
}

// spec: FMT#toml
#[test]
fn toml_output_round_trips() {
    let src = r#"#{ a: #{ b: [#{ c: [1, 2.5, "x", true] }] }, "k.y": "v", d: [[1], [#{ e: 1 }]] }"#;
    let out = text(&format!("std::encode_toml({src})"));
    let back = json(&format!("std::decode_toml({})", lit(&out)));
    assert_eq!(back, json(src), "{out}");
}

// spec: FMT#env-files
#[test]
fn env_files_single_quote_every_value() {
    assert_eq!(
        text(
            r#"std::encode_env(#{ PORT: 80, DEBUG: false, RATIO: 1.0, NAME: "a b$c\"d", _X: "" })"#
        ),
        "DEBUG='false'\nNAME='a b$c\"d'\nPORT='80'\nRATIO='1.0'\n_X=''\n"
    );
    assert_eq!(text("std::encode_env(#{})"), "");
}

// spec: FMT#env-files
#[test]
fn env_files_refuse_what_cannot_be_single_quoted() {
    for value in [r#""it's""#, r#""a\nb""#, r#""a\rb""#, r#""a\u0000b""#] {
        let err = throws(&format!("std::encode_env(#{{ A: {value} }})"));
        assert!(err.contains(r#"`value["A"]` cannot be encoded"#), "{err}");
    }
    for key in ["1A", "A-B", "", "É"] {
        let err = throws(&format!("std::encode_env(#{{ {}: 1 }})", lit(key)));
        assert!(err.contains("an env file key"), "{err}");
    }
}

// spec: FMT#env-files
#[test]
fn env_files_decode_their_strict_form() {
    let decoded = json(&format!(
        "std::decode_env({})",
        lit("# comment\n\n  \t\nA=\nB='x y$z\"\\\\#'\nC=a-b.c/d:e@f%g+h,i=j\n_d1='it'")
    ));
    assert_eq!(
        decoded,
        serde_json::json!({ "A": "", "B": "x y$z\"\\\\#", "C": "a-b.c/d:e@f%g+h,i=j", "_d1": "it" })
    );
    for (bad, line) in [
        ("A=1\nB=\"x\"", 2),
        ("A=$HOME", 1),
        ("A=a\\b", 1),
        ("export A=1", 1),
        ("A='x' # c", 1),
        ("A = 1", 1),
        (" A=1", 1),
        ("A=1\r\n", 1),
        ("A='x\ry'", 1),
        ("A='x", 1),
        ("1A=x", 1),
        ("A=x y", 1),
    ] {
        let err = throws(&format!("std::decode_env({})", lit(bad)));
        assert!(
            err.contains(&format!("is not an env file at line {line}")),
            "{bad:?}: {err}"
        );
    }
    let err = throws(&format!("std::decode_env({})", lit("A=1\nA=2\n")));
    assert!(err.contains("names the same key twice, at line 2"), "{err}");
}

// spec: FMT#env-files
#[test]
fn env_files_round_trip() {
    let src = r#"#{ A: "x y $z \"q\" \\ #", B: "" }"#;
    let out = text(&format!("std::encode_env({src})"));
    assert_eq!(json(&format!("std::decode_env({})", lit(&out))), json(src));
}

// spec: FMT#decoding
#[test]
fn decoders_return_values_by_the_mapping_table() {
    assert_eq!(
        json(&format!(
            "std::decode_json({})",
            lit(r#"{"a": null, "b": [true, 1, -0, 1.0, 1e2, "s"], "c": {}}"#)
        )),
        serde_json::json!({ "a": null, "b": [true, 1, 0, 1.0, 100.0, "s"], "c": {} })
    );
    let v = eval(&format!("std::decode_json({})", lit("-0"))).unwrap();
    assert!(v.is_int(), "-0 has no fraction or exponent");
    assert_eq!(
        json(&format!(
            "std::decode_yaml({})",
            lit("a: ~\nb: [yes, true, 0x1F, 0o17, 010, 1., .5, 'true', \"1\"]\nc:\n")
        )),
        serde_json::json!({ "a": null, "b": ["yes", true, 31, 15, 10, 1.0, 0.5, "true", "1"], "c": null })
    );
    assert_eq!(
        eval(&format!("std::decode_yaml({})", lit("-.inf")))
            .unwrap()
            .as_float()
            .unwrap(),
        f64::NEG_INFINITY
    );
    assert!(
        eval(&format!("std::decode_yaml({})", lit(".nan")))
            .unwrap()
            .as_float()
            .unwrap()
            .is_nan()
    );
    assert_eq!(
        json(&format!(
            "std::decode_toml({})",
            lit(
                "a = 1979-05-27 07:32:00Z\nb = 1979-05-27\nc = 07:32:00\nd = 1979-05-27t07:32:00\ne = [1, 2.0, \"x\", { f = true }]"
            )
        )),
        serde_json::json!({
            "a": "1979-05-27T07:32:00Z",
            "b": "1979-05-27",
            "c": "07:32:00",
            "d": "1979-05-27T07:32:00",
            "e": [1, 2.0, "x", { "f": true }],
        })
    );
}

// spec: FMT#decoding
#[test]
fn decoders_refuse_numbers_out_of_range() {
    for (helper, doc) in [
        ("decode_json", "9223372036854775808"),
        ("decode_json", "-9223372036854775809"),
        ("decode_json", "[18446744073709551616]"),
        ("decode_json", "1e400"),
        ("decode_yaml", "9223372036854775808"),
        ("decode_yaml", "0x8000000000000000"),
        ("decode_yaml", "1e400"),
        ("decode_toml", "a = 9223372036854775808"),
        ("decode_toml", "a = 1e400"),
    ] {
        throws(&format!("std::{helper}({})", lit(doc)));
    }
    assert_eq!(
        json(&format!(
            "std::decode_json({})",
            lit("-9223372036854775808")
        )),
        serde_json::json!(i64::MIN)
    );
}

// spec: FMT#decoding
#[test]
fn decoders_refuse_a_key_given_twice() {
    for (helper, doc) in [
        ("decode_json", r#"{"a": 1, "\u0061": 2}"#),
        ("decode_yaml", "a: 1\n\"a\": 2"),
        ("decode_toml", "a = 1\na = 2"),
    ] {
        let err = throws(&format!("std::{helper}({})", lit(doc)));
        assert!(err.contains("line"), "{helper}: {err}");
    }
}

// spec: FMT#decoding
#[test]
fn a_decoding_failure_names_the_line_but_not_the_text() {
    for (helper, doc, line) in [
        ("decode_json", "{\n\"a\": s3cr3t\n}", 2),
        ("decode_yaml", "a: 1\nb: [s3cr3t\n", 3),
        ("decode_yaml", "a: 1\n\"s3cr3t", 2),
        ("decode_toml", "a = 1\nb = s3cr3t", 2),
        ("decode_env", "A=1\nB=s3cr3t x", 2),
    ] {
        let err = throws(&format!("std::{helper}({})", lit(doc)));
        assert!(err.contains(&format!("at line {line}")), "{helper}: {err}");
        assert!(!err.contains("s3cr3t"), "{helper}: {err}");
    }
    let err = throws("std::decode_json(blob(1, 0xff))");
    assert!(err.contains("not valid UTF-8 at line 1"), "{err}");
}

// spec: FMT#decoding
#[test]
fn decoders_ignore_a_byte_order_mark() {
    for (helper, doc) in [
        ("decode_json", "{}"),
        ("decode_yaml", "{}"),
        ("decode_toml", ""),
        ("decode_env", ""),
    ] {
        let v = eval(&format!(
            "std::{helper}({})",
            lit(&format!("\u{feff}{doc}"))
        ))
        .unwrap();
        assert!(v.is_map(), "{helper}");
    }
}

// spec: FMT#yaml-documents
#[test]
fn yaml_reads_exactly_one_document() {
    for doc in ["", "# only a comment\n", "a: 1\n---\nb: 2\n"] {
        throws(&format!("std::decode_yaml({})", lit(doc)));
    }
    assert_eq!(
        json(&format!("std::decode_yaml({})", lit("---\na: 1\n...\n"))),
        serde_json::json!({ "a": 1 })
    );
}

// spec: FMT#yaml-documents
#[test]
fn yaml_tags_must_be_core_and_fit_their_node() {
    assert_eq!(
        json(&format!(
            "std::decode_yaml({})",
            lit("[!!str 1, !!int \"2\", !!float 3, !!bool true, !!null '', !!seq [], !!map {}]")
        )),
        serde_json::json!(["1", 2, 3.0, true, null, [], {}])
    );
    for doc in [
        "!foo x",
        "! x",
        "!!binary aGk=",
        "!!int x",
        "!!float 0x1F",
        "!!map []",
        "!!seq {}",
        "!!str [1]",
    ] {
        let err = throws(&format!("std::decode_yaml({})", lit(doc)));
        assert!(err.contains("tag"), "{doc}: {err}");
    }
}

// spec: FMT#yaml-documents
#[test]
fn yaml_keys_must_be_strings() {
    for doc in ["1: a", "true: a", "~: a", "[a]: b", "? {a: 1}\n: b"] {
        let err = throws(&format!("std::decode_yaml({})", lit(doc)));
        assert!(err.contains("not a string"), "{doc}: {err}");
    }
}

// spec: FMT#yaml-documents
#[test]
fn yaml_aliases_copy_their_anchor_within_a_budget() {
    assert_eq!(
        json(&format!(
            "std::decode_yaml({})",
            lit("a: &x [1, 2]\nb: *x\n")
        )),
        serde_json::json!({ "a": [1, 2], "b": [1, 2] })
    );
    // Each level doubles the copies: 10 levels of 10 copies is far past the budget.
    let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..10 {
        let p = i - 1;
        bomb.push_str(&format!(
            "a{i}: &a{i} [*a{p}, *a{p}, *a{p}, *a{p}, *a{p}, *a{p}, *a{p}, *a{p}, *a{p}, *a{p}]\n"
        ));
    }
    let err = throws(&format!("std::decode_yaml({})", lit(&bomb)));
    assert!(err.contains("more than 100000 values"), "{err}");
    throws(&format!("std::decode_yaml({})", lit("a: *x")));
    throws(&format!("std::decode_yaml({})", lit("a: &x [*x]")));
}

// spec: FMT#helpers
#[test]
fn decoders_take_a_bytes_argument() {
    assert_eq!(
        json("std::decode_json(std::encode_utf8(\"[1]\"))"),
        serde_json::json!([1])
    );
    let err = throws("std::decode_json(1)");
    assert!(
        err.contains("`text` must be a string, Blob, File or Directory"),
        "{err}"
    );
}

// spec: FMT#helpers
// spec: LIM#limits
#[test]
fn results_are_held_to_the_engine_limits() {
    let mut engine = engine();
    engine.set_max_string_size(24);
    engine.set_max_array_size(3);
    engine.set_max_map_size(2);
    let err = engine
        .eval::<Dynamic>(r#"std::encode_json(["aaaaaaaaaa", "bbbbbbbbbb"])"#)
        .expect_err("text too long")
        .to_string();
    assert!(err.contains("Length of string"), "{err}");
    for doc in ["[1, 2, 3, 4]", r#"{"a": 1, "b": 2, "c": 3}"#] {
        let err = engine
            .eval::<Dynamic>(&format!("std::decode_yaml({})", lit(doc)))
            .expect_err(doc)
            .to_string();
        assert!(err.contains("too large"), "{doc}: {err}");
    }
}

// spec: FMT#json
#[test]
fn rhais_to_json_points_at_encode_json() {
    let mut engine = engine();
    crate::defs::register(&mut engine);
    for src in [
        "#{ a: 1 }.to_json()",
        "to_json(#{ a: 1 })",
        r#"Fn("to_json").call(#{ a: 1 })"#,
    ] {
        let err = engine.eval::<Dynamic>(src).expect_err(src).to_string();
        assert!(err.contains("std::encode_json"), "{src}: {err}");
    }
}
