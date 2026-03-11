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
