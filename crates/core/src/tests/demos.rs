use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use seedling_bundle::Bundle;
use seedling_protocol::names::{AppName, GateName};

use crate::defs::{
    self,
    gate::AfterTarget,
    resource::{ResourceId, ResourceKind},
};
use crate::runtime::{
    EphemeralInstanceRegistry,
    probe::{ProbeRequest, ProbeResponse, probe_app},
};

/// The folder of shared files the demos include, which is not a demo.
const SHARED: &str = "shared";

fn demos_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demos")
}

fn demo_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(demos_dir())
        .expect("the demos folder reads")
        .map(|entry| entry.expect("a demos entry reads"))
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| {
            entry
                .file_name()
                .into_string()
                .expect("a demo's name is UTF-8")
        })
        .filter(|name| name != SHARED)
        .collect();
    names.sort();
    names
}

/// A demo's bundle, assembled as the CLI assembles a folder it pushes.
fn demo(name: &str) -> Arc<Bundle> {
    let files = seedling_bundle::source::read_folder_definition(&demos_dir().join(name))
        .unwrap_or_else(|e| panic!("{name} reads as a definition folder: {e}"));
    Arc::new(Bundle::from_files(files).unwrap_or_else(|e| panic!("{name} is a bundle: {e}")))
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// The parameters a demo needs set to install: none, except where a demo
/// exists to show a required parameter.
fn install_params(name: &str) -> BTreeMap<String, String> {
    match name {
        "params-tour" => params(&[("contact", "ops@demo.test")]),
        _ => BTreeMap::new(),
    }
}

fn evaluated(name: &str, params: &BTreeMap<String, String>) -> defs::app::App {
    let (app, err) = crate::runtime::apps::evaluate(
        &AppName::new_unchecked(name),
        &demo(name),
        params,
        &crate::runtime::seeds::AppSeed::for_tests(),
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "{name} evaluates: {err:?}");
    app
}

fn loaded(
    name: &str,
    params: &BTreeMap<String, String>,
) -> (rhai::Engine, rhai::AST, defs::app::App) {
    let bundle = demo(name);
    let (engine, mut scope, mut app) =
        crate::setup_language(&crate::ScriptLimits::default(), Arc::clone(&bundle))
            .expect("the scope builds");
    app.seed = Some(crate::runtime::seeds::AppSeed::for_tests());
    *app.stored.lock() = params.clone();
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        d.name = AppName::new_unchecked(name);
        d
    });
    defs::app::set_appdef_holder(&app.def);
    let script = bundle.script().expect("the bundle holds a script");
    let ast = crate::defs::compile(&engine, script.text()).expect("the definition compiles");
    engine
        .run_ast_with_scope(&mut scope, &ast)
        .expect("the definition runs");
    defs::app::clear_appdef_holder();
    (engine, ast, app)
}

fn probe(
    name: &str,
    now: &BTreeMap<String, String>,
    last: Option<&BTreeMap<String, String>>,
    lenient: bool,
) -> ProbeResponse {
    let (engine, ast, app) = loaded(name, now);
    let last = last.map(|p| loaded(name, p).2);
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());
    probe_app(
        &engine,
        &ast,
        &app,
        registry,
        &ProbeRequest {
            lenient,
            ..Default::default()
        },
        &move |_| Ok(last.clone()),
    )
    .expect("the probe runs")
}

fn gate_probe<'r>(response: &'r ProbeResponse, name: &str) -> &'r BTreeSet<String> {
    &response
        .per_handler
        .iter()
        .find(|h| h.name == name && h.kind == crate::runtime::probe::HandlerKind::Gate)
        .unwrap_or_else(|| panic!("gate {name} has a body that is probed"))
        .images
}

fn gate(name: &str) -> GateName {
    GateName::new_unchecked(name)
}

fn workload(kind: ResourceKind, name: &str) -> ResourceId {
    ResourceId {
        kind,
        name: Arc::new(name.to_owned()),
    }
}

fn deployments(names: &[&str]) -> BTreeSet<ResourceId> {
    names
        .iter()
        .map(|n| workload(ResourceKind::Deployment, n))
        .collect()
}

fn after_gates(names: &[&str]) -> BTreeSet<AfterTarget> {
    names.iter().map(|n| AfterTarget::Gate(gate(n))).collect()
}

fn deployment_after(def: &defs::app::AppDef, name: &str) -> BTreeSet<AfterTarget> {
    defs::gate::workload_after(&def.resources[&workload(ResourceKind::Deployment, name)])
        .expect("a deployment is a workload")
}

fn declared(app: &defs::app::App) -> Vec<String> {
    let def = app.def.load();
    let mut gates: Vec<_> = def.gates.iter().collect();
    gates.sort_by_key(|(_, g)| g.declaration);
    gates.into_iter().map(|(n, _)| n.to_string()).collect()
}

#[test]
fn every_demo_evaluates_with_only_the_params_it_requires() {
    let names = demo_names();
    assert!(names.iter().any(|n| n == "hello-world"), "{names:?}");
    for name in names {
        evaluated(&name, &install_params(&name));
    }
}

#[test]
fn every_demo_says_it_is_a_demo() {
    for name in demo_names() {
        let app = evaluated(&name, &install_params(&name));
        let description = app.def.load().description.clone();
        assert!(
            description
                .as_deref()
                .is_some_and(|d| d.starts_with("Demo: ")),
            "{name}: {description:?}"
        );
    }
}

#[test]
fn every_demo_action_and_gate_body_runs_under_a_probe() {
    for name in demo_names() {
        let response = probe(&name, &install_params(&name), None, true);
        for handler in &response.per_handler {
            assert!(handler.error.is_none(), "{name}: {handler:?}");
        }
    }
}

fn gates_params(format: &str, greeting: &str) -> BTreeMap<String, String> {
    params(&[("format", format), ("greeting", greeting)])
}

// spec: GATE#declaration
#[test]
fn the_actions_and_gates_demo_declares_its_gates() {
    let app = evaluated("actions-and-gates", &BTreeMap::new());
    assert_eq!(declared(&app), ["migrate", "render", "verify", "announce"]);
    let def = app.def.load();

    let migrate = &def.gates["migrate"];
    assert!(migrate.has_body);
    assert_eq!(migrate.exclusive, deployments(&["web"]));
    let check = migrate.check.as_ref().expect("migrate has a check");
    assert_eq!(check.job, workload(ResourceKind::Job, "store-check"));
    assert_eq!(check.interval, Some(std::time::Duration::from_secs(600)));

    assert_eq!(def.gates["render"].after, after_gates(&["migrate"]));
    assert!(def.gates["render"].exclusive.is_empty());

    let verify = &def.gates["verify"];
    assert!(!verify.has_body, "verify waits on its check alone");
    assert_eq!(verify.after, after_gates(&["migrate", "render"]));
    let check = verify.check.as_ref().expect("verify has a check");
    assert_eq!(check.job, workload(ResourceKind::Job, "verify-check"));
    assert_eq!(check.interval, None);

    assert_eq!(
        def.gates["announce"].after,
        BTreeSet::from([AfterTarget::Workload(workload(
            ResourceKind::Deployment,
            "web"
        ))])
    );
    assert_eq!(
        deployment_after(&def, "web"),
        after_gates(&["render", "verify"])
    );
}

// spec: GATE#keys
#[test]
fn the_actions_and_gates_demo_gates_move_with_what_they_converge_on() {
    let key = |p: &BTreeMap<String, String>, gate_name: &str| {
        evaluated("actions-and-gates", p).def.load().gates[gate_name]
            .key
            .clone()
    };
    let base = gates_params("2", "Kia ora");
    let stepped = gates_params("3", "Kia ora");
    let greeted = gates_params("2", "Hello");

    assert_ne!(key(&base, "migrate"), key(&stepped, "migrate"));
    assert_eq!(key(&base, "render"), key(&stepped, "render"));
    assert_ne!(key(&base, "render"), key(&greeted, "render"));
    assert_eq!(key(&base, "migrate"), key(&greeted, "migrate"));
    assert_eq!(key(&base, "verify"), key(&greeted, "verify"));
}

// spec: IMG#image-discovery
#[test]
fn the_migrate_gate_body_initialises_first_and_steps_up_after() {
    let first = probe("actions-and-gates", &gates_params("2", "Kia ora"), None, true);
    assert_eq!(
        gate_probe(&first, "migrate"),
        &BTreeSet::from(["docker.io/library/busybox:1.38.0-musl".to_owned()])
    );

    let later = probe(
        "actions-and-gates",
        &gates_params("3", "Kia ora"),
        Some(&gates_params("1", "Kia ora")),
        true,
    );
    assert_eq!(
        gate_probe(&later, "migrate"),
        &BTreeSet::from(["docker.io/library/alpine:3.24.4".to_owned()])
    );
}

// spec: GATE#body
#[test]
fn stepping_the_store_down_fails_the_migrate_gate_body() {
    let response = probe(
        "actions-and-gates",
        &gates_params("1", "Kia ora"),
        Some(&gates_params("3", "Kia ora")),
        false,
    );
    let migrate = response
        .per_handler
        .iter()
        .find(|h| h.name == "migrate")
        .expect("the migrate gate is probed");
    assert!(
        migrate
            .error
            .as_deref()
            .is_some_and(|e| e.contains("refusing to step the store down from format 3 to 1")),
        "{migrate:?}"
    );
}
