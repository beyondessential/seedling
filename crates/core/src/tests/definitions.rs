use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use seedling_protocol::names::{AppName, GateName};

use crate::defs::{
    self,
    gate::AfterTarget,
    resource::{ResourceId, ResourceKind},
};
use crate::runtime::{
    EphemeralInstanceRegistry,
    definition::Bundle,
    probe::{ProbeRequest, ProbeResponse, probe_app},
};

const POSTGRES: &str = include_str!("../../../../apps/postgres.seed.rhai");
const CENTRAL: &str = include_str!("../../../../apps/tamanu-central.seed.rhai");
const FACILITY: &str = include_str!("../../../../apps/tamanu-facility.seed.rhai");

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn postgres_params(version: &str) -> BTreeMap<String, String> {
    params(&[("version", version), ("password", "hunter2")])
}

fn central_params() -> BTreeMap<String, String> {
    params(&[
        ("public-hostname", "central.example.org"),
        ("patient-portal-fqdn", "portal.example.org"),
        ("fhir-enabled", "true"),
        ("auth-secret", "a"),
        ("refresh-secret", "b"),
    ])
}

fn facility_params() -> BTreeMap<String, String> {
    params(&[
        ("public-hostname", "facility.example.org"),
        ("central-url", "https://central.example.org"),
        ("facility-id", "facility-1"),
        ("sync-password", "pw"),
        ("auth-secret", "a"),
    ])
}

fn evaluated(source: &str, params: &BTreeMap<String, String>) -> defs::app::App {
    let (app, err) = crate::runtime::apps::evaluate_script(
        &AppName::new_unchecked("demo"),
        source,
        params,
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "the definition evaluates: {err:?}");
    app
}

fn loaded(
    source: &str,
    params: &BTreeMap<String, String>,
) -> (rhai::Engine, rhai::AST, defs::app::App) {
    let (engine, mut scope, app) = crate::setup_language(
        &crate::ScriptLimits::default(),
        Arc::new(Bundle::from_stored_script(source)),
    )
    .expect("the scope builds");
    *app.stored.lock() = params.clone();
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        d.name = AppName::new_unchecked("demo");
        d
    });
    defs::app::set_appdef_holder(&app.def);
    let ast = engine.compile(source).expect("the definition compiles");
    engine
        .run_ast_with_scope(&mut scope, &ast)
        .expect("the definition runs");
    (engine, ast, app)
}

fn probe_gates(
    source: &str,
    now: &BTreeMap<String, String>,
    last: Option<&BTreeMap<String, String>>,
) -> ProbeResponse {
    let (engine, ast, app) = loaded(source, now);
    let last = last.map(|p| loaded(source, p).2);
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());
    let response = probe_app(
        &engine,
        &ast,
        &app,
        registry,
        &ProbeRequest {
            lenient: true,
            ..Default::default()
        },
        &move |_| Ok(last.clone()),
    )
    .expect("the probe runs");
    for handler in &response.per_handler {
        assert!(handler.error.is_none(), "{handler:?}");
    }
    response
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

// spec: GATE#declaration
#[test]
fn the_postgres_definition_declares_its_gates() {
    let app = evaluated(POSTGRES, &postgres_params("17.2"));
    assert_eq!(
        declared(&app),
        ["config", "major", "password", "analyze", "bestool-psql"]
    );
    let def = app.def.load();

    let major = &def.gates["major"];
    assert_eq!(major.exclusive, deployments(&["postgres"]));
    let check = major.check.as_ref().expect("the major gate has a check");
    assert_eq!(check.job, workload(ResourceKind::Job, "data-check"));
    assert_eq!(check.interval, Some(std::time::Duration::from_secs(600)));

    assert_eq!(
        def.gates["password"].after,
        BTreeSet::from([AfterTarget::Workload(workload(
            ResourceKind::Deployment,
            "postgres"
        ))])
    );
    assert!(def.gates["password"].exclusive.is_empty());

    assert_eq!(
        deployment_after(&def, "postgres"),
        after_gates(&["config", "major"])
    );
}

// spec: GATE#keys
#[test]
fn the_postgres_gates_move_with_what_they_converge_on() {
    let key = |p: &BTreeMap<String, String>, gate_name: &str| {
        evaluated(POSTGRES, p).def.load().gates[gate_name]
            .key
            .clone()
    };
    let base = postgres_params("17.2");
    let minor = postgres_params("17.3");
    let major = postgres_params("18.0");
    let mut rotated = base.clone();
    rotated.insert("password".into(), "correct horse".into());
    let mut tuned = base.clone();
    tuned.insert("max-connections".into(), "200".into());

    assert_eq!(key(&base, "major"), key(&minor, "major"));
    assert_ne!(key(&base, "major"), key(&major, "major"));
    assert_ne!(key(&base, "password"), key(&rotated, "password"));
    assert_eq!(key(&base, "major"), key(&rotated, "major"));
    assert_ne!(key(&base, "config"), key(&tuned, "config"));
    assert_eq!(key(&base, "password"), key(&tuned, "password"));
}

// spec: IMG#image-discovery
#[test]
fn the_postgres_gate_bodies_run_under_a_probe_first_and_on_upgrade() {
    let first = probe_gates(POSTGRES, &postgres_params("17.2"), None);
    let initdb = BTreeSet::from(["docker.io/library/postgres:17".to_owned()]);
    assert_eq!(gate_probe(&first, "major"), &initdb);
    assert!(
        gate_probe(&first, "analyze").is_empty(),
        "nothing to analyze on a first run"
    );

    let upgrade = probe_gates(
        POSTGRES,
        &postgres_params("18.0"),
        Some(&postgres_params("16.4")),
    );
    assert_eq!(
        gate_probe(&upgrade, "major"),
        &BTreeSet::from([
            "docker.io/library/postgres:17".to_owned(),
            "docker.io/library/postgres:18".to_owned(),
            "docker.io/tianon/postgres-upgrade:16-to-17".to_owned(),
            "docker.io/tianon/postgres-upgrade:17-to-18".to_owned(),
        ])
    );
    assert!(gate_probe(&upgrade, "analyze").contains("docker.io/library/postgres:18.0"));
}

// spec: GATE#body
#[test]
fn a_postgres_downgrade_fails_the_major_gate_body() {
    let (engine, ast, app) = loaded(POSTGRES, &postgres_params("16.4"));
    let (_, _, last) = loaded(POSTGRES, &postgres_params("17.2"));
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());
    let response = probe_app(
        &engine,
        &ast,
        &app,
        registry,
        &ProbeRequest::default(),
        &move |_| Ok(Some(last.clone())),
    )
    .expect("the probe runs");
    let major = response
        .per_handler
        .iter()
        .find(|h| h.name == "major")
        .expect("the major gate is probed");
    assert!(
        major
            .error
            .as_deref()
            .is_some_and(|e| e.contains("refusing to downgrade from PostgreSQL 17 to 16")),
        "{major:?}"
    );
}

// spec: GATE#declaration
#[test]
fn the_central_definition_declares_its_gates() {
    let app = evaluated(CENTRAL, &central_params());
    assert_eq!(
        declared(&app),
        ["config", "database", "migrate", "certificates"]
    );
    let def = app.def.load();

    let migrate = &def.gates["migrate"];
    assert_eq!(migrate.after, after_gates(&["config", "database"]));
    assert_eq!(
        migrate.exclusive,
        deployments(&["api", "tasks", "fhir-refresh", "fhir-resolve"])
    );
    assert_eq!(
        def.gates["database"].check.as_ref().expect("a check").job,
        workload(ResourceKind::Job, "database-check")
    );

    let after = |name: &str| deployment_after(&def, name);
    for part in ["api", "tasks", "fhir-refresh", "fhir-resolve"] {
        assert_eq!(after(part), after_gates(&["config", "migrate"]), "{part}");
    }
    assert_eq!(after("patient-portal"), after_gates(&["config", "migrate"]));
    assert_eq!(after("frontend"), after_gates(&["migrate"]));
}

// spec: GATE#declaration
#[test]
fn the_central_migration_leaves_out_the_workers_it_does_not_declare() {
    let mut p = central_params();
    p.remove("fhir-enabled");
    p.remove("patient-portal-fqdn");
    let app = evaluated(CENTRAL, &p);
    let def = app.def.load();
    assert_eq!(
        def.gates["migrate"].exclusive,
        deployments(&["api", "tasks"])
    );
}

// spec: GATE#keys
#[test]
fn the_central_gates_move_with_what_they_converge_on() {
    let key = |p: &BTreeMap<String, String>, gate_name: &str| {
        evaluated(CENTRAL, p).def.load().gates[gate_name]
            .key
            .clone()
    };
    let base = central_params();
    let mut upgraded = base.clone();
    upgraded.insert("version".into(), "v2.60.0".into());
    let mut retimed = base.clone();
    retimed.insert("timezone".into(), "Pacific/Auckland".into());

    assert_ne!(key(&base, "migrate"), key(&upgraded, "migrate"));
    assert_eq!(key(&base, "config"), key(&upgraded, "config"));
    assert_ne!(key(&base, "config"), key(&retimed, "config"));
    assert_eq!(key(&base, "migrate"), key(&retimed, "migrate"));
}

// spec: IMG#image-discovery
#[test]
fn the_central_gate_bodies_and_actions_run_under_a_probe() {
    let probed = probe_gates(CENTRAL, &central_params(), None);
    assert!(
        gate_probe(&probed, "migrate").contains("ghcr.io/beyondessential/tamanu-central:v2.53.3")
    );
    assert!(
        probed
            .per_handler
            .iter()
            .any(|h| h.name == "provision-admin")
    );
}

// spec: GATE#declaration
#[test]
fn the_facility_definition_declares_its_gates() {
    let app = evaluated(FACILITY, &facility_params());
    assert_eq!(
        declared(&app),
        ["config", "database", "migrate", "certificates"]
    );
    let def = app.def.load();

    let migrate = &def.gates["migrate"];
    assert_eq!(migrate.after, after_gates(&["config", "database"]));
    assert_eq!(migrate.exclusive, deployments(&["api", "tasks", "sync"]));

    let after = |name: &str| deployment_after(&def, name);
    for part in ["api", "tasks", "sync"] {
        assert_eq!(after(part), after_gates(&["config", "migrate"]), "{part}");
    }
    assert_eq!(after("frontend"), after_gates(&["migrate"]));
}

// spec: GATE#keys
#[test]
fn the_facility_gates_move_with_what_they_converge_on() {
    let key = |p: &BTreeMap<String, String>, gate_name: &str| {
        evaluated(FACILITY, p).def.load().gates[gate_name]
            .key
            .clone()
    };
    let base = facility_params();
    let mut upgraded = base.clone();
    upgraded.insert("version".into(), "v2.60.0".into());
    let mut renamed = base.clone();
    renamed.insert("facility-id".into(), "facility-2".into());

    assert_ne!(key(&base, "migrate"), key(&upgraded, "migrate"));
    assert_eq!(key(&base, "config"), key(&upgraded, "config"));
    assert_ne!(key(&base, "config"), key(&renamed, "config"));
    assert_eq!(key(&base, "migrate"), key(&renamed, "migrate"));
}

// spec: IMG#image-discovery
#[test]
fn the_facility_gate_bodies_run_under_a_probe() {
    let probed = probe_gates(FACILITY, &facility_params(), None);
    assert!(
        gate_probe(&probed, "migrate").contains("ghcr.io/beyondessential/tamanu-facility:v2.53.3")
    );
}

// spec: PRM#validation
#[test]
fn the_tamanu_definitions_refuse_a_database_or_role_name_that_needs_quoting() {
    for (source, base) in [(CENTRAL, central_params()), (FACILITY, facility_params())] {
        let bundle = Arc::new(Bundle::from_stored_script(source));
        let rejected = |name: &str, value: &str| {
            let mut proposed = base.clone();
            proposed.insert(name.to_owned(), value.to_owned());
            // A bad name fails the evaluation itself, so the validators
            // are those of the last evaluation that succeeded.
            let (_, outcome) = crate::runtime::apps::evaluate_validated(
                &AppName::new_unchecked("demo"),
                &bundle,
                &base,
                &proposed,
                &crate::runtime::seeds::AppSeed::for_tests(),
                &crate::ScriptLimits::default(),
            );
            outcome
                .expect("the definition evaluates")
                .into_iter()
                .any(|r| r.name == name)
        };
        for name in ["db-name", "db-user"] {
            assert!(!rejected(name, "tamanu_2"), "{name}: a plain name");
            for bad in ["$(reboot)", "a\"b", "a'b", "a`b`", "a b", "A", ""] {
                assert!(rejected(name, bad), "{name}: {bad:?} is refused");
            }
        }
    }
}

// spec: GATE#keys
// A definition whose required parameters are set evaluates without an
// advertised URL, so the config gate's run, not the evaluation, reports it.
#[test]
fn the_tamanu_definitions_evaluate_with_no_hostname_or_canonical_url() {
    for (name, source, mut params) in [
        ("tamanu-central", CENTRAL, central_params()),
        ("tamanu-facility", FACILITY, facility_params()),
    ] {
        params.remove("public-hostname");
        params.remove("canonical-url");
        let (_, err) = crate::runtime::apps::evaluate_script(
            &AppName::new_unchecked("demo"),
            source,
            &params,
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "{name}: {err:?}");
    }
}

// spec: PRM#validation
// A name stored without passing the validator is checked again where it is
// written out, so the definition fails to evaluate rather than writing it.
#[test]
fn the_tamanu_definitions_do_not_write_out_a_database_name_that_needs_quoting() {
    for (name, source, mut params) in [
        ("tamanu-central", CENTRAL, central_params()),
        ("tamanu-facility", FACILITY, facility_params()),
    ] {
        params.insert("db-name".into(), "x\"; DROP DATABASE postgres; --".into());
        let (_, err) = crate::runtime::apps::evaluate_script(
            &AppName::new_unchecked("demo"),
            source,
            &params,
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_some(), "{name}");
    }
}
