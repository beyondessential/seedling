use super::*;

/// Evaluate `source` in the static context under `limits`, compiled as a
/// definition is.
fn eval_with(
    limits: &crate::ScriptLimits,
    source: &str,
) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(limits);
    let _seed =
        crate::runtime::seeds::AppSeedFrame::enter(&crate::runtime::seeds::AppSeed::for_tests());
    let ast = crate::defs::compile(&engine, source)?;
    engine.eval_ast_with_scope(&mut scope, &ast)
}

fn eval(source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    eval_with(&crate::ScriptLimits::default(), source)
}

/// What `source` evaluates to, as BSL prints it.
fn shown(source: &str) -> String {
    let value = eval(source).unwrap_or_else(|e| panic!("{source} should run, got {e}"));
    format!("{value:?}")
}

/// The error `source` fails with.
fn failure(source: &str) -> String {
    eval(source).expect_err(source).to_string()
}

fn truth(source: &str) -> bool {
    eval(source)
        .unwrap_or_else(|e| panic!("{source} should run, got {e}"))
        .as_bool()
        .expect("a boolean")
}

// spec: REX#compiling-a-pattern
#[test]
fn a_malformed_pattern_throws() {
    let err = failure(r#"let p = "("; std::regex(p)"#);
    assert!(err.contains("not a valid regular expression"), "{err}");
    assert!(
        truth(
            r#"let p = "("; let caught = false; try { std::regex(p); } catch { caught = true; } caught"#
        ),
        "the error is a script error a script can catch"
    );
}

// spec: REX#compiling-a-pattern
#[test]
fn a_pattern_that_is_not_a_string_throws_naming_it() {
    let err = failure("std::regex(42)");
    assert!(err.contains("`pattern` must be a string"), "{err}");
}

// spec: REX#compiling-a-pattern
#[test]
fn a_regex_converts_to_its_pattern() {
    assert_eq!(
        eval(r#"`${std::regex("a+(b)")}`"#).unwrap().to_string(),
        "a+(b)"
    );
}

// spec: REX#compiling-a-pattern
#[test]
fn a_malformed_literal_pattern_fails_evaluation_even_where_it_would_not_run() {
    for source in [
        r#"fn never() { std::regex("(") } 1"#,
        r#"if false { std::regex("[") } 1"#,
        r#"if false { std::parse_regex("(") } 1"#,
        "if false { std::regex(`(`) } 1",
    ] {
        let err = failure(source);
        assert!(
            err.contains("not a valid regular expression"),
            "{source}: {err}"
        );
    }
}

// spec: REX#compiling-a-pattern
#[test]
fn a_pattern_not_written_as_a_literal_is_checked_only_when_it_runs() {
    for source in [
        r#"const P = "("; if false { std::regex(P) } 1"#,
        r#"let x = "("; if false { std::regex(`${x}`) } 1"#,
    ] {
        assert_eq!(eval(source).expect(source).as_int().unwrap(), 1, "{source}");
    }
}

// spec: REX#compiling-a-pattern
#[test]
fn a_literal_too_large_for_the_whole_limit_fails_evaluation() {
    let limits = crate::ScriptLimits {
        max_regex_compiled_size: 100_000,
        ..crate::ScriptLimits::default()
    };
    let err = eval_with(&limits, r#"if false { std::regex("\\w{1000}") } 1"#)
        .expect_err("too large")
        .to_string();
    assert!(err.contains("compiled size limit"), "{err}");
}

// spec: REX#compiled-size
#[test]
fn a_refusal_with_much_of_the_limit_left_still_uses_it_all() {
    let limits = crate::ScriptLimits {
        max_regex_compiled_size: 1_000_000,
        ..crate::ScriptLimits::default()
    };
    let source = r#"
        let p = "\\w{1000}";
        try { std::regex(p); } catch {}
        let small = true;
        try { std::regex("a"); } catch { small = false; }
        small
    "#;
    assert!(
        !eval_with(&limits, source).unwrap().as_bool().unwrap(),
        "a tiny pattern is refused after an enormous one"
    );
}

// spec: REX#compiled-size
#[test]
fn compiling_counts_toward_the_evaluation_and_a_refusal_uses_the_rest() {
    let limits = crate::ScriptLimits {
        max_regex_compiled_size: 400_000,
        ..crate::ScriptLimits::default()
    };
    // Each compiles well within the limit, but not many times over.
    let source = r#"
        let n = 0;
        let refused = false;
        while !refused && n < 1000 {
            try { std::regex(`[a-z]{${n + 50}}`); n += 1; } catch { refused = true; }
        }
        let small = true;
        try { std::regex("a"); } catch { small = false; }
        [n, small]
    "#;
    let result = eval_with(&limits, source)
        .expect("runs")
        .into_typed_array::<Dynamic>()
        .unwrap();
    let compiled = result[0].as_int().unwrap();
    assert!(compiled > 0 && compiled < 1000, "{compiled}");
    assert!(
        !result[1].as_bool().unwrap(),
        "after a refusal even a tiny pattern is refused"
    );
}

// spec: REX#compiled-size
#[test]
fn the_same_pattern_counts_each_time_it_is_compiled() {
    let limits = crate::ScriptLimits {
        max_regex_compiled_size: 400_000,
        ..crate::ScriptLimits::default()
    };
    let source = r#"
        let n = 0;
        while n < 1000 {
            try { std::regex("[a-z]{100}"); n += 1; } catch { break; }
        }
        n
    "#;
    let first = eval_with(&limits, source).unwrap().as_int().unwrap();
    let second = eval_with(&limits, source).unwrap().as_int().unwrap();
    assert!(first < 1000, "{first}");
    assert_eq!(
        first, second,
        "a cached pattern counts what it did uncached"
    );
}

// spec: REX#compiled-size
#[test]
fn a_compiled_size_limit_of_zero_is_no_limit() {
    let limits = crate::ScriptLimits {
        max_regex_compiled_size: 0,
        ..crate::ScriptLimits::default()
    };
    let n = eval_with(
        &limits,
        r#"let n = 0; while n < 200 { std::regex("[a-z]{500}"); n += 1; } n"#,
    )
    .expect("no limit");
    assert_eq!(n.as_int().unwrap(), 200);
}

// spec: REX#searching
#[test]
fn a_search_finds_the_leftmost_first_match_anywhere() {
    assert!(truth(r#"std::regex("b+").is_match("aabba")"#));
    assert!(!truth(r#"std::regex("^b+$").is_match("aabba")"#));
    assert!(!truth(r#"std::regex("^a$").is_match("a\n")"#));
    assert_eq!(shown(r#"std::regex("a|ab").find("xab").text"#), r#""a""#);
}

// spec: REX#searching
#[test]
fn an_empty_match_touching_the_previous_match_is_skipped() {
    assert_eq!(
        shown(r#"std::regex("b*").find_all("abba").map(|m| [m.start, m.end])"#),
        "[[0, 0], [1, 3], [4, 4]]"
    );
    assert_eq!(
        shown(r#"std::regex("|a").find_all("a").map(|m| [m.start, m.end])"#),
        "[[0, 0], [1, 1]]"
    );
}

// spec: REX#positions
#[test]
fn positions_count_characters() {
    assert_eq!(
        shown(r#"std::regex("b").find_all("ébéb").map(|m| [m.start, m.end])"#),
        "[[1, 2], [3, 4]]"
    );
    assert_eq!(
        shown(
            r#"let t = "ééxé"; let m = std::regex("x").find(t); t.sub_string(m.start, m.end - m.start)"#
        ),
        r#""x""#
    );
    assert_eq!(
        shown(r#"std::regex("").find_all("é").map(|m| m.start)"#),
        "[0, 1]"
    );
}

// spec: REX#matches
#[test]
fn a_match_is_its_text_and_positions() {
    assert_eq!(
        shown(r#"let m = std::regex("b+").find("abbc"); [m.text, m.start, m.end]"#),
        r#"["bb", 1, 3]"#
    );
    assert_eq!(shown(r#"std::regex("z").find("abc")"#), "()");
}

// spec: REX#captures
#[test]
fn captures_list_every_group_and_name_the_named_ones() {
    let source = r#"
        let c = std::regex("(?<key>\\w+)=(\\d+)?(x)?").captures("é a=");
        [c.groups.len(), c.groups[0].text, c.groups[1].start, c.groups[2], c.named.key.text]
    "#;
    assert_eq!(shown(source), r#"[4, "a=", 2, (), "a"]"#);
    assert_eq!(shown(r#"std::regex("z").captures("abc")"#), "()");
}

// spec: REX#methods
#[test]
fn every_match_methods_return_each_in_order() {
    assert_eq!(
        shown(r#"std::regex("\\d").find_all("a1b2").map(|m| m.text)"#),
        r#"["1", "2"]"#
    );
    assert_eq!(
        shown(r#"std::regex("(\\w)(\\d)").captures_all("a1 b2").map(|c| c.groups[1].text)"#),
        r#"["a", "b"]"#
    );
}

// spec: REX#methods
#[test]
fn split_keeps_empty_pieces_and_honours_a_limit() {
    assert_eq!(
        shown(r#"std::regex(",").split(",a,,b,")"#),
        r#"["", "a", "", "b", ""]"#
    );
    assert_eq!(
        shown(r#"std::regex(",").split("a,b,c", 2)"#),
        r#"["a", "b,c"]"#
    );
    assert_eq!(shown(r#"std::regex(",").split("a,b", 1)"#), r#"["a,b"]"#);
    let err = failure(r#"std::regex(",").split("a,b", 0)"#);
    assert!(err.contains("`limit` must be at least 1"), "{err}");
}

// spec: REX#methods
#[test]
fn a_text_that_is_not_a_string_throws_naming_it() {
    let err = failure(r#"std::regex("a").find_all(1)"#);
    assert!(err.contains("`text` must be a string"), "{err}");
}

// spec: REX#replacements
#[test]
fn a_replacement_string_expands_group_references() {
    // Pattern, text and replacement as BSL source, then what `replace` and
    // `replace_all` give.
    let cases = [
        (
            r#""(\\w)(\\d)""#,
            r#""a1 b2""#,
            r#""$2$1""#,
            "1a b2",
            "1a 2b",
        ),
        (r#""(?<l>\\w)\\d""#, r#""a1""#, r#""<${l}>""#, "<a>", "<a>"),
        (r#""(?<l>\\w)\\d""#, r#""a1""#, r#""<$l>""#, "<a>", "<a>"),
        (r#""\\d""#, r#""a1""#, r#""$$""#, "a$", "a$"),
        (r#""\\d""#, r#""a1""#, r#""[$0]""#, "a[1]", "a[1]"),
        (r#""(\\d)""#, r#""a1""#, r#""${1}x""#, "a1x", "a1x"),
        (r#""(x)?\\d""#, r#""a1""#, r#""[$1]""#, "a[]", "a[]"),
    ];
    for (pattern, text, replacement, once, all) in cases {
        for (method, expected) in [("replace", once), ("replace_all", all)] {
            let source = format!("std::regex({pattern}).{method}({text}, {replacement})");
            assert_eq!(eval(&source).unwrap().to_string(), expected, "{source}");
        }
    }
}

// spec: REX#replacements
#[test]
fn a_malformed_replacement_string_throws_even_without_a_match() {
    for replacement in [
        r#""$""#,
        r#""100$""#,
        r#""a$ b""#,
        r#""${1""#,
        r#""${oops""#,
        r#""$2""#,
        r#""$1a""#,
        r#""${nope}""#,
    ] {
        let source = format!(r#"std::regex("(\\d)").replace_all("no digits", {replacement})"#);
        let err = failure(&source);
        assert!(err.contains("`replacement`"), "{source}: {err}");
    }
}

// spec: REX#replacements
#[test]
fn a_replacement_closure_receives_each_match_and_is_inserted_unexpanded() {
    assert_eq!(
        eval(r#"std::regex("\\d").replace_all("a1b2", |c| `<${c.groups[0].start}$1>`)"#)
            .unwrap()
            .to_string(),
        "a<1$1>b<3$1>"
    );
    let err = failure(r#"std::regex("\\d").replace("a1", |c| 5)"#);
    assert!(err.contains("must return a string"), "{err}");
    let err = failure(r#"std::regex("\\d").replace("a1", 5)"#);
    assert!(err.contains("must be a string or a closure"), "{err}");
}

// spec: ACN#kind-parsers
#[test]
fn parse_regex_applies_the_single_line_rule_and_returns_a_regex() {
    assert!(truth(r#"std::parse_regex("a+").is_match("baa")"#));
    let err = failure(r#"let p = "a\nb"; std::parse_regex(p)"#);
    assert!(!err.contains("not a valid regular expression"), "{err}");
}

// spec: LIM#limits
#[test]
fn every_match_results_are_held_to_the_array_limit() {
    let limits = crate::ScriptLimits {
        max_array_size: 100,
        ..crate::ScriptLimits::default()
    };
    for method in ["find_all", "captures_all", "split"] {
        let source = format!(r#"let t = ""; t.pad(500, "a"); std::regex("a").{method}(t)"#);
        let err = eval_with(&limits, &source).expect_err(method);
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorDataTooLarge(..)),
            "{method}: {err:?}"
        );
    }
}

// spec: LIM#limits
#[test]
fn a_replacement_is_held_to_the_string_limit() {
    let limits = crate::ScriptLimits {
        max_string_size: 1_000,
        ..crate::ScriptLimits::default()
    };
    let source = r#"let t = ""; t.pad(500, "a"); std::regex("a").replace_all(t, "bbbbbbbbbb")"#;
    let err = eval_with(&limits, source).expect_err("too long");
    assert!(
        matches!(*err, rhai::EvalAltResult::ErrorDataTooLarge(..)),
        "{err:?}"
    );
}
