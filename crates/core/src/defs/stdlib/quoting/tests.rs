use std::{path::Path, process::Command};

use rhai::{Dynamic, Engine, EvalAltResult};

use super::super::{HelperLimits, module};

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_static_module("std", module().into());
    engine.set_default_tag(Dynamic::from(HelperLimits {
        derived_values: 0,
        password_hashes: 0,
        regex_compiled_size: 0,
    }));
    engine
}

fn eval(src: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    engine().eval::<Dynamic>(src)
}

fn quote(word: &str) -> String {
    eval(&format!("std::quote_shell({word:?})"))
        .unwrap()
        .into_string()
        .unwrap()
}

fn throws(src: &str) -> String {
    eval(src).expect_err(src).to_string()
}

// spec: QUOT#shell-words
#[test]
fn a_word_of_safe_characters_stands_unquoted() {
    for word in [
        "abcXYZ019",
        "_",
        "-",
        ".",
        "/",
        ":",
        ",",
        "+",
        "@",
        "%",
        "/usr/local/bin",
        "user@host:2222",
        "v1.2.3-rc.1+build",
        "50%",
    ] {
        assert_eq!(quote(word), word);
    }
}

// spec: QUOT#shell-words
#[test]
fn any_other_character_quotes_the_word() {
    for word in [
        "a=b", "~", "~user", "a b", "*", "$HOME", "`id`", "\"", "\\", "#", ";", "!", "{a,b}",
        "a\nb", "\t", "ā", "(", "|", "&", "<", "?", "[",
    ] {
        assert_eq!(quote(word), format!("'{word}'"));
    }
}

// spec: QUOT#shell-words
#[test]
fn a_single_quote_is_written_outside_the_quotes() {
    assert_eq!(quote(""), "''");
    assert_eq!(quote("it's"), r"'it'\''s'");
    assert_eq!(quote("'"), r"''\'''");
}

// spec: QUOT#shell-words
#[test]
fn an_array_gives_one_word_per_element() {
    let quoted = |src: &str| eval(src).unwrap().into_string().unwrap();
    assert_eq!(
        quoted(r#"std::quote_shell(["ls", "-l", "my file"])"#),
        "ls -l 'my file'"
    );
    assert_eq!(quoted(r#"std::quote_shell(["", ""])"#), "'' ''");
    assert_eq!(quoted("std::quote_shell([])"), "");
}

// spec: QUOT#shell-words
#[test]
fn an_argument_of_another_type_throws() {
    let err = throws(r#"std::quote_shell(["a", 1])"#);
    assert!(err.contains("element 1 is a i64"), "{err}");
    let err = throws("std::quote_shell(1)");
    assert!(
        err.contains("`words` must be a string or an array of strings, got i64"),
        "{err}"
    );
}

// spec: QUOT#shell-words
// spec: SEC#redaction
#[test]
fn a_nul_character_throws_without_the_word() {
    for src in [
        r#"std::quote_shell("s3cr3t\x00")"#,
        r#"std::quote_shell(["ok", "s3cr3t\x00"])"#,
    ] {
        let err = throws(src);
        assert!(err.contains("NUL"), "{err}");
        assert!(!err.contains("s3cr3t"), "{err}");
    }
}

// spec: QUOT#results
#[test]
fn a_result_beyond_the_string_limit_fails() {
    let mut engine = engine();
    engine.set_max_string_size(6);
    let err = engine
        .eval::<Dynamic>(r#"std::quote_shell(["ab", "cd", "e"])"#)
        .unwrap_err();
    assert!(
        matches!(*err, EvalAltResult::ErrorDataTooLarge(..)),
        "{err}"
    );
}

// spec: QUOT#shell-words
#[test]
fn the_shell_reads_back_each_word() {
    let sh = Path::new("/bin/sh");
    if !sh.exists() {
        return;
    }
    let words = [
        "", " ", "a b", "it's", "'", "''", "\\", "\\'", "$HOME", "${x:-y}", "`id`", "$(id)", "*",
        "~", "~root", "a=b", "-n", "--", "\n", "a\nb", "\t", "ā€", "%s", "\"", "!", "!!", "{a,b}",
        "#", ";", "&&", "|", "<", ">", "(", ")", "[a]", "?", "plain", "/usr/bin",
    ];
    let array = words
        .iter()
        .map(|w| format!("{w:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let quoted = eval(&format!("std::quote_shell([{array}])"))
        .unwrap()
        .into_string()
        .unwrap();
    let output = Command::new(sh)
        .arg("-c")
        .arg(format!(r"printf '%s\0' {quoted}"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let read: Vec<&[u8]> = output.stdout.split(|&b| b == 0).collect();
    let expected: Vec<&[u8]> = words
        .iter()
        .map(|w| w.as_bytes())
        .chain([&b""[..]])
        .collect();
    assert_eq!(read, expected);
}
