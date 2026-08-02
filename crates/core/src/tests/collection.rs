use super::*;

// l[verify collection.interface]
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

// l[verify collection.one]
#[test]
fn collection_one_on_app() {
    run_test_script_app(
        r#"
        app.service("web");
        let item = app.one();
    "#,
    );
}

// l[verify collection.only]
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

// l[verify collection.except]
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

// l[verify collection.select]
// l[verify collection.select.types]
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

// l[verify collection.select]
// l[verify collection.select.names]
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

// l[verify collection.select]
// l[verify collection.select.name-patterns]
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

// l[verify collection.select]
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

// l[verify collection.col]
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

// l[verify collection.col]
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

// l[verify collection.col]
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

// l[verify collection.col]
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

// l[verify collection.col]
#[test]
fn col_from_unknown_yields_empty() {
    run_test_script_app(
        r#"
        let c = col(42);
        let item = c.one();
    "#,
    );
}

// l[verify bsl.args.strict]
// l[verify collection.select.types]
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

// l[verify bsl.args.strict]
// l[verify collection.select.types]
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

// l[verify bsl.args.strict]
// l[verify collection.select.names]
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

// l[verify bsl.args.strict]
// l[verify collection.select]
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
