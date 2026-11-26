use super::*;
use crate::runtime::barrier::runtime::ActionClosureGuard;
use crate::tests::ActionHooks;

// spec: COL#collection
#[test]
fn collection_is_abstract_interface() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let dep = app.deployment("web");
        let job = app.job("task");
    "#,
    );
}

// spec: COL#query-methods
#[test]
fn collection_one_on_app() {
    run_test_script_app(
        r#"
        app.service("web");
        let item = app.one();
    "#,
    );
}

// spec: COL#query-methods
#[test]
fn collection_only_returns_collection() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let dep = app.deployment("api");
        let subset = app.only(svc);
    "#,
    );
}

// spec: COL#query-methods
#[test]
fn collection_except_returns_collection() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let dep = app.deployment("api");
        let rest = app.except(svc);
    "#,
    );
}

// spec: COL#selection
#[test]
fn collection_select_by_types() {
    run_test_script_app(
        r#"
        app.service("web");
        app.deployment("api");
        let selected = app.select(#{
            types: [ResourceType.Service],
        });
    "#,
    );
}

// spec: COL#selection
#[test]
fn collection_select_by_names() {
    run_test_script_app(
        r#"
        app.service("web");
        app.service("api");
        let selected = app.select(#{
            names: ["web"],
        });
    "#,
    );
}

// spec: COL#selection
#[test]
fn collection_select_by_name_patterns() {
    run_test_script_app(
        r#"
        app.service("web-frontend");
        app.service("web-backend");
        app.service("api");
        let selected = app.select(#{
            name_patterns: ["web-*"],
        });
    "#,
    );
}

// spec: COL#selection
#[test]
fn collection_select_multiple_criteria() {
    run_test_script_app(
        r#"
        app.service("web");
        app.deployment("web");
        let selected = app.select(#{
            types: [ResourceType.Service],
            names: ["web"],
        });
    "#,
    );
}

// spec: COL#coercion
#[test]
fn col_from_collection_is_identity() {
    run_test_script_app(
        r#"
        app.service("web");
        let c = col(app);
        let c2 = col(c);
    "#,
    );
}

// spec: COL#coercion
#[test]
fn col_from_app_yields_all_resources() {
    run_test_script_app(
        r#"
        app.service("web");
        app.deployment("api");
        let c = col(app);
        let item = c.one();
    "#,
    );
}

// spec: COL#coercion
#[test]
fn col_from_resource_yields_single_item() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let c = col(svc);
        let item = c.one();
    "#,
    );
}

// spec: COL#coercion
#[test]
fn col_from_array_yields_union() {
    run_test_script_app(
        r#"
        let svc = app.service("web");
        let dep = app.deployment("api");
        let c = col([svc, dep]);
        let item = c.one();
    "#,
    );
}

// spec: COL#coercion
#[test]
fn col_from_unknown_yields_empty() {
    run_test_script_app(
        r#"
        let c = col(42);
        let item = c.one();
    "#,
    );
}

// spec: COL#coercion
// An Action is invoked, never scheduled: `col` must not wrap it into a
// Collection that could reach `rt.start` and friends.
#[test]
fn col_from_action_yields_empty() {
    run_test_script_app(
        r#"
        let act = app.on_action("migrate", |rt, _param| {});
        if type_of(col(act).one()) != "()" {
            throw "col(action) must be empty";
        }
        if type_of(col([act]).one()) != "()" {
            throw "col([action]) must be empty";
        }
    "#,
    );
}

// spec: COL#collection
// A gate is not a resource, so no collection holds one.
#[test]
fn col_from_gate_yields_empty() {
    run_test_script_app(
        r#"
        let g = app.gate("migrate");
        app.deployment("web");
        if type_of(col(g).one()) != "()" {
            throw "col(gate) must be empty";
        }
        if type_of(col([g]).one()) != "()" {
            throw "col([gate]) must be empty";
        }
    "#,
    );
}

// spec: BSL#arguments
// spec: COL#selection
// A `types` value that is not an array used to drop the criterion, and a
// select with no surviving criteria matches every resource in the app — so a
// following rt.stop stopped every workload instead of the services.
#[test]
fn select_types_must_be_an_array() {
    let err = run_test_script_err(
        r#"
        app.service("web");
        app.select(#{ types: ResourceType.Service });
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("select types"), "{message}");
    assert!(message.contains("must be an array"), "{message}");
}

// spec: BSL#arguments
// spec: COL#selection
#[test]
fn select_types_elements_must_be_resource_types() {
    let err = run_test_script_err(
        r#"
        app.service("web");
        app.select(#{ types: [ResourceType.Service, "deployment"] });
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("element 1"), "{message}");
}

// spec: BSL#arguments
// spec: COL#selection
#[test]
fn select_names_elements_must_be_strings() {
    let err = run_test_script_err(
        r#"
        app.service("web");
        app.select(#{ names: ["web", 42] });
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("select names"), "{message}");
    assert!(message.contains("element 1"), "{message}");
}

// spec: BSL#arguments
// spec: COL#selection
// The spec says all possible keys are defined in it, so a typo'd key is a
// criterion that silently constrains nothing.
#[test]
fn select_rejects_unknown_criteria() {
    let err = run_test_script_err(
        r#"
        app.service("web");
        app.select(#{ nmaes: ["web"] });
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("unknown select criterion"), "{message}");
    assert!(message.contains("nmaes"), "{message}");
}

fn collection_size(source: &str) -> usize {
    use crate::defs::collection::Collection;

    let (engine, mut scope, _app, _ast) = run_test_script(
        r#"
        let web = app.service("web");
        let api = app.service("api");
        let ext = app.external_service("upstream");
        let web_http = web.http(80);
    "#,
    );
    let _guard = ActionClosureGuard::new(
        Arc::new(arc_swap::ArcSwap::new(Arc::new(
            defs::app::AppDef::default(),
        ))),
        uuid::Uuid::new_v4().to_string(),
        std::collections::HashMap::new(),
    );
    let collection: Collection = engine
        .eval_with_scope(&mut scope, source)
        .expect("the collection expression should evaluate");
    collection.resolve().len()
}

// spec: COL#coercion
// The branch coercing an external service was lost when ExternalService was
// removed, so `col(external_service)` was an empty collection.
#[test]
fn col_from_external_service_is_that_service_alone() {
    assert_eq!(collection_size("col(ext)"), 1);
    assert_eq!(collection_size("col(ext).only(ext)"), 1);
    assert_eq!(collection_size("col(ext).only(web)"), 0);
    assert_eq!(collection_size("col(ext).except(ext)"), 0);
    assert_eq!(collection_size("col(app).only(ext)"), 1);
    assert_eq!(
        collection_size("col(app).select(#{ types: [ResourceType.ExternalService] })"),
        1
    );
}

// spec: COL#coercion
#[test]
fn col_from_http_service_is_the_service_it_views() {
    assert_eq!(collection_size("col(web_http)"), 1);
    assert_eq!(collection_size("col(web_http).only(web)"), 1);
    assert_eq!(collection_size("col(web_http).only(api)"), 0);
}

// spec: COL#membership
#[test]
fn collection_holds_each_resource_once() {
    assert_eq!(collection_size("col([web, web])"), 1);
    assert_eq!(collection_size("col([web, api, web])"), 2);
    assert_eq!(collection_size("col([col(app), web, ext])"), 3);
    assert_eq!(collection_size("col([web, col(web)])"), 1);
}

// spec: COL#membership
// spec: IDN#anonymous-instances
// Anonymous resources carried an empty name, so every anonymous resource of
// one type was the same resource as every other.
#[test]
fn anonymous_resources_are_the_same_only_as_themselves() {
    let setup = "let a = app.deployment(); let b = app.deployment(); \
                 let j = app.job(); let k = app.job(); \
                 let s = app.service(); let t = app.service(); ";
    for (expression, size) in [
        ("col(a)", 1),
        ("col([a, b])", 2),
        ("col([a, a])", 1),
        ("col([a, b]).only(a)", 1),
        ("col([a, b]).except(a)", 1),
        ("col(a).only(b)", 0),
        ("col(a).except(b)", 1),
        ("col(a).except(a)", 0),
        ("col([j, k]).only(k)", 1),
        ("col([j, k]).except(j)", 1),
        ("col([s, t]).only(s)", 1),
        ("col([s, t]).except(t)", 1),
        ("col([a, j, s]).only(j)", 1),
        ("col([a, web]).except(a)", 1),
        ("col(a).only(web)", 0),
    ] {
        assert_eq!(
            collection_size(&format!("{setup}{expression}")),
            size,
            "{expression}"
        );
    }
}

// spec: COL#selection
// An anonymous resource has no name, so a pattern that matches the empty
// string must not match it.
#[test]
fn anonymous_resources_match_no_name_criterion() {
    let setup = "let a = app.deployment(); ";
    assert_eq!(
        collection_size(&format!(
            "{setup}col(a).select(#{{ name_patterns: [\"*\"] }})"
        )),
        0
    );
    assert_eq!(
        collection_size(&format!("{setup}col(a).select(#{{ names: [\"\"] }})")),
        0
    );
    assert_eq!(
        collection_size(&format!(
            "{setup}col(a).select(#{{ types: [ResourceType.Deployment] }})"
        )),
        1
    );
}

// spec: IDN#anonymous-instances
// Each extraction minted a new instance for an anonymous resource, so a Job
// started twice ran as two Jobs and a Started never narrowed to the resource
// it came from.
#[test]
fn anonymous_resource_keeps_one_instance_for_its_lifetime() {
    let progress = Arc::new(parking_lot::RwLock::new(None));
    let result = run_action_with(
        r#"
        app.on_action("launch", |rt, _param| {
            let j = app.job().image("docker.io/library/busybox:latest");
            let other = app.job().image("docker.io/library/busybox:latest");
            let started = rt.start([j, other]);
            if type_of(started.only(j).one()) == "()" {
                throw "a Started must narrow to the anonymous resource it holds";
            }
            if type_of(started.except(j).one()) == "()" {
                throw "the other anonymous resource must remain";
            }
            if type_of(started.except(j).except(other).one()) != "()" {
                throw "nothing remains once both are excluded";
            }
            rt.start(j);
        });
    "#,
        &[],
        "launch",
        None,
        ActionHooks {
            active_progress: Some(Arc::clone(&progress)),
            ..Default::default()
        },
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "got {result:?}"
    );
    let progress = progress.read().clone().expect("progress should be set");
    assert_eq!(
        progress.dynamic_defs.len(),
        2,
        "two anonymous jobs, each started under one identity"
    );
}
