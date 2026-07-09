use std::sync::Arc;

use seedling_protocol::names::AppName;
use tokio::sync::Notify;

use super::*;
use crate::runtime::apps::{AppEntry, AppPhase, evaluate_script};
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn entry_with_script(name: &str, script: &str) -> AppEntry {
    let (app_def, err) = evaluate_script(
        &app(name),
        script,
        &std::collections::BTreeMap::new(),
        &crate::ScriptLimits::default(),
    );
    assert!(err.is_none(), "script error: {err:?}");
    AppEntry {
        name: app(name),
        script: script.to_owned(),
        app: app_def,
        phase: Arc::new(parking_lot::Mutex::new(AppPhase::NotInstalled)),
        active_progress: Arc::new(parking_lot::RwLock::new(None)),
        tick_notify: Arc::new(Notify::new()),
        script_error: None,
        current_generation: 0,
    }
}

fn active_registry_faults(db: &Db, name: &str) -> Vec<crate::runtime::faults::FaultRecord> {
    crate::runtime::faults::list_active_faults(db, Some(&app(name)))
        .expect("list")
        .into_iter()
        .filter(|f| f.kind == FAULT_KIND)
        .collect()
}

// l[verify container.image.registry-allowlist]
#[test]
fn disallowed_registry_files_fault_naming_the_registry() {
    let db = Db::open_in_memory().expect("open");
    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("evil.example.com/foo:1");"#,
    );

    sync_registry_faults(&db, &entry);

    let faults = active_registry_faults(&db, "myapp");
    assert_eq!(faults.len(), 1);
    assert!(
        faults[0].description.contains("evil.example.com"),
        "description should name the registry: {}",
        faults[0].description
    );
}

// l[verify container.image.registry-allowlist]
#[test]
fn allowed_registry_files_no_fault() {
    let db = Db::open_in_memory().expect("open");
    // docker.io is seeded into the allowlist by migration.
    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("docker.io/library/nginx:latest");"#,
    );

    sync_registry_faults(&db, &entry);

    assert!(active_registry_faults(&db, "myapp").is_empty());
}

// l[verify container.image.registry-allowlist]
#[test]
fn sync_is_idempotent_for_unchanged_disallowed_set() {
    let db = Db::open_in_memory().expect("open");
    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("evil.example.com/foo:1");"#,
    );

    sync_registry_faults(&db, &entry);
    let first = active_registry_faults(&db, "myapp");
    assert_eq!(first.len(), 1);

    sync_registry_faults(&db, &entry);
    let second = active_registry_faults(&db, "myapp");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, first[0].id, "same fault must be kept");
}

// l[verify container.image.registry-allowlist]
#[test]
fn fault_clears_once_registry_becomes_allowed() {
    let db = Db::open_in_memory().expect("open");
    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("private.example.com/foo:1");"#,
    );

    sync_registry_faults(&db, &entry);
    assert_eq!(active_registry_faults(&db, "myapp").len(), 1);

    crate::runtime::registries::add_allowed_registry(&db, "private.example.com").expect("allow");
    sync_registry_faults(&db, &entry);

    assert!(active_registry_faults(&db, "myapp").is_empty());
}

// l[verify container.image.registry-allowlist]
#[test]
fn changed_disallowed_set_replaces_the_fault() {
    let db = Db::open_in_memory().expect("open");

    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("one.example.com/foo:1");"#,
    );
    sync_registry_faults(&db, &entry);
    let first = active_registry_faults(&db, "myapp");
    assert_eq!(first.len(), 1);

    let entry = entry_with_script(
        "myapp",
        r#"app.deployment("web").image("two.example.com/foo:1");"#,
    );
    sync_registry_faults(&db, &entry);
    let second = active_registry_faults(&db, "myapp");
    assert_eq!(second.len(), 1, "stale fault must be replaced, not stacked");
    assert_ne!(second[0].id, first[0].id);
    assert!(second[0].description.contains("two.example.com"));
}
