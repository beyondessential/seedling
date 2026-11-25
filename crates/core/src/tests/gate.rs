use std::collections::BTreeSet;

use defs::{
    gate::{AfterTarget, GateKey},
    resource::{ResourceId, ResourceKind},
};

use super::*;

fn evaluate(script: &str) -> (defs::app::App, Option<crate::runtime::apps::ScriptError>) {
    crate::runtime::apps::evaluate_script(
        &seedling_protocol::names::AppName::new_unchecked("demo"),
        script,
        &Default::default(),
        &crate::ScriptLimits::default(),
    )
}

fn evaluation_error(script: &str) -> String {
    evaluate(script)
        .1
        .unwrap_or_else(|| panic!("the definition evaluates: {script}"))
        .0
}

fn evaluated(script: &str) -> defs::app::App {
    let (app, err) = evaluate(script);
    assert!(err.is_none(), "evaluates: {err:?}");
    app
}

fn key_of(script: &str) -> GateKey {
    let app = run_test_script_app(script);
    let def = app.def.load();
    def.gates.get("migrate").expect("the gate").key.clone()
}

fn workload(kind: ResourceKind, name: &str) -> ResourceId {
    ResourceId {
        kind,
        name: std::sync::Arc::new(name.to_owned()),
    }
}

const WORKLOADS: &str = r#"
    let api = app.deployment("api").image("docker.io/library/nginx:1");
    let web = app.deployment("web").image("docker.io/library/nginx:1");
    let tool = app.job("tool").image("docker.io/library/tools:1").deadline("60s");
"#;

// spec: GATE#declaration
#[test]
fn a_gate_is_declared_by_name_and_the_same_name_is_the_same_gate() {
    let app = run_test_script_app(
        r#"
        let first = app.gate("migrate").key(3);
        app.gate("migrate").run(|rt| {});
        app.gate("seed");
    "#,
    );
    let def = app.def.load();
    assert_eq!(def.gates.len(), 2);
    let migrate = &def.gates["migrate"];
    assert!(migrate.has_body, "the second call is the first gate");
    assert_eq!(migrate.key, GateKey::from_script(3_i64.into()).unwrap());
    assert_eq!(migrate.declaration, 0);
    assert_eq!(def.gates["seed"].declaration, 1);
}

// spec: GATE#declaration
#[test]
fn a_gate_is_not_a_resource() {
    let app = run_test_script_app(r#"app.gate("migrate");"#);
    assert!(app.def.load().resources.is_empty());
}

// spec: GATE#declaration
#[test]
fn a_gate_name_that_is_not_a_valid_name_throws() {
    for name in ["_migrate", "ab", "-migrate", "migrate-", "has space"] {
        let err = run_test_script_err(&format!(r#"app.gate("{name}");"#)).to_string();
        assert!(err.contains("name"), "{name}: {err}");
    }
}

// spec: GATE#declaration
#[test]
fn declaring_a_gate_in_an_action_closure_throws() {
    let err = run_action_err(
        r#"app.on_action("run-it", |rt, p| { app.gate("migrate"); });"#,
        "run-it",
    );
    assert!(err.contains("app.gate"), "{err}");
}

// spec: GATE#declaration
#[test]
fn every_builder_method_throws_outside_the_static_context() {
    for call in [
        "g.key(1)",
        "g.run(|rt| {})",
        "g.check(tool)",
        "g.exclusive(app)",
        "g.after(other)",
    ] {
        let source = format!(
            r#"
            {WORKLOADS}
            let g = app.gate("migrate");
            let other = app.gate("seed");
            app.on_action("run-it", |rt, p| {{ {call}; }});
            "#
        );
        let err = run_action_err(&source, "run-it");
        assert!(err.contains("action closure"), "{call}: {err}");
    }
}

// spec: GATE#declaration
#[test]
fn a_gate_is_not_a_selection_for_a_runtime_handle_method() {
    for call in [
        "rt.start(g)",
        "rt.stop(g)",
        "rt.query(g)",
        "rt.restart(g)",
        "rt.warm_images(g)",
        "rt.warm_certs(g)",
        "rt.start([api, g])",
    ] {
        let source = format!(
            r#"
            {WORKLOADS}
            let g = app.gate("migrate");
            app.on_action("run-it", |rt, p| {{ {call}; }});
            "#
        );
        let err = run_action_err(&source, "run-it");
        assert!(
            err.contains("a gate is not a resource") || err.contains("expects a Deployment"),
            "{call}: {err}"
        );
    }
}

// spec: GATE#keys
#[test]
fn a_gate_without_a_key_has_the_unit_key_and_calling_key_again_replaces_it() {
    assert_eq!(
        key_of(r#"app.gate("migrate");"#),
        GateKey::from_script(rhai::Dynamic::UNIT).unwrap()
    );
    assert_eq!(
        key_of(r#"app.gate("migrate").key(1).key("two");"#),
        key_of(r#"app.gate("migrate").key("two");"#)
    );
}

// spec: GATE#keys
#[test]
fn two_keys_are_the_same_when_they_are_the_same_json_value() {
    let same = [
        (
            r#"#{a: 1, b: [1, 2, #{c: "x", d: ()}]}"#,
            r#"#{b: [1, 2, #{d: (), c: "x"}], a: 1}"#,
        ),
        ("[1, 2, 3]", "[1, 2, 3]"),
        ("1.5", "1.5"),
        ("0.0", "-0.0"),
        ("true", "true"),
    ];
    for (a, b) in same {
        assert_eq!(
            key_of(&format!(r#"app.gate("migrate").key({a});"#)),
            key_of(&format!(r#"app.gate("migrate").key({b});"#)),
            "{a} and {b}"
        );
    }
}

// spec: GATE#keys
#[test]
fn two_keys_that_differ_in_value_order_or_number_type_are_not_the_same() {
    let different = [
        ("1", "1.0"),
        ("[1, 2]", "[2, 1]"),
        (r#"#{a: 1}"#, r#"#{a: 1.0}"#),
        ("()", "false"),
        (r#""1""#, "1"),
        ("[]", "#{}"),
    ];
    for (a, b) in different {
        assert_ne!(
            key_of(&format!(r#"app.gate("migrate").key({a});"#)),
            key_of(&format!(r#"app.gate("migrate").key({b});"#)),
            "{a} and {b}"
        );
    }
}

// spec: GATE#keys
#[test]
fn a_key_that_is_not_representable_in_json_throws() {
    for value in [
        "'c'",
        "0.0 / 0.0",
        "1.0 / 0.0",
        r#"app.job("tool")"#,
        r#"app.gate("seed")"#,
        r#"[1, [2, 'c']]"#,
        r#"#{a: #{b: app}}"#,
        "Fn(\"f\")",
    ] {
        let err = run_test_script_err(&format!("fn f() {{}}\napp.gate(\"migrate\").key({value});"))
            .to_string();
        assert!(err.contains("`key`"), "{value}: {err}");
    }
}

// spec: GATE#keys
#[test]
fn the_error_for_a_key_names_where_in_it_the_value_is_not_representable() {
    let err = run_test_script_err(r#"app.gate("migrate").key(#{a: [1, 'c']});"#).to_string();
    assert!(err.contains("`key`.a[1]"), "{err}");
}

// spec: GATE#keys
#[test]
fn a_key_digests_the_same_whatever_the_order_of_a_map_and_differently_for_an_int_and_a_float() {
    let cipher = crate::runtime::secrets::Cipher::for_tests();
    let digest = |script: &str| key_of(script).digest(&cipher).unwrap();
    assert_eq!(
        digest(r#"app.gate("migrate").key(#{a: 1, b: 2});"#),
        digest(r#"app.gate("migrate").key(#{b: 2, a: 1});"#)
    );
    assert_ne!(
        digest(r#"app.gate("migrate").key(1);"#),
        digest(r#"app.gate("migrate").key(1.0);"#)
    );
}

// spec: GATE#body
#[test]
fn a_gate_takes_a_body_once_and_only_a_closure() {
    let err = run_test_script_err(r#"app.gate("migrate").run(|rt| {}).run(|rt| {});"#).to_string();
    assert!(err.contains("already has a body"), "{err}");

    crate::tests::assert_named_handler_rejected("run", r#"app.gate("migrate").run(HANDLER);"#);
}

// spec: GATE#body
#[test]
fn a_gates_body_is_captured_with_the_evaluation_that_made_it() {
    defs::app::begin_closure_capture();
    let _ = run_test_script_app(r#"app.gate("migrate").run(|rt, last| {}); app.gate("seed");"#);
    let captured = defs::app::end_closure_capture();
    assert!(captured.gates["migrate"].is_some());
    assert!(captured.gates["seed"].is_none());
}

// spec: GATE#check
#[test]
fn a_check_is_a_named_job_with_a_deadline_and_an_optional_interval() {
    let app = run_test_script_app(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").check(tool);
        app.job("probe").image("docker.io/library/tools:1").deadline("5s");
        app.job("sniff").image("docker.io/library/tools:1").deadline("5s");
        app.gate("seed").check(app.job("probe"), #{{ interval: "30s" }});
        app.gate("later").check(app.job("sniff"), #{{ interval: "1m 30s" }});
        "#
    ));
    let def = app.def.load();
    let check = def.gates["migrate"].check.as_ref().expect("a check");
    assert_eq!(check.job, workload(ResourceKind::Job, "tool"));
    assert_eq!(check.interval, None);
    assert_eq!(
        def.gates["seed"].check.as_ref().unwrap().interval,
        Some(std::time::Duration::from_secs(30))
    );
    assert_eq!(
        def.gates["later"].check.as_ref().unwrap().interval,
        Some(std::time::Duration::from_secs(90))
    );
}

// spec: GATE#check
#[test]
fn calling_check_again_replaces_the_check() {
    let app = run_test_script_app(&format!(
        r#"{WORKLOADS}
        app.job("probe").image("docker.io/library/tools:1").deadline("5s");
        app.gate("migrate").check(tool).check(app.job("probe"));
        "#
    ));
    let def = app.def.load();
    assert_eq!(
        def.gates["migrate"].check.as_ref().unwrap().job,
        workload(ResourceKind::Job, "probe")
    );
}

// spec: GATE#check
#[test]
fn a_check_that_is_not_a_named_job_with_a_deadline_throws() {
    for (what, source, expected) in [
        (
            "no deadline",
            r#"app.gate("migrate").check(app.job("tool").image("docker.io/library/tools:1"));"#,
            "deadline",
        ),
        (
            "a deployment",
            r#"app.gate("migrate").check(app.deployment("api"));"#,
            "check",
        ),
        ("a string", r#"app.gate("migrate").check("tool");"#, "check"),
    ] {
        let err = run_test_script_err(source).to_string();
        assert!(err.contains(expected), "{what}: {err}");
    }
}

// spec: GATE#check
#[test]
fn a_check_takes_only_an_interval_of_at_least_a_second() {
    for (options, expected) in [
        (r#"#{ interval: "0s" }"#, "`interval`"),
        (r#"#{ interval: "1500ms" }"#, "whole number of seconds"),
        (r#"#{ interval: "-3s" }"#, "`interval`"),
        (r#"#{ interval: "5" }"#, "`interval`"),
        (r#"#{ interval: 5 }"#, "`interval`"),
        (r#"#{ every: "5s" }"#, "every"),
        (r#"#{ interval: "5s", extra: 1 }"#, "extra"),
    ] {
        let err = run_test_script_err(&format!(
            r#"{WORKLOADS}
            app.gate("migrate").check(tool, {options});"#
        ))
        .to_string();
        assert!(err.contains(expected), "{options}: {err}");
    }
}

// spec: GATE#check
#[test]
fn a_job_checks_one_gate() {
    let err = run_test_script_err(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").check(tool);
        app.gate("seed").check(tool);"#
    ))
    .to_string();
    assert!(err.contains("already the check of gate 'migrate'"), "{err}");
}

// spec: GATE#check
#[test]
fn a_gate_may_declare_its_own_check_again() {
    run_test_script_app(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").check(tool).check(tool, #{{ interval: "10s" }});"#
    ));
}

// spec: GATE#check
#[test]
fn a_check_job_in_its_gates_exclusive_selection_fails_evaluation() {
    let err = evaluation_error(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").check(tool).exclusive(app);"#
    ));
    assert!(err.contains("exclusive selection"), "{err}");
    assert!(err.contains("tool"), "{err}");
}

// spec: GATE#check
#[test]
fn a_gate_ordered_after_its_own_check_job_fails_evaluation_directly_or_through_other_targets() {
    let direct = evaluation_error(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").check(tool).after(tool);"#
    ));
    assert!(direct.contains("its own check"), "{direct}");

    let through = evaluation_error(&format!(
        r#"{WORKLOADS}
        api.after(tool);
        app.gate("migrate").check(tool).after(api);"#
    ));
    assert!(through.contains("its own check"), "{through}");
}

// spec: GATE#check
#[test]
fn a_check_job_ordered_after_its_gate_fails_evaluation_directly_or_through_other_targets() {
    let direct = evaluation_error(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").check(tool);
        tool.after(g);"#
    ));
    assert!(direct.contains("ordered after the gate"), "{direct}");

    let through = evaluation_error(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").check(tool);
        api.after(g);
        tool.after(api);"#
    ));
    assert!(through.contains("ordered after the gate"), "{through}");

    let through_gate = evaluation_error(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").check(tool);
        let h = app.gate("seed").after(g);
        tool.after(h);"#
    ));
    assert!(
        through_gate.contains("ordered after the gate"),
        "{through_gate}"
    );
}

// spec: GATE#check
#[test]
fn a_check_job_may_be_ordered_after_other_gates() {
    evaluated(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").check(tool);
        let h = app.gate("seed");
        tool.after(h);
        api.after(g);"#
    ));
}

// spec: GATE#exclusive-selection
#[test]
fn an_exclusive_selection_resolves_to_the_deployments_and_jobs_of_the_app_at_the_end() {
    let app = evaluated(
        r#"
        let g = app.gate("migrate").exclusive(app);
        app.deployment("api").image("docker.io/library/nginx:1");
        app.job("tool").image("docker.io/library/tools:1");
        app.service("front");
        app.volume("data");
        "#,
    );
    let def = app.def.load();
    assert_eq!(
        def.gates["migrate"].exclusive,
        BTreeSet::from([
            workload(ResourceKind::Deployment, "api"),
            workload(ResourceKind::Job, "tool"),
        ])
    );
}

// spec: GATE#exclusive-selection
#[test]
fn an_exclusive_selection_may_be_a_collection_a_resource_or_an_array() {
    let app = evaluated(&format!(
        r#"{WORKLOADS}
        app.gate("one").exclusive(col(app).except(tool));
        app.gate("two").exclusive(api);
        app.gate("three").exclusive([api, tool]);
        app.gate("four").exclusive(app.select(#{{ types: [ResourceType.Job] }}));
        "#
    ));
    let def = app.def.load();
    let api = workload(ResourceKind::Deployment, "api");
    let web = workload(ResourceKind::Deployment, "web");
    let tool = workload(ResourceKind::Job, "tool");
    assert_eq!(
        def.gates["one"].exclusive,
        BTreeSet::from([api.clone(), web])
    );
    assert_eq!(def.gates["two"].exclusive, BTreeSet::from([api.clone()]));
    assert_eq!(
        def.gates["three"].exclusive,
        BTreeSet::from([api, tool.clone()])
    );
    assert_eq!(def.gates["four"].exclusive, BTreeSet::from([tool]));
}

// spec: GATE#exclusive-selection
#[test]
fn calling_exclusive_again_replaces_the_selection() {
    let app = evaluated(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").exclusive(api).exclusive(web);"#
    ));
    assert_eq!(
        app.def.load().gates["migrate"].exclusive,
        BTreeSet::from([workload(ResourceKind::Deployment, "web")])
    );
}

// spec: GATE#exclusive-selection
#[test]
fn a_value_that_is_not_a_selection_makes_exclusive_throw() {
    for value in ["5", r#""api""#, "()", r#"app.gate("seed")"#, "[1]", "#{}"] {
        let err = run_test_script_err(&format!(
            r#"{WORKLOADS}
            app.gate("migrate").exclusive({value});"#
        ))
        .to_string();
        assert!(err.contains("`selection`"), "{value}: {err}");
    }
}

// spec: GATE#exclusive-selection
#[test]
fn an_exclusive_selection_holding_a_target_fails_evaluation_directly_or_through_other_targets() {
    let direct = evaluation_error(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").exclusive(api).after(api);"#
    ));
    assert!(direct.contains("deployment 'api'"), "{direct}");

    let through_workload = evaluation_error(&format!(
        r#"{WORKLOADS}
        api.after(web);
        app.gate("migrate").exclusive(web).after(api);"#
    ));
    assert!(
        through_workload.contains("deployment 'web'"),
        "{through_workload}"
    );

    let through_gate = evaluation_error(&format!(
        r#"{WORKLOADS}
        let seed = app.gate("seed").after(api);
        app.gate("migrate").exclusive(api).after(seed);"#
    ));
    assert!(through_gate.contains("deployment 'api'"), "{through_gate}");

    let whole_app = evaluation_error(&format!(
        r#"{WORKLOADS}
        app.gate("migrate").exclusive(app).after(tool);"#
    ));
    assert!(whole_app.contains("job 'tool'"), "{whole_app}");
}

// spec: GATE#exclusive-selection
#[test]
fn an_exclusive_selection_may_hold_the_dependants_of_its_gate() {
    let app = evaluated(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").exclusive(api);
        api.after(g);"#
    ));
    assert!(app.def.load().gates["migrate"].after.is_empty());
}

// spec: GATE#exclusive-selection
#[test]
fn a_selection_held_by_an_evaluation_that_did_not_complete_is_not_taken_up_by_a_later_one() {
    for _ in 0..64 {
        drop(run_test_script_err(&format!(
            r#"{WORKLOADS}
            app.gate("migrate").exclusive(api);
            throw "abandoned";"#
        )));
        let later = defs::app::App::default();
        assert!(later.take_gate_selections().is_empty());
    }
}

// spec: GATE#start-order
#[test]
fn after_orders_a_workload_or_gate_after_gates_deployments_and_jobs() {
    let app = evaluated(&format!(
        r#"{WORKLOADS}
        let seed = app.gate("seed");
        let migrate = app.gate("migrate").after(seed).after([api, tool]);
        web.after(migrate).after([seed, api]);
        tool.after(seed);"#
    ));
    let def = app.def.load();
    assert_eq!(
        def.gates["migrate"].after,
        BTreeSet::from([
            AfterTarget::Gate("seed".parse().unwrap()),
            AfterTarget::Workload(workload(ResourceKind::Deployment, "api")),
            AfterTarget::Workload(workload(ResourceKind::Job, "tool")),
        ])
    );
    let after =
        defs::gate::workload_after(&def.resources[&workload(ResourceKind::Deployment, "web")])
            .expect("a deployment");
    assert_eq!(
        after,
        BTreeSet::from([
            AfterTarget::Gate("migrate".parse().unwrap()),
            AfterTarget::Gate("seed".parse().unwrap()),
            AfterTarget::Workload(workload(ResourceKind::Deployment, "api")),
        ])
    );
}

// spec: GATE#start-order
#[test]
fn each_after_call_adds_to_the_targets_already_given() {
    let app = evaluated(&format!(
        r#"{WORKLOADS}
        let a = app.gate("alpha");
        let b = app.gate("bravo");
        web.after(a).after(b).after([]);"#
    ));
    let def = app.def.load();
    let after =
        defs::gate::workload_after(&def.resources[&workload(ResourceKind::Deployment, "web")])
            .unwrap();
    assert_eq!(after.len(), 2);
}

// spec: GATE#start-order
// spec: POD#start-order
#[test]
fn after_returns_the_builder_it_was_called_on() {
    run_test_script_app(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate");
        web.after(g).env("A", "1").after(g);
        g.after(api).key(1);"#
    ));
}

// spec: GATE#start-order
#[test]
fn a_target_that_is_not_a_gate_a_named_deployment_or_a_named_job_throws() {
    for target in [
        "5",
        r#""api""#,
        "()",
        "app",
        r#"app.service("front")"#,
        r#"[api, "web"]"#,
        "[[api]]",
        r#"app.volume("data")"#,
    ] {
        for builder in ["web", r#"app.gate("migrate")"#] {
            let err = run_test_script_err(&format!(
                r#"{WORKLOADS}
                {builder}.after({target});"#
            ))
            .to_string();
            assert!(err.contains("`target`"), "{builder}.after({target}): {err}");
        }
    }
}

// spec: GATE#start-order
#[test]
fn after_throws_in_the_action_context_and_on_an_anonymous_pod() {
    for call in [
        "api.after(g)",
        r#"app.gate("seed")"#,
        r#"app.deployment("late").after(g)"#,
        r#"app.job().image("docker.io/library/tools:1").after(g)"#,
    ] {
        let source = format!(
            r#"{WORKLOADS}
            let g = app.gate("migrate");
            app.on_action("run-it", |rt, p| {{ {call}; }});"#
        );
        let err = run_action_err(&source, "run-it");
        assert!(!err.is_empty(), "{call}");
    }
}

// spec: GATE#start-order
#[test]
fn a_gate_or_workload_ordered_after_itself_fails_evaluation() {
    for script in [
        r#"let g = app.gate("migrate"); g.after(g);"#,
        &format!("{WORKLOADS} api.after(api);"),
        &format!("{WORKLOADS} api.after([web, api]);"),
    ] {
        let err = evaluation_error(script);
        assert!(
            err.contains("cannot be ordered after itself"),
            "{script}: {err}"
        );
    }
}

// spec: GATE#start-order
#[test]
fn ordering_that_forms_a_cycle_fails_evaluation_naming_it() {
    for script in [
        r#"let a = app.gate("alpha"); let b = app.gate("bravo"); a.after(b); b.after(a);"#
            .to_owned(),
        format!("{WORKLOADS} api.after(web); web.after(api);"),
        format!(
            r#"{WORKLOADS}
            let g = app.gate("migrate");
            api.after(g); g.after(tool); tool.after(api);"#
        ),
    ] {
        let err = evaluation_error(&script);
        assert!(err.contains("cycle"), "{script}: {err}");
        assert!(err.contains("->"), "{err}");
    }
}

// spec: GATE#start-order
#[test]
fn ordering_that_is_not_a_cycle_evaluates() {
    evaluated(&format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate");
        let h = app.gate("seed").after(g);
        api.after(g).after(h);
        web.after(api).after(h);
        tool.after(g);"#
    ));
}

// spec: GATE#declaration
#[test]
fn evaluating_the_same_definition_twice_gives_equal_gates() {
    let script = &format!(
        r#"{WORKLOADS}
        let g = app.gate("migrate").key(#{{a: 1}}).exclusive(app.select(#{{ types: [ResourceType.Deployment] }}))
            .run(|rt| {{}}).check(tool, #{{ interval: "5s" }});
        api.after(g);"#
    );
    let first = evaluated(script);
    let second = evaluated(script);
    assert_eq!(first.def.load().gates, second.def.load().gates);
}

// spec: UPD#spec-hash
#[test]
fn a_workload_carries_the_digests_of_the_gates_it_is_ordered_directly_after_only() {
    use crate::runtime::secrets::Cipher;

    let app = evaluated(&format!(
        r#"{WORKLOADS}
        let migrate = app.gate("migrate").key(1);
        let seed = app.gate("seed").key(2);
        api.after(migrate).after(seed).after(tool);
        web.after(api);
        tool.after(migrate);"#
    ));
    let def = app.def.load();
    let cipher = Cipher::for_tests();
    let digests = defs::gate::GateKeyDigests::of(&def, &cipher).unwrap();
    let directly = |kind, name| {
        digests
            .directly_after(&def.resources[&workload(kind, name)])
            .unwrap()
            .into_iter()
            .map(|(gate, _)| gate.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        directly(ResourceKind::Deployment, "api"),
        ["migrate", "seed"]
    );
    assert_eq!(directly(ResourceKind::Job, "tool"), ["migrate"]);
    assert!(directly(ResourceKind::Deployment, "web").is_empty());

    let missing = defs::gate::GateKeyDigests::default();
    assert_eq!(
        missing
            .directly_after(&def.resources[&workload(ResourceKind::Job, "tool")])
            .unwrap_err()
            .0,
        "migrate"
    );
}
