use serde_json::json;

use crate::oi::test_support::TestOi;

const REF: &str = "docker.io/library/nginx:1.29";

// spec: IMGM#pulling-an-image
// spec: IMGM#listing-images
#[test]
fn pull_then_list_shows_image() {
    let oi = TestOi::new();
    oi.call("/images/pull", json!({ "reference": REF }))
        .unwrap();

    let result = oi.call("/images/list", json!({})).unwrap();
    let images = result["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    let tags = images[0]["tags"].as_array().unwrap();
    assert_eq!(tags[0], REF);
    assert_eq!(images[0]["in_use"], false);
    assert!(images[0]["pinned_by"].as_array().unwrap().is_empty());
}

// spec: IMGM#pulling-an-image
#[test]
fn pull_rejects_empty_reference_and_unknown_app() {
    let oi = TestOi::new();
    let (code, _) = oi
        .call("/images/pull", json!({ "reference": "  " }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call("/images/pull", json!({ "reference": REF, "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: IMGM#pulling-an-image
// spec: WIRE#error-codes
// A failed pull answered `not_found`, which says the reference is unknown to
// the host rather than that fetching it failed. The pin is kept all the same.
#[test]
fn a_failed_pull_answers_fetch_failed_and_keeps_the_pin() {
    let oi = TestOi::with_unreadable_containers();
    oi.call(
        "/apps/create",
        json!({ "app": "demo", "script": crate::oi::test_support::MINIMAL_SCRIPT }),
    )
    .unwrap();
    let (code, message) = oi
        .call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap_err();
    assert_eq!(code, "fetch_failed");
    assert!(message.contains("pull failed"), "{message}");

    let result = oi.call("/images/pins/list", json!({})).unwrap();
    assert_eq!(result["pins"].as_array().unwrap().len(), 1);
}

// spec: IMGM#pulling-an-image
// spec: IMGM#image-pins
#[test]
fn pull_for_app_creates_pin() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();

    let result = oi.call("/images/pins/list", json!({})).unwrap();
    let pins = result["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0]["app"], "demo");
    assert_eq!(pins[0]["reference"], REF);

    // The pinning app also shows up against the image in /images/list.
    let result = oi.call("/images/list", json!({})).unwrap();
    let pinned_by = result["images"][0]["pinned_by"].as_array().unwrap();
    assert_eq!(pinned_by.len(), 1);
    assert_eq!(pinned_by[0], "demo");
}

// spec: IMGM#image-pins
#[test]
fn pins_list_filters_by_app_and_validates_name() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();

    let result = oi
        .call("/images/pins/list", json!({ "app": "demo" }))
        .unwrap();
    assert_eq!(result["pins"].as_array().unwrap().len(), 1);

    let (code, _) = oi
        .call("/images/pins/list", json!({ "app": "_bad_" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
}

// spec: IMGM#image-pins
#[test]
fn pins_clear_removes_pins_for_app() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();
    oi.call("/images/pins/clear", json!({ "app": "demo" }))
        .unwrap();

    let result = oi.call("/images/pins/list", json!({})).unwrap();
    assert!(result["pins"].as_array().unwrap().is_empty());
}

// spec: IMGM#removing-an-image
#[test]
fn remove_deletes_pulled_image() {
    let oi = TestOi::new();
    oi.call("/images/pull", json!({ "reference": REF }))
        .unwrap();
    oi.call("/images/remove", json!({ "reference": REF }))
        .unwrap();

    let result = oi.call("/images/list", json!({})).unwrap();
    assert!(result["images"].as_array().unwrap().is_empty());
}

// spec: IMGM#removing-an-image
#[test]
fn remove_unknown_image_is_not_found() {
    let oi = TestOi::new();
    let (code, message) = oi
        .call(
            "/images/remove",
            json!({ "reference": "docker.io/nope:latest" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
    assert!(message.contains("not found locally"), "message: {message}");
}

// spec: IMGM#removing-an-image
#[test]
fn removing_an_image_in_use_is_image_in_use_and_clears_no_pin() {
    use crate::system::types::{TransientRestart, TransientUnitSpec};

    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();
    oi.block_on(
        oi.state.driver.process.start_transient(TransientUnitSpec {
            name: "seedling-user.service".to_owned(),
            description: String::new(),
            exec_start: ["podman", "run", "--name", "seedling-user", REF]
                .map(str::to_owned)
                .to_vec(),
            restart: TransientRestart::Always,
            log_extra_fields: vec![],
            kill_signal: None,
            timeout_stop_secs: None,
            standing: crate::system::priority::UnitStanding::Infrastructure,
        }),
    )
    .unwrap();

    let (code, _) = oi
        .call("/images/remove", json!({ "reference": REF }))
        .unwrap_err();
    assert_eq!(code, "image_in_use");
    let pins = oi.call("/images/pins/list", json!({})).unwrap();
    assert_eq!(pins["pins"].as_array().unwrap().len(), 1, "{pins}");

    oi.call("/images/remove", json!({ "reference": REF, "force": true }))
        .unwrap();
    let pins = oi.call("/images/pins/list", json!({})).unwrap();
    assert!(pins["pins"].as_array().unwrap().is_empty());
}

// spec: IMGM#removing-an-image
#[test]
fn the_pins_on_a_reference_are_cleared_even_when_it_resolves_to_no_image() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();
    oi.block_on(oi.state.container_runtime.remove_image(REF, true))
        .unwrap();

    let (code, _) = oi
        .call("/images/remove", json!({ "reference": REF }))
        .unwrap_err();
    assert_eq!(code, "not_found");
    let pins = oi.call("/images/pins/list", json!({})).unwrap();
    assert!(pins["pins"].as_array().unwrap().is_empty(), "{pins}");
}

// spec: IMGM#image-discovery
#[test]
fn discover_returns_empty_probe_for_handlerless_app() {
    let oi = TestOi::with_app("demo");
    let result = oi
        .call("/apps/images/discover", json!({ "app": "demo" }))
        .unwrap();
    assert!(result["per_handler"].as_array().unwrap().is_empty());
    assert!(result["all_images"].as_array().unwrap().is_empty());

    let (code, _) = oi
        .call("/apps/images/discover", json!({ "app": "ghost" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: IMGM#listing-images
#[test]
fn an_unreadable_container_list_is_not_an_image_nothing_uses() {
    let oi = TestOi::new();
    oi.call("/images/pull", json!({ "reference": REF }))
        .unwrap();
    let bad = TestOi::with_unreadable_containers();

    let (code, _) = bad.call("/images/list", json!({})).unwrap_err();
    assert_eq!(code, "internal");
}

// spec: IMGM#removing-an-image
#[test]
fn a_remove_that_cannot_tell_whether_the_image_is_in_use_does_not_proceed() {
    let oi = TestOi::with_unreadable_containers();
    let (code, _) = oi
        .call("/images/remove", json!({ "reference": REF }))
        .unwrap_err();
    assert_eq!(code, "internal");
}

// spec: IMGM#image-pins
#[test]
fn an_unreadable_pin_table_is_an_error_not_no_pins() {
    let oi = TestOi::with_app("demo");
    oi.call("/images/pull", json!({ "reference": REF, "app": "demo" }))
        .unwrap();
    oi.state
        .db
        .call(|db| db.conn.execute_batch("DROP TABLE image_pins;").unwrap());

    let (code, _) = oi.call("/images/pins/list", json!({})).unwrap_err();
    assert_eq!(code, "internal");
    let (code, _) = oi.call("/images/list", json!({})).unwrap_err();
    assert_eq!(code, "internal");
}

// spec: IMGM#image-discovery
// spec: IMG#image-discovery
#[test]
fn discover_probes_a_gate_body_with_last_from_the_gates_record() {
    use seedling_protocol::names::{AppName, GateName};

    let script = r#"
        app.gate("migrate").key(1).run(|rt, last| {
            let image = if last == () { "ghcr.io/example/first:1" } else { "ghcr.io/example/again:1" };
            rt.warm_images(app.job().image(image)).ready();
        });
    "#;
    let oi = TestOi::new();
    oi.call("/apps/create", json!({ "app": "demo", "script": script }))
        .unwrap();

    let discover = || {
        let result = oi
            .call("/apps/images/discover", json!({ "app": "demo" }))
            .unwrap();
        let handlers = result["per_handler"].as_array().unwrap().clone();
        assert_eq!(handlers.len(), 1, "{result}");
        assert_eq!(handlers[0]["kind"], "gate");
        assert_eq!(handlers[0]["name"], "migrate");
        assert!(handlers[0]["error"].is_null(), "{result}");
        handlers[0]["images"].clone()
    };
    assert_eq!(discover(), json!(["ghcr.io/example/first:1"]));

    let name = AppName::new("demo").unwrap();
    let digest = {
        let registry = oi.state.registry.read();
        let def = registry.get("demo").unwrap().app.def.load_full();
        def.gates["migrate"]
            .key
            .digest(&crate::runtime::secrets::Cipher::for_tests())
            .unwrap()
    };
    oi.state.db.call(move |db| {
        let generation = crate::runtime::generations::current(db, &name)
            .unwrap()
            .unwrap_or(1);
        crate::runtime::gates::store::record_success(
            db,
            &name,
            &GateName::new("migrate").unwrap(),
            &digest,
            generation,
        )
        .unwrap();
    });
    assert_eq!(discover(), json!(["ghcr.io/example/again:1"]));
}

// spec: IMG#image-discovery
// spec: LIM#breakers
#[test]
fn discovery_reports_a_handler_whose_breaker_is_tripped_and_probes_the_rest() {
    use crate::runtime::breakers::{self, Run, Thing};
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        json!({
            "app": "demo",
            "script": r#"
                app.on_action("spin", |rt, p| { throw "evaluated"; });
                app.on_action("other", |rt, p| {
                    rt.warm_images(app.job().image("ghcr.io/example/other:1")).ready();
                });
            "#,
        }),
    )
    .unwrap();
    let run_of_spin = {
        let reg = oi.state.registry.read();
        Run::of_app(&reg.get("demo").unwrap().app, &oi.state.script_limits)
    };
    let cipher = std::sync::Arc::clone(&oi.state.cipher);
    let limits = oi.state.script_limits.clone();
    oi.state
        .db
        .call(move |db| {
            breakers::trip(
                db,
                &cipher,
                &seedling_protocol::names::AppName::new_unchecked("demo"),
                &Thing::Action(seedling_protocol::names::ActionName::new_unchecked("spin")),
                &run_of_spin,
                1,
                &limits,
            )
        })
        .unwrap();

    let result = oi
        .call("/apps/images/discover", json!({ "app": "demo" }))
        .unwrap();
    let handler = |name: &str| {
        result["per_handler"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no handler {name}: {result}"))
    };
    let spin = handler("spin");
    let why = spin["error"].as_str().unwrap_or_default();
    assert!(
        why.contains("ran past the evaluation time limit"),
        "{result}"
    );
    assert!(handler("other")["error"].is_null(), "{result}");
    assert_eq!(
        handler("other")["images"],
        json!(["ghcr.io/example/other:1"])
    );
}
