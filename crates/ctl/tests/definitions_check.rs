//! `definitions check`, run as the built binary: host constants are set once
//! per process, so each check needs a process of its own.

use std::{path::Path, process::Command};

use serde_json::{Value, json};

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
}

fn check(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_seedling-ctl"))
        .current_dir(dir)
        .args(["definitions", "check"])
        .args(args)
        .output()
        .expect("seedling-ctl runs");
    Run {
        status: out.status.code().expect("seedling-ctl exits"),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn json_errors(run: &Run) -> Vec<Value> {
    let v: Value = serde_json::from_str(&run.stdout).expect("stdout is JSON");
    v["errors"].as_array().expect("errors is an array").clone()
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        write(dir.path(), path, contents);
    }
    dir
}

// spec: CLI#definition-checks
#[test]
fn a_definition_that_evaluates_passes_silently() {
    let dir = dir_with(&[("web.seed.rhai", "app.description(\"web\");\n")]);
    let run = check(dir.path(), &["web.seed.rhai"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert_eq!(run.stderr, "");
}

// spec: CLI#check-results
#[test]
fn a_passing_check_prints_an_empty_list_as_json() {
    let dir = dir_with(&[("web.seed.rhai", "app.description(\"web\");\n")]);
    let run = check(dir.path(), &["web.seed.rhai", "--json"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&run.stdout).unwrap(),
        json!({ "errors": [] })
    );
}

// spec: CLI#check-results
#[test]
fn a_script_error_in_a_file_source_lies_at_the_path_given() {
    let dir = dir_with(&[(
        "apps/broken.seed.rhai",
        "let a = 1;\nlet b = 2;\nthrow \"broken\";\n",
    )]);
    let run = check(dir.path(), &["apps/broken.seed.rhai"]);
    assert_eq!(run.status, 1);
    assert!(
        run.stderr.starts_with("apps/broken.seed.rhai:3: "),
        "{}",
        run.stderr
    );
}

// spec: CLI#check-results
#[test]
fn a_script_error_as_json_names_its_kind_file_and_line() {
    let dir = dir_with(&[("broken.seed.rhai", "\n\nthrow \"broken\";\n")]);
    let run = check(dir.path(), &["broken.seed.rhai", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "script");
    assert_eq!(errors[0]["file"], "broken.seed.rhai");
    assert_eq!(errors[0]["line"], 3);
    assert_eq!(errors[0]["parameter"], Value::Null);
    assert!(errors[0]["message"].as_str().unwrap().contains("broken"));
}

// spec: CLI#check-results
// spec: BND#script-formation
#[test]
fn a_script_error_in_a_folder_lies_at_its_own_file_and_line() {
    let dir = dir_with(&[
        (
            "def/seedling.toml",
            "script = [\"first.rhai\", \"lib/db.rhai\"]\n",
        ),
        ("def/first.rhai", "let a = 1;\nlet b = 2;\n"),
        ("def/lib/db.rhai", "let c = 3;\n\nthrow \"broken\";\n"),
    ]);
    let run = check(dir.path(), &["def"]);
    assert_eq!(run.status, 1);
    let expected = Path::new("def").join("lib").join("db.rhai");
    assert!(
        run.stderr
            .starts_with(&format!("{}:3: ", expected.display())),
        "{}",
        run.stderr
    );
}

// spec: CLI#check-results
#[test]
fn a_script_error_in_an_included_file_lies_at_its_source() {
    let dir = dir_with(&[
        (
            "repo/apps/def/seedling.toml",
            "script = [\"app.seed.rhai\", \"lib/common.rhai\"]\n\n[include]\n\"lib/common.rhai\" = \"../shared/common.rhai\"\n",
        ),
        ("repo/apps/def/app.seed.rhai", "let a = 1;\n"),
        (
            "repo/apps/shared/common.rhai",
            "let b = 2;\nthrow \"broken\";\n",
        ),
    ]);
    std::fs::create_dir(dir.path().join("repo/.git")).unwrap();
    let run = check(dir.path(), &["repo/apps/def"]);
    assert_eq!(run.status, 1);
    let expected = Path::new("repo/apps").join("shared").join("common.rhai");
    assert!(
        run.stderr
            .starts_with(&format!("{}:2: ", expected.display())),
        "{}",
        run.stderr
    );
}

// spec: CLI#check-results
#[test]
#[cfg(unix)]
fn an_included_file_through_a_symlinked_folder_lies_at_its_source() {
    let dir = dir_with(&[
        (
            "repo/apps/def/seedling.toml",
            "script = [\"app.seed.rhai\", \"lib/common.rhai\"]\n\n[include]\n\"lib/common.rhai\" = \"../shared/common.rhai\"\n",
        ),
        ("repo/apps/def/app.seed.rhai", "let a = 1;\n"),
        (
            "repo/apps/shared/common.rhai",
            "let b = 2;\nthrow \"broken\";\n",
        ),
    ]);
    std::fs::create_dir(dir.path().join("repo/.git")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("repo/apps/def"), dir.path().join("link")).unwrap();
    let run = check(dir.path(), &["link"]);
    assert_eq!(run.status, 1);
    assert!(
        run.stderr.starts_with("repo/apps/shared/common.rhai:2: "),
        "{}",
        run.stderr
    );

    let inner = dir.path().join("repo/apps/def");
    let run = check(&inner, &["."]);
    assert!(
        run.stderr.starts_with("../shared/common.rhai:2: "),
        "{}",
        run.stderr
    );
}

// spec: CLI#check-results
#[test]
fn an_error_raised_in_a_function_lies_where_it_was_raised() {
    let mut script = String::from("fn fail() {\n    throw \"broken\";\n}\n");
    script.push_str(&"\n".repeat(36));
    script.push_str("fail();\n");
    let dir = dir_with(&[("f.seed.rhai", &script)]);
    let run = check(dir.path(), &["f.seed.rhai"]);
    assert_eq!(run.status, 1);
    assert!(run.stderr.starts_with("f.seed.rhai:2: "), "{}", run.stderr);
}

// spec: CLI#definition-checks
// spec: CLI#check-results
#[test]
fn the_version_is_checked_before_any_other_field() {
    let dir = dir_with(&[
        ("def/seedling.toml", "seedling = \">=999\"\nunknown = 1\n"),
        ("def/app.seed.rhai", ""),
    ]);
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "version");
    let message = errors[0]["message"].as_str().unwrap();
    assert!(message.contains(">=999"), "{message}");
    assert!(message.contains(env!("CARGO_PKG_VERSION")), "{message}");
    let expected = Path::new("def").join("seedling.toml");
    assert_eq!(errors[0]["file"], expected.display().to_string());
}

// spec: CLI#definition-checks
#[test]
fn a_bundle_naming_a_missing_script_file_is_not_evaluated() {
    let dir = dir_with(&[
        ("def/seedling.toml", "script = [\"missing.rhai\"]\n"),
        ("def/app.seed.rhai", "throw \"evaluated\";\n"),
    ]);
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "bundle");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("missing.rhai")
    );
}

// spec: CLI#include-assembly
// spec: CLI#check-results
#[test]
fn a_folder_past_the_file_limit_is_a_bundle_error() {
    let dir = dir_with(&[("def/app.seed.rhai", "throw \"evaluated\";\n")]);
    for i in 0..seedling_bundle::BUNDLE_FILE_LIMIT {
        write(dir.path(), &format!("def/many/{i}"), "");
    }
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "bundle", "{errors:?}");
}

// spec: CLI#check-results
#[test]
fn an_include_naming_nothing_is_a_source_error_in_the_metadata_file() {
    let dir = dir_with(&[
        ("def/seedling.toml", "[include]\n\"lib\" = \"../nowhere\"\n"),
        ("def/app.seed.rhai", ""),
    ]);
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "source");
    assert!(errors[0]["message"].as_str().unwrap().contains("\"lib\""));
    let expected = Path::new("def").join("seedling.toml");
    assert_eq!(errors[0]["file"], expected.display().to_string());
}

// spec: CLI#definition-sources
#[test]
fn a_lone_metadata_file_is_a_source_error() {
    let dir = dir_with(&[("seedling.toml", "")]);
    let run = check(dir.path(), &["seedling.toml", "--json"]);
    assert_eq!(run.status, 1);
    assert_eq!(json_errors(&run)[0]["kind"], "source");
}

// spec: CLI#sample-parameter-values
#[test]
fn a_script_branching_on_a_sample_value_fails_only_with_it() {
    let dir = dir_with(&[(
        "m.seed.rhai",
        "let mode = app.param(\"mode\").required(false);\nif mode.is_set() && mode.value() == \"cluster\" { throw \"no clusters\"; }\n",
    )]);
    assert_eq!(check(dir.path(), &["m.seed.rhai"]).status, 0);
    let run = check(dir.path(), &["m.seed.rhai", "--set", "mode=cluster"]);
    assert_eq!(run.status, 1);
    assert!(run.stderr.contains("no clusters"), "{}", run.stderr);
}

fn rejections(script: &str, sets: &[&str]) -> Vec<Value> {
    let dir = dir_with(&[("v.seed.rhai", script)]);
    let mut args = vec!["v.seed.rhai", "--json"];
    for set in sets {
        args.extend(["--set", set]);
    }
    let run = check(dir.path(), &args);
    assert_eq!(run.status, 1, "{}", run.stderr);
    let errors = json_errors(&run);
    for e in &errors {
        assert_eq!(e["kind"], "validation", "{e}");
        assert_eq!(e["file"], Value::Null, "{e}");
        assert_eq!(e["line"], Value::Null, "{e}");
    }
    errors
}

fn parameters(errors: &[Value]) -> Vec<&str> {
    errors
        .iter()
        .map(|e| e["parameter"].as_str().unwrap())
        .collect()
}

// spec: CLI#sample-parameter-values
// spec: CLI#check-results
#[test]
fn each_value_its_validator_rejects_is_a_validation_error() {
    let errors = rejections(
        "app.param(\"alpha\").required(false).validate(|v, all| { throw \"too short\"; });\napp.param(\"bravo\").required(false).validate(|v, all| { throw \"too long\"; });\n",
        &["alpha=x", "bravo=y"],
    );
    assert_eq!(parameters(&errors), ["alpha", "bravo"]);
    assert!(errors[0]["message"].as_str().unwrap().contains("too short"));
}

// spec: CLI#sample-parameter-values
// spec: PRM#validation
#[test]
fn a_value_its_kind_rejects_runs_no_validator() {
    let errors = rejections(
        "app.param(\"port\").kind(\"integer\").required(false).validate(|v, all| { throw \"validator ran\"; });\n",
        &["port=abc"],
    );
    assert_eq!(parameters(&errors), ["port"]);
    assert!(
        !errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("validator ran")
    );
}

// spec: CLI#sample-parameter-values
#[test]
fn a_value_for_an_undeclared_parameter_is_rejected() {
    let errors = rejections(
        "app.param(\"database-url\").required(false);\n",
        &["dtaabase-url=x"],
    );
    assert_eq!(parameters(&errors), ["dtaabase-url"]);
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("not declared")
    );
}

// spec: CLI#sample-parameter-values
#[test]
fn a_sample_value_is_split_at_the_first_equals() {
    let dir = dir_with(&[(
        "u.seed.rhai",
        "let url = app.param(\"url\");\nif url.value() != \"a=b\" { throw url.value(); }\n",
    )]);
    let run = check(dir.path(), &["u.seed.rhai", "--set", "url=a=b"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#sample-parameter-values
// spec: CLI#check-results
#[test]
fn a_bad_sample_value_option_checks_nothing() {
    let dir = dir_with(&[("x.seed.rhai", "throw \"evaluated\";\n")]);
    for args in [
        &["--set", "mode"][..],
        &["--set", "a=1", "--set", "a=2"][..],
    ] {
        let mut all = vec!["x.seed.rhai", "--json"];
        all.extend_from_slice(args);
        let run = check(dir.path(), &all);
        assert_eq!(run.status, 2, "{args:?}");
        assert_eq!(run.stdout, "", "{args:?}");
        assert!(!run.stderr.is_empty(), "{args:?}");
        assert!(!run.stderr.contains("evaluated"), "{args:?}");
    }
}

// spec: CLI#check-host-constants
#[test]
fn host_constants_take_their_check_defaults() {
    let dir = dir_with(&[(
        "c.seed.rhai",
        "if TIMEZONE != \"UTC\" || NODE_NAME != \"\" || HOST_HAS_IPV4 || HOST_HAS_IPV6 || NAT64_ACTIVE || HAS_SNAPSHOTS { throw \"wrong defaults\"; }\nif AVAILABLE_THREADS < 1 || CPU_ARCHITECTURE == \"\" { throw \"no machine values\"; }\n",
    )]);
    let run = check(dir.path(), &["c.seed.rhai"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#check-host-constants
#[test]
fn host_constants_take_the_values_given() {
    let dir = dir_with(&[(
        "c.seed.rhai",
        "if CPU_ARCHITECTURE != \"aarch64\" || AVAILABLE_MEMORY != 2147483648 || !HOST_HAS_IPV6 || TIMEZONE != \"Pacific/Auckland\" || NODE_NAME != \"n1\" || AVAILABLE_THREADS != 3 { throw \"not set\"; }\n",
    )]);
    let run = check(
        dir.path(),
        &[
            "c.seed.rhai",
            "--const",
            "AVAILABLE_MEMORY=2147483648",
            "--const",
            "CPU_ARCHITECTURE=aarch64",
            "--const",
            "HOST_HAS_IPV6=true",
            "--const",
            "TIMEZONE=Pacific/Auckland",
            "--const",
            "NODE_NAME=n1",
            "--const",
            "AVAILABLE_THREADS=3",
        ],
    );
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#check-host-constants
#[test]
fn a_bad_host_constant_checks_nothing() {
    let dir = dir_with(&[("x.seed.rhai", "throw \"evaluated\";\n")]);
    for arg in [
        "AVAILABLE_THREADS=0",
        "AVAILABLE_MEMORY=lots",
        "HOST_HAS_IPV4=yes",
        "CPU_ARCHITECTURE=",
        "TIMEZONE=Mars/Olympus",
        "IDLE_CMD=x",
        "NOT_A_CONSTANT=1",
        "TIMEZONE",
    ] {
        let run = check(dir.path(), &["x.seed.rhai", "--const", arg]);
        assert_eq!(run.status, 2, "{arg}: {}", run.stderr);
        assert!(!run.stderr.contains("evaluated"), "{arg}");
    }
    let run = check(
        dir.path(),
        &[
            "x.seed.rhai",
            "--const",
            "NODE_NAME=a",
            "--const",
            "NODE_NAME=b",
        ],
    );
    assert_eq!(run.status, 2);
}

// spec: CLI#check-host-constants
#[test]
fn undetermined_memory_fails_where_it_is_read() {
    let dir = dir_with(&[("m.seed.rhai", "let a = 1;\nlet m = AVAILABLE_MEMORY;\n")]);
    let run = check(dir.path(), &["m.seed.rhai", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors[0]["kind"], "script");
    assert_eq!(errors[0]["line"], 2);
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("AVAILABLE_MEMORY cannot be determined"),
        "{errors:?}"
    );
    let given = check(
        dir.path(),
        &["m.seed.rhai", "--const", "AVAILABLE_MEMORY=2147483648"],
    );
    assert_eq!(given.status, 0, "{}", given.stderr);
}

// spec: CLI#check-host-constants
#[test]
fn undetermined_memory_fails_where_a_closure_captures_it() {
    let dir = dir_with(&[("m.seed.rhai", "let a = 1;\nlet f = || AVAILABLE_MEMORY;\n")]);
    let run = check(dir.path(), &["m.seed.rhai", "--json"]);
    assert_eq!(run.status, 1);
    assert_eq!(json_errors(&run)[0]["line"], 2);
}

// spec: CLI#check-host-constants
// spec: CONST#constant-lifetime
#[test]
fn undetermined_memory_read_in_a_function_fails_where_it_is_read() {
    let dir = dir_with(&[(
        "m.seed.rhai",
        "fn half() {\n    AVAILABLE_MEMORY / 2\n}\nlet h = half();\n",
    )]);
    let run = check(dir.path(), &["m.seed.rhai", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors[0]["line"], 2, "{errors:?}");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("AVAILABLE_MEMORY cannot be determined"),
        "{errors:?}"
    );
    let given = check(
        dir.path(),
        &["m.seed.rhai", "--const", "AVAILABLE_MEMORY=2147483648"],
    );
    assert_eq!(given.status, 0, "{}", given.stderr);
}

// spec: CLI#check-host-constants
#[test]
fn undetermined_memory_left_unread_passes() {
    let dir = dir_with(&[(
        "m.seed.rhai",
        "if false { let m = AVAILABLE_MEMORY; }\nlet AVAILABLE_MEMORY = 5;\nlet m = AVAILABLE_MEMORY;\n",
    )]);
    let run = check(dir.path(), &["m.seed.rhai"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#definition-checks
// spec: LIM#limits
#[test]
fn the_engine_limits_are_the_daemons() {
    let dir = dir_with(&[(
        "loop.seed.rhai",
        "let n = 0;\nwhile n < 200000 { n += 1; }\n",
    )]);
    assert_eq!(check(dir.path(), &["loop.seed.rhai"]).status, 1);
    assert_eq!(
        check(
            dir.path(),
            &["loop.seed.rhai", "--script-max-operations", "0"]
        )
        .status,
        0
    );
    assert_eq!(
        check(
            dir.path(),
            &["loop.seed.rhai", "--script-max-call-depth", "0"]
        )
        .status,
        2
    );
}

// spec: CLI#definition-checks
#[test]
fn no_gate_or_action_closure_runs() {
    let dir = dir_with(&[(
        "g.seed.rhai",
        "app.gate(\"migrate\").run(|rt| { throw \"gate ran\"; });\napp.on_action(\"deploy\", |rt, p| { throw \"action ran\"; }, #{});\n",
    )]);
    let run = check(dir.path(), &["g.seed.rhai"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#definition-sources
// spec: CLI#definition-checks
#[test]
fn a_bad_reference_or_two_sources_check_nothing() {
    let dir = dir_with(&[("x.seed.rhai", "")]);
    assert_eq!(check(dir.path(), &["--ref", "example/def:1"]).status, 2);
    assert_eq!(check(dir.path(), &["--ref", "ghcr.io/x/y"]).status, 2);
    assert_eq!(
        check(dir.path(), &["x.seed.rhai", "--ref", "ghcr.io/x/y:1"]).status,
        2
    );
    assert_eq!(check(dir.path(), &[]).status, 2);
}

// spec: CLI#definition-checks
#[test]
fn a_folder_evaluates_its_script_files_in_order() {
    let dir = dir_with(&[
        (
            "def/seedling.toml",
            "script = [\"first.rhai\", \"second.rhai\"]\n",
        ),
        ("def/first.rhai", "let first = 1;\n"),
        (
            "def/second.rhai",
            "if first != 1 { throw \"out of order\"; }\n",
        ),
    ]);
    let run = check(dir.path(), &["def"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#include-assembly
#[test]
fn an_included_script_is_evaluated() {
    let dir = dir_with(&[
        (
            "repo/apps/def/seedling.toml",
            "script = [\"lib/common.rhai\", \"app.seed.rhai\"]\n\n[include]\n\"lib/common.rhai\" = \"../shared/common.rhai\"\n",
        ),
        (
            "repo/apps/def/app.seed.rhai",
            "if common != 2 { throw \"not included\"; }\n",
        ),
        ("repo/apps/shared/common.rhai", "let common = 2;\n"),
    ]);
    std::fs::create_dir(dir.path().join("repo/.git")).unwrap();
    let run = check(dir.path(), &["repo/apps/def"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}

// spec: CLI#check-results
// spec: BND#script-formation
#[test]
fn a_syntax_error_lies_at_its_own_file_and_line() {
    let mut second = "let a = 1;\n".repeat(11);
    second.push_str("let = ;\n");
    let dir = dir_with(&[
        (
            "def/seedling.toml",
            "script = [\"first.rhai\", \"second.rhai\"]\n",
        ),
        ("def/first.rhai", "let b = 2;\nlet c = 3;\n"),
        ("def/second.rhai", &second),
    ]);
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors[0]["kind"], "script");
    assert_eq!(errors[0]["line"], 12);
    let expected = Path::new("def").join("second.rhai");
    assert_eq!(errors[0]["file"], expected.display().to_string());
}

// spec: CLI#check-results
// spec: BND#metadata-table
#[test]
fn a_metadata_table_holding_nan_lies_in_the_metadata_file() {
    let dir = dir_with(&[
        ("def/seedling.toml", "[metadata]\nratio = nan\n"),
        ("def/app.seed.rhai", ""),
    ]);
    let run = check(dir.path(), &["def", "--json"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors[0]["kind"], "bundle");
    let expected = Path::new("def").join("seedling.toml");
    assert_eq!(errors[0]["file"], expected.display().to_string());
}

// spec: BSL#evaluation
#[test]
fn an_import_that_never_runs_is_a_script_error() {
    let dir = dir_with(&[("i.seed.rhai", "if false { import \"x\" as x; }\n")]);
    let run = check(dir.path(), &["i.seed.rhai", "--json"]);
    assert_eq!(run.status, 1);
    assert_eq!(json_errors(&run)[0]["kind"], "script");
}

// spec: CLI#definition-checks
#[test]
fn a_script_error_hides_the_sample_values_validators_would_reject() {
    let dir = dir_with(&[(
        "s.seed.rhai",
        "app.param(\"alpha\").required(false).validate(|v, all| { throw \"rejected\"; });\nthrow \"broken\";\n",
    )]);
    let run = check(dir.path(), &["s.seed.rhai", "--json", "--set", "alpha=x"]);
    assert_eq!(run.status, 1);
    let errors = json_errors(&run);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["kind"], "script");
}

// spec: CLI#definition-checks
// spec: LIM#limits
#[test]
fn the_operation_limit_can_be_tightened() {
    let dir = dir_with(&[("loop.seed.rhai", "let n = 0;\nwhile n < 5000 { n += 1; }\n")]);
    assert_eq!(check(dir.path(), &["loop.seed.rhai"]).status, 0);
    assert_eq!(
        check(
            dir.path(),
            &["loop.seed.rhai", "--script-max-operations", "1000"]
        )
        .status,
        1
    );
}

// spec: CLI#definition-checks
#[test]
fn the_help_lists_the_daemons_engine_limit_options() {
    let out = Command::new(env!("CARGO_BIN_EXE_seedling-ctl"))
        .args(["definitions", "check", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    for (option, default) in [
        ("--script-max-operations", "100000"),
        ("--script-max-call-depth", "64"),
        ("--script-max-expr-depth", "64"),
        ("--script-max-string-size", "1048576"),
        ("--script-max-array-size", "10000"),
        ("--script-max-map-size", "10000"),
        ("--script-max-derived-values", "1000"),
        ("--script-max-password-hashes", "16"),
        ("--script-max-regex-compiled-size", "33554432"),
    ] {
        let at = help
            .find(option)
            .unwrap_or_else(|| panic!("{option} missing:\n{help}"));
        let rest = &help[at..];
        let entry = &rest[..rest[1..].find("--script-max").map_or(rest.len(), |i| i + 1)];
        assert!(
            entry.contains(&format!("[default: {default}]")),
            "{option}: {entry}"
        );
    }
}

// spec: CLI#definition-checks
#[test]
fn an_action_using_the_runtime_needs_no_container_engine() {
    let dir = dir_with(&[(
        "a.seed.rhai",
        "app.on_action(\"deploy\", |rt, p| { rt.start(app.job().image(\"example\").command(IDLE_CMD)); }, #{});\n",
    )]);
    let run = check(dir.path(), &["a.seed.rhai"]);
    assert_eq!(run.status, 0, "{}", run.stderr);
}
