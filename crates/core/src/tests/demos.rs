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
