//! Definitions end to end through the OI: sources, fetching, refusals,
//! validation, atomic updates, provenance, and templates.

use std::sync::atomic::Ordering;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use crate::oi::test_support::{MINIMAL_SCRIPT, TestOi};

const REPO: &str = "ghcr.io/org/app-def";

fn wire(files: &[(&str, &[u8])]) -> Value {
    let map: serde_json::Map<String, Value> = files
        .iter()
        .map(|(p, c)| ((*p).to_owned(), json!(BASE64.encode(c))))
        .collect();
    Value::Object(map)
}

fn publish(oi: &TestOi, tag: &str, files: &[(&str, &[u8])]) -> String {
    let digest = oi.registry.push_definition(REPO, files, None);
    oi.registry.tag(REPO, tag, &digest);
    digest
}

fn show(oi: &TestOi, app: &str) -> Value {
    oi.call("/apps/show", json!({ "app": app })).unwrap()
}

fn faults_of(oi: &TestOi, app: &str) -> Vec<String> {
    show(oi, app)["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn err_code(r: Result<Value, (String, String)>) -> String {
    r.expect_err("the request must be refused").0
}

/// A definition whose `version` parameter accepts only `v2.12`, with a
/// handler for it.
const DEF_B: &str = r#"
    let version = app.param("version");
    version.validate(|value, all| {
        if value != "v2.12" { throw `definition B runs v2.12 only, not ${value}`; }
    });
    version.on_change(|rt, old| {});
    app.deployment("web").image("docker.io/library/nginx:1.29");
"#;

const DEF_A: &str = r#"
    app.param("version");
    app.deployment("web").image("docker.io/library/nginx:1.29");
"#;

// i[verify definition.fetch]
// i[verify definition.provenance]
#[test]
fn a_fetched_definition_registers_with_its_reference_and_digest() {
    let oi = TestOi::new();
    let digest = publish(&oi, "1", &[("app.seed.rhai", MINIMAL_SCRIPT.as_bytes())]);
    oi.call(
        "/apps/create",
        json!({ "app": "web", "reference": format!("{REPO}:1") }),
    )
    .unwrap();
    let def = &show(&oi, "web")["definition"];
    assert_eq!(def["kind"], "fetched");
    assert_eq!(def["reference"], format!("{REPO}:1"));
    assert_eq!(def["digest"], digest);
    assert!(def["content_hash"].as_str().unwrap().starts_with("sha256:"));
}

// i[verify definition.fetch]
#[test]
fn a_definition_from_an_index_entry_records_the_entry_digest() {
    let oi = TestOi::new();
    let image = oi.registry.push_image(REPO);
    let def =
        oi.registry
            .push_definition(REPO, &[("app.seed.rhai", MINIMAL_SCRIPT.as_bytes())], None);
    let index = oi.registry.push_index(
        REPO,
        &[
            (&image, None, None),
            (
                &def,
                Some(crate::runtime::definition::fetch::ARTIFACT_TYPE),
                None,
            ),
        ],
    );
    oi.registry.tag(REPO, "2", &index);
    oi.call(
        "/apps/create",
        json!({ "app": "web", "reference": format!("{REPO}:2") }),
    )
    .unwrap();
    assert_eq!(show(&oi, "web")["definition"]["digest"], def);
}

// i[verify definition.fetch]
#[test]
fn a_reference_naming_no_definition_registers_nothing() {
    let oi = TestOi::new();
    let image = oi.registry.push_image(REPO);
    oi.registry.tag(REPO, "img", &image);
    let code = err_code(oi.call(
        "/apps/create",
        json!({ "app": "web", "reference": format!("{REPO}:img") }),
    ));
    assert_eq!(code, "fetch_failed");
    assert!(
        oi.call("/apps/list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

// i[verify definition.fetch.access]
#[test]
fn a_registry_off_the_allowlist_is_never_contacted() {
    let oi = TestOi::new();
    let code = err_code(oi.call(
        "/apps/create",
        json!({ "app": "web", "reference": "registry.example.com/org/def:1" }),
    ));
    assert_eq!(code, "registry_not_allowed");
    assert_eq!(oi.registry.requests.load(Ordering::SeqCst), 0);
}

// i[verify definition.source]
#[test]
fn exactly_one_definition_source_is_accepted() {
    let oi = TestOi::new();
    for params in [
        json!({ "app": "web" }),
        json!({ "app": "web", "script": MINIMAL_SCRIPT, "reference": format!("{REPO}:1") }),
        json!({ "app": "web", "reference": format!("{REPO}:1"), "origin": {"url": "u", "revision": "r"} }),
        json!({ "app": "web", "reference": "app-def:1" }),
    ] {
        assert_eq!(
            err_code(oi.call("/apps/create", params)),
            "requirements_invalid"
        );
    }
}

// i[verify app.update]
#[test]
fn an_update_whose_fetch_fails_changes_nothing() {
    let oi = TestOi::with_app("web");
    let before = show(&oi, "web");
    oi.registry.set_unreachable(true);
    let code = err_code(oi.call(
        "/apps/update",
        json!({ "app": "web", "reference": format!("{REPO}:1") }),
    ));
    assert_eq!(code, "fetch_failed");
    let after = show(&oi, "web");
    assert_eq!(after["generation"], before["generation"]);
    assert_eq!(after["definition"], before["definition"]);
    assert!(faults_of(&oi, "web").is_empty());
}

// i[verify definition.tags]
#[test]
fn tags_are_listed_for_allowed_repositories_only() {
    let oi = TestOi::new();
    publish(&oi, "1", &[("app.seed.rhai", b"")]);
    publish(&oi, "2", &[("app.seed.rhai", b"// 2")]);
    let tags = oi
        .call("/registries/tags", json!({ "repository": REPO }))
        .unwrap();
    assert_eq!(tags["tags"], json!(["1", "2"]));
    let code = err_code(oi.call(
        "/registries/tags",
        json!({ "repository": "registry.example.com/org/def" }),
    ));
    assert_eq!(code, "registry_not_allowed");
}

// i[verify definition.bundle.limits]
#[test]
fn a_pushed_bundle_breaking_the_rules_is_refused() {
    let oi = TestOi::new();
    let code = err_code(oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(&[("app.seed.rhai", b""), ("../escape", b"x")]) }),
    ));
    assert_eq!(code, "bundle_invalid");
    let code = err_code(oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(&[("seedling.toml", b"surprise = 1"), ("app.seed.rhai", b"")]) }),
    ));
    assert_eq!(code, "bundle_invalid");
}

// i[verify definition.bundle.seedling-versions]
#[test]
fn a_definition_for_another_seedling_is_refused() {
    let oi = TestOi::new();
    let code = err_code(oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(&[
            ("seedling.toml", b"seedling = \">=99\"\nfuture = true"),
            ("app.seed.rhai", MINIMAL_SCRIPT.as_bytes()),
        ]) }),
    ));
    assert_eq!(code, "unsupported_seedling");
    let oi = TestOi::with_app("web");
    let code = err_code(oi.call(
        "/apps/update",
        json!({ "app": "web", "bundle": wire(&[
            ("seedling.toml", b"seedling = \"<0.1\""),
            ("app.seed.rhai", MINIMAL_SCRIPT.as_bytes()),
        ]) }),
    ));
    assert_eq!(code, "unsupported_seedling");
}

// l[verify bsl.bundle]
// l[verify bsl.bundle.script-errors]
#[test]
fn several_script_files_evaluate_as_one_and_report_errors_by_file() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(&[
            ("seedling.toml", b"script = [\"lib.rhai\", \"app.seed.rhai\"]"),
            ("lib.rhai", b"fn image() { \"docker.io/library/nginx:1.29\" }\n"),
            ("app.seed.rhai", b"app.deployment(\"web\").image(image());\n"),
        ]) }),
    )
    .unwrap();
    let script = oi.call("/apps/script", json!({ "app": "web" })).unwrap();
    assert_eq!(script["files"], json!(["lib.rhai", "app.seed.rhai"]));

    let (code, message) = oi
        .call(
            "/apps/update",
            json!({ "app": "web", "bundle": wire(&[
                ("seedling.toml", b"script = [\"lib.rhai\", \"app.seed.rhai\"]"),
                ("lib.rhai", b"let a = 1;\nlet b = 2;\n"),
                ("app.seed.rhai", b"let c = 3;\nthrow \"bad\";\n"),
            ]) }),
        )
        .unwrap_err();
    assert_eq!(code, "script_error");
    assert!(message.contains("app.seed.rhai line 2"), "{message}");
}

// i[verify definition.content-hash]
// r[verify generation.script-storage]
#[test]
fn pushed_and_fetched_copies_of_one_folder_share_a_hash_and_a_row() {
    let oi = TestOi::new();
    let files: &[(&str, &[u8])] = &[
        ("app.seed.rhai", MINIMAL_SCRIPT.as_bytes()),
        ("pages/500.html", b"<h1>oops</h1>"),
    ];
    oi.call(
        "/apps/create",
        json!({ "app": "pushed", "bundle": wire(files) }),
    )
    .unwrap();
    publish(&oi, "1", files);
    oi.call(
        "/apps/create",
        json!({ "app": "fetched", "reference": format!("{REPO}:1") }),
    )
    .unwrap();
    assert_eq!(
        show(&oi, "pushed")["definition"]["content_hash"],
        show(&oi, "fetched")["definition"]["content_hash"]
    );
    let rows: i64 = oi
        .state
        .db
        .call(|db| {
            db.conn
                .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
        })
        .unwrap();
    assert_eq!(rows, 1);
}

// i[verify app.update]
#[test]
fn a_refused_update_stores_nothing_and_leaves_faults_alone() {
    let oi = TestOi::with_app("web");
    // A failed param set leaves a script_error standing.
    oi.call(
        "/apps/update",
        json!({ "app": "web", "script": r#"
            let mode = app.param("mode");
            if mode.is_set() && mode.value() == "broken" { throw "boom"; }
            app.deployment("web").image("docker.io/library/nginx:1.29");
        "# }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "mode", "value": "broken" }),
    )
    .unwrap();
    assert_eq!(faults_of(&oi, "web"), vec!["script_error"]);
    let before = show(&oi, "web");
    let rows_before: i64 = oi
        .state
        .db
        .call(|db| {
            db.conn
                .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
        })
        .unwrap();

    let code = err_code(oi.call(
        "/apps/update",
        json!({ "app": "web", "script": "throw \"still broken\";" }),
    ));
    assert_eq!(code, "script_error");
    let after = show(&oi, "web");
    assert_eq!(after["generation"], before["generation"]);
    assert_eq!(after["definition"], before["definition"]);
    assert_eq!(after["resources"], before["resources"]);
    assert_eq!(
        faults_of(&oi, "web"),
        vec!["script_error"],
        "not cleared, not refiled"
    );
    let rows_after: i64 = oi
        .state
        .db
        .call(|db| {
            db.conn
                .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
        })
        .unwrap();
    assert_eq!(rows_after, rows_before, "no bundle stored");

    // A successful update clears the standing fault.
    oi.call(
        "/apps/update",
        json!({ "app": "web", "script": MINIMAL_SCRIPT }),
    )
    .unwrap();
    assert!(faults_of(&oi, "web").is_empty());
}

// i[verify param.set]
// l[verify param.validate]
// i[verify param.validation]
#[test]
fn a_param_set_the_validator_rejects_is_refused() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_B }))
        .unwrap();
    let before = show(&oi, "web")["generation"].clone();
    let (code, message) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "web", "name": "version", "value": "v2.11" }),
        )
        .unwrap_err();
    assert_eq!(code, "validation_failed");
    assert!(
        message.contains("version") && message.contains("v2.12 only, not v2.11"),
        "{message}"
    );
    let after = show(&oi, "web");
    assert_eq!(after["generation"], before);
    let version = after["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "version")
        .unwrap()
        .clone();
    assert_eq!(version["is_set"], false);

    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "version", "value": "v2.12" }),
    )
    .unwrap();
}

// l[verify param.validate]
#[test]
fn a_validator_that_errors_by_accident_rejects() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "script": r#"
            app.param("knob").validate(|v, all| { no_such_function(v); });
        "# }),
    )
    .unwrap();
    let code = err_code(oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "knob", "value": "1" }),
    ));
    assert_eq!(code, "validation_failed");
}

// l[verify param.validate.unset]
// i[verify param.validation]
#[test]
fn an_unset_param_passes_and_registration_runs_no_validators() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_B }))
        .expect("registration with `version` unset succeeds");
}

// i[verify param.unset]
// i[verify param.validation]
#[test]
fn an_unset_another_validator_rejects_is_refused() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "script": r#"
            app.param("tls");
            app.param("port").validate(|v, all| {
                if v == "443" && !("tls" in all) { throw "port 443 needs tls"; }
            });
        "# }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "tls", "value": "on" }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "port", "value": "443" }),
    )
    .unwrap();
    let code = err_code(oi.call("/apps/params/unset", json!({ "app": "web", "name": "tls" })));
    assert_eq!(code, "validation_failed");
}

const FALLBACK_SCRIPT: &str = r#"
    let mode = app.param("mode");
    mode.validate(|v, all| { if v == "forbidden" { throw "mode may not be forbidden"; } });
    if mode.is_set() && mode.value() != "ok" { throw "unsupported mode"; }
"#;

// i[verify param.validation]
// i[verify param.set]
#[test]
fn a_failing_evaluation_falls_back_to_the_last_good_validators() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "script": FALLBACK_SCRIPT }),
    )
    .unwrap();
    // Fails to evaluate, passes the last good validators: stored, faulted.
    let result = oi
        .call(
            "/apps/params/set",
            json!({ "app": "web", "name": "mode", "value": "odd" }),
        )
        .unwrap();
    assert!(result["generation"].as_u64().unwrap() > 1);
    assert_eq!(faults_of(&oi, "web"), vec!["script_error"]);
    // Fails to evaluate and fails the last good validators: refused.
    let code = err_code(oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "mode", "value": "forbidden" }),
    ));
    assert_eq!(code, "validation_failed");
}

// i[verify param.validation]
#[test]
fn a_conditional_validator_is_found_under_the_proposed_values() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "script": r#"
            let strict = app.param("strict");
            let name = app.param("name");
            if strict.is_set() {
                name.validate(|v, all| { if v.len() > 3 { throw "too long under strict"; } });
            }
        "# }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "name", "value": "lengthy" }),
    )
    .unwrap();
    let code = err_code(oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "strict", "value": "yes" }),
    ));
    assert_eq!(code, "validation_failed");
}

// l[verify param.validate.pure]
#[test]
fn a_validator_defining_a_resource_rejects() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "script": r#"
            app.param("knob").validate(|v, all| { app.volume("sneaky"); });
        "# }),
    )
    .unwrap();
    let (code, message) = oi
        .call(
            "/apps/params/set",
            json!({ "app": "web", "name": "knob", "value": "1" }),
        )
        .unwrap_err();
    assert_eq!(code, "validation_failed");
    assert!(message.contains("must not define resources"), "{message}");
}

// i[verify app.update]
// l[verify param.on-change.old]
// r[verify generation.reconstruction]
// r[verify operation.lifecycle.generations]
#[test]
fn a_regime_change_is_one_atomic_definition_and_param_update() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_A }))
        .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "version", "value": "v2.11" }),
    )
    .unwrap();
    oi.install("web");
    let before = show(&oi, "web")["generation"].as_u64().unwrap();

    // B alone is refused: the stored v2.11 fails B's validator.
    let code = err_code(oi.call("/apps/update", json!({ "app": "web", "script": DEF_B })));
    assert_eq!(code, "validation_failed");
    assert_eq!(show(&oi, "web")["generation"].as_u64().unwrap(), before);

    // B with v2.12 in the same step succeeds in one generation, and B's
    // handler for `version` is scheduled.
    let result = oi
        .call(
            "/apps/update",
            json!({ "app": "web", "script": DEF_B, "param": { "name": "version", "value": "v2.12" } }),
        )
        .unwrap();
    let generation = result["generation"].as_u64().unwrap();
    assert_eq!(generation, before + 1);
    assert_eq!(result["schedule"], "accepted");

    // `old` for the handler is the previous generation: A at v2.11; the
    // target is B at v2.12.
    let app = seedling_protocol::names::AppName::new("web").unwrap();
    let cipher = std::sync::Arc::clone(&oi.state.cipher);
    let limits = oi.state.script_limits.clone();
    let (old, new) = oi.state.db.call(move |db| {
        use crate::runtime::generations::reconstruct_app_def;
        (
            reconstruct_app_def(db, &app, generation - 1, &limits, &cipher).unwrap(),
            reconstruct_app_def(db, &app, generation, &limits, &cipher).unwrap(),
        )
    });
    assert_eq!(
        old.stored.lock().get("version").map(String::as_str),
        Some("v2.11")
    );
    assert!(old.def.load().validators.is_empty(), "old is definition A");
    assert_eq!(
        new.stored.lock().get("version").map(String::as_str),
        Some("v2.12")
    );
    assert!(!new.def.load().validators.is_empty(), "new is definition B");

    let history = oi
        .call("/apps/generations", json!({ "app": "web" }))
        .unwrap();
    let entry = &history[0];
    assert_eq!(entry["kind"], "script_update");
    assert_eq!(entry["param_name"], "version");
    assert_eq!(entry["previous_value"], "v2.11");
    assert_eq!(entry["new_value"], "v2.12");
    assert_eq!(entry["definition"]["kind"], "pushed");
}

// i[verify param.validation]
#[test]
fn another_validator_sees_the_proposed_values_during_an_atomic_update() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_A }))
        .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "version", "value": "v2.11" }),
    )
    .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "database", "value": "pg16" }),
    )
    .unwrap();
    let def = r#"
        app.param("version");
        app.param("database").validate(|v, all| {
            if all["version"] != "v2.12" { throw `database sees ${all["version"]}`; }
        });
    "#;
    oi.call(
        "/apps/update",
        json!({ "app": "web", "script": def, "param": { "name": "version", "value": "v2.12" } }),
    )
    .expect("the database validator sees the proposed v2.12");
}

// i[verify app.update]
#[test]
fn a_definition_update_without_a_param_change_schedules_nothing() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_B }))
        .unwrap();
    oi.install("web");
    let result = oi
        .call("/apps/update", json!({ "app": "web", "script": DEF_B }))
        .unwrap();
    assert_eq!(result["schedule"], "not_scheduled");
}

// i[verify definition.provenance]
// i[verify definition.source]
// i[verify generation.history]
#[test]
fn a_push_records_the_actor_and_the_reported_origin() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "web",
            "script": MINIMAL_SCRIPT,
            "origin": { "url": "https://github.com/o/r/tree/main/web", "revision": "abc123" },
        }),
    )
    .unwrap();
    let def = &show(&oi, "web")["definition"];
    assert_eq!(def["kind"], "pushed");
    assert_eq!(def["pushed_by"]["id"], "test-suite");
    assert_eq!(def["reported_origin"]["revision"], "abc123");
    let history = oi
        .call("/apps/generations", json!({ "app": "web" }))
        .unwrap();
    assert_eq!(history[0]["definition"], *def);
}

// i[verify app.bundle]
// i[verify app.script]
#[test]
fn every_generation_bundle_is_retrievable() {
    let oi = TestOi::new();
    let first: &[(&str, &[u8])] = &[
        ("app.seed.rhai", MINIMAL_SCRIPT.as_bytes()),
        ("pages/500.html", &[0xde, 0xad, 0xbe, 0xef]),
    ];
    oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(first) }),
    )
    .unwrap();
    oi.call(
        "/apps/update",
        json!({ "app": "web", "script": MINIMAL_SCRIPT }),
    )
    .unwrap();
    let old = oi
        .call("/apps/bundle", json!({ "app": "web", "generation": 1 }))
        .unwrap();
    assert_eq!(old["bundle"], wire(first));
    assert_eq!(old["provenance"]["kind"], "pushed");
    let current = oi.call("/apps/bundle", json!({ "app": "web" })).unwrap();
    assert_eq!(current["generation"], 2);
    assert_eq!(
        current["bundle"],
        wire(&[("app.seed.rhai", MINIMAL_SCRIPT.as_bytes())])
    );
    let script = oi
        .call("/apps/script", json!({ "app": "web", "generation": 1 }))
        .unwrap();
    assert_eq!(script["script"], MINIMAL_SCRIPT);
    assert_eq!(script["files"], json!(["app.seed.rhai"]));
}

// r[verify generation.deregister]
#[test]
fn deregistering_deletes_bundles_only_that_app_referenced() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "one", "script": MINIMAL_SCRIPT }),
    )
    .unwrap();
    oi.call(
        "/apps/create",
        json!({ "app": "two", "script": MINIMAL_SCRIPT }),
    )
    .unwrap();
    oi.call("/apps/update", json!({ "app": "one", "script": DEF_A }))
        .unwrap();
    let count = |oi: &TestOi| -> i64 {
        oi.state
            .db
            .call(|db| {
                db.conn
                    .query_row("SELECT COUNT(*) FROM definition_bundles", [], |r| r.get(0))
            })
            .unwrap()
    };
    assert_eq!(count(&oi), 2);
    oi.call("/apps/remove", json!({ "app": "one" })).unwrap();
    assert_eq!(count(&oi), 1, "MINIMAL_SCRIPT is still referenced by `two`");
}

// i[verify plan.dry-run]
// i[verify param.validation]
#[test]
fn the_plan_reports_validator_rejections_for_a_proposed_definition() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_A }))
        .unwrap();
    oi.call(
        "/apps/params/set",
        json!({ "app": "web", "name": "version", "value": "v2.11" }),
    )
    .unwrap();
    publish(&oi, "b", &[("app.seed.rhai", DEF_B.as_bytes())]);
    let plan = oi
        .call(
            "/apps/plan",
            json!({ "app": "web", "proposed_reference": format!("{REPO}:b") }),
        )
        .unwrap();
    assert_eq!(plan["rejections"][0]["name"], "version");
    let plan = oi
        .call(
            "/apps/plan",
            json!({
                "app": "web",
                "proposed_reference": format!("{REPO}:b"),
                "proposed_params": [{ "name": "version", "value": "v2.12" }],
            }),
        )
        .unwrap();
    assert_eq!(plan["rejections"], json!([]));
    assert_eq!(plan["on_change_would_fire"], json!(["version"]));
    let code = err_code(oi.call(
        "/apps/plan",
        json!({ "app": "web", "proposed_reference": "registry.example.com/x/y:1" }),
    ));
    assert_eq!(code, "registry_not_allowed");
}

// r[verify fault.definition-unsupported]
#[test]
fn an_unsupported_stored_definition_faults_until_replaced() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({ "app": "web", "bundle": wire(&[
            ("seedling.toml", b"seedling = \"<0.13\""),
            ("app.seed.rhai", MINIMAL_SCRIPT.as_bytes()),
        ]) }),
    )
    .unwrap();
    let app = seedling_protocol::names::AppName::new("web").unwrap();
    let bundle = std::sync::Arc::clone(&oi.state.registry.read().get("web").unwrap().bundle);
    // As if the runtime had restarted as 0.13.
    let (a, b) = (app.clone(), std::sync::Arc::clone(&bundle));
    oi.state.db.call(move |db| {
        crate::runtime::definition::faults::sync_unsupported(
            db,
            &a,
            &b,
            &semver::Version::new(0, 13, 0),
        );
    });
    assert_eq!(faults_of(&oi, "web"), vec!["definition_unsupported"]);
    // The app still runs its definition.
    assert!(!show(&oi, "web")["resources"].as_array().unwrap().is_empty());
    // Restarting as a supported version clears it.
    let (a, b) = (app.clone(), bundle);
    oi.state.db.call(move |db| {
        crate::runtime::definition::faults::sync_unsupported(
            db,
            &a,
            &b,
            &semver::Version::new(0, 12, 5),
        );
    });
    assert!(faults_of(&oi, "web").is_empty());
}

// i[verify template.definition]
// i[verify template.instantiate]
#[test]
fn a_template_from_a_reference_passes_its_provenance_and_files_on() {
    let oi = TestOi::new();
    let digest = publish(
        &oi,
        "1",
        &[
            (
                "app.seed.rhai",
                br#"app.volume("pages").write("/500.html", app.file("pages/500.html"));"#,
            ),
            ("pages/500.html", b"<h1>oops</h1>"),
        ],
    );
    oi.call(
        "/templates/create",
        json!({ "name": "pages", "reference": format!("{REPO}:1") }),
    )
    .unwrap();
    let t = oi
        .call("/templates/show", json!({ "name": "pages" }))
        .unwrap();
    assert_eq!(t["provenance"]["kind"], "fetched");
    assert_eq!(t["provenance"]["digest"], digest);
    assert_eq!(t["supported"], true);
    oi.call(
        "/templates/instantiate",
        json!({ "template": "pages", "app": "site" }),
    )
    .unwrap();
    let def = &show(&oi, "site")["definition"];
    assert_eq!(def["kind"], "fetched");
    assert_eq!(def["digest"], digest);
}

// i[verify template.create]
// i[verify template.list]
// i[verify template.instantiate]
#[test]
fn an_unsupported_template_is_stored_but_not_instantiated() {
    let oi = TestOi::new();
    oi.call(
        "/templates/create",
        json!({ "name": "future", "bundle": wire(&[
            ("seedling.toml", b"seedling = \">=99\"\nnew_field = 1"),
            ("app.seed.rhai", b""),
        ]) }),
    )
    .unwrap();
    let list = oi.call("/templates/list", json!({})).unwrap();
    assert_eq!(list[0]["supported"], false);
    let code = err_code(oi.call(
        "/templates/instantiate",
        json!({ "template": "future", "app": "web" }),
    ));
    assert_eq!(code, "unsupported_seedling");
}

// i[verify event.types]
#[test]
fn app_updated_carries_the_provenance_and_the_param_change() {
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "web", "script": DEF_A }))
        .unwrap();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/apps/update",
        json!({ "app": "web", "script": DEF_A, "param": { "name": "version", "value": "v3" } }),
    )
    .unwrap();
    let event = loop {
        let e = rx.try_recv().expect("an AppUpdated event");
        let v = serde_json::to_value(&e).unwrap();
        if v["type"] == "AppUpdated" {
            break v;
        }
    };
    assert_eq!(event["definition"]["kind"], "pushed");
    assert_eq!(event["name"], "version");
    assert_eq!(event["previous_value"], Value::Null);
    assert_eq!(event["new_value"], "v3");
}
