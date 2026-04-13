use std::sync::Arc;

use super::*;
use defs::resource::ResourceKind;

// l[verify volume.type]
#[test]
fn volume_named() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("data");
    "#,
    );
    let def = app.def.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Volume && &*id.name == "data")
    );
}

// l[verify volume.type]
#[test]
fn volume_anonymous_disallowed_at_top_level() {
    let (engine, mut scope, _app) = crate::setup_language();
    let result = super::run_script(&engine, &mut scope, r#"let v = app.volume();"#);
    assert!(
        result.is_err(),
        "anonymous volume at top level should error"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("action closures"),
        "error should mention action closures, got: {err}"
    );
}

// l[verify volume.readonly]
#[test]
fn volume_readonly() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg").readonly();
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        assert!(vol.def.lock().read_only);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.write]
#[test]
fn volume_write() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg");
        v.write("/app.conf", "key=value");
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        let vol_def = vol.def.lock();
        assert_eq!(vol_def.writes.len(), 1);
        assert_eq!(vol_def.writes[0], ("/app.conf".into(), "key=value".into()));
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_path_traversal() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("/../etc/passwd", "evil");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_dotdot_escape() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("/sub/../../escape", "evil");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_relative_path() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("relative.conf", "data");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_null_bytes() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("/app\0.conf", "data");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_root() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("/", "data");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_rejects_dotdot_to_root() {
    let _ = run_test_script_err(
        r#"
        let v = app.volume("cfg");
        v.write("/foo/..", "data");
    "#,
    );
}

// l[verify volume.write.validation]
#[test]
fn volume_write_accepts_nested_paths() {
    run_test_script_app(
        r#"
        let v = app.volume("cfg");
        v.write("/app.conf", "key=value");
        v.write("/sub/dir/file.txt", "nested");
        v.write("/a/b/c/d.yaml", "deep");
    "#,
    );
}

// l[verify volume.write]
#[test]
fn volume_write_multiple() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg");
        v.write("/a.conf", "aaa");
        v.write("/b.conf", "bbb");
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        let vol_def = vol.def.lock();
        assert_eq!(vol_def.writes.len(), 2);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.external]
#[test]
fn external_volume_creates_resource() {
    let app = run_test_script_app(
        r#"
        let v = app.external_volume("pg-socket");
    "#,
    );
    let def = app.def.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::ExternalVolume && &*id.name == "pg-socket")
    );
}

// l[verify volume.external]
#[test]
fn external_volume_can_be_mounted() {
    run_test_script_app(
        r#"
        let evol = app.external_volume("shared");
        app.deployment("web")
            .image("nginx")
            .mount("/shared", evol);
    "#,
    );
}

// ---------------------------------------------------------------------------
// Anonymous volume naming in action closures
// ---------------------------------------------------------------------------

fn run_action_with_volumes(
    script: &str,
    action_name: &str,
) -> crate::runtime::desired::OperationProgress {
    use crate::runtime::{
        EphemeralInstanceRegistry, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
    };

    let (engine, mut scope, app, ast) = run_test_script(script);

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());
    let progress = Arc::new(parking_lot::RwLock::new(None));

    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name,
            log: &log,
            world: oracle,
            registry,
            active_progress: Some(Arc::clone(&progress)),
            tick_notify: None,
            install_requirements: None,
            is_shell: false,
            db: None,
        },
        &mut scope,
    );

    // The action should complete (no barriers in these tests).
    assert!(
        matches!(result, OperationResult::Completed),
        "expected Completed, got {result:?}"
    );

    progress.read().clone().expect("progress should be set")
}

// l[verify app.resources.context.anonymous]
#[test]
fn anon_volume_in_action_gets_seedling_prefix() {
    let progress = run_action_with_volumes(
        r#"
        app.on_action("goo", |rt| {
            let vol = app.volume();
            let j = app.job().image("busybox").mount("/data", vol);
            rt.start(j);
        });
    "#,
        "goo",
    );

    // The dynamic defs should contain the anonymous job.
    assert!(
        !progress.dynamic_defs.is_empty(),
        "expected dynamic defs from anonymous job"
    );

    // Check that the anonymous volume got a seedling-anon- prefixed name
    // by examining the job's container spec through the captured resource.
    for (_inst, resource) in &progress.dynamic_defs {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for (_path, vm) in &container.volume_mounts {
                if let defs::container::VolumeMount::Volume(v) = vm {
                    assert!(v.name.is_none(), "anonymous volume should have no BSL name");
                    let anon_id = v
                        .anon_id
                        .as_ref()
                        .expect("anonymous volume should have anon_id");
                    assert!(
                        anon_id.starts_with("seedling-anon-"),
                        "anon_id should start with seedling-anon-, got: {anon_id}"
                    );
                }
            }
        }
    }
}

// l[verify app.resources.context.anonymous]
#[test]
fn shared_anon_volume_same_id_across_containers() {
    let progress = run_action_with_volumes(
        r#"
        app.on_action("goo", |rt| {
            let vol = app.volume();
            vol.write("/config.txt", "hello");
            let j1 = app.job().image("busybox").mount("/a", vol);
            let j2 = app.job().image("busybox").mount("/b", vol);
            rt.start(j1);
            rt.start(j2);
        });
    "#,
        "goo",
    );

    // Collect the anon_ids from all volume mounts across all dynamic resources.
    let mut anon_ids: Vec<String> = Vec::new();
    for (_inst, resource) in &progress.dynamic_defs {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for (_path, vm) in &container.volume_mounts {
                if let defs::container::VolumeMount::Volume(v) = vm {
                    if let Some(id) = &v.anon_id {
                        anon_ids.push(id.clone());
                    }
                }
            }
        }
    }

    assert_eq!(
        anon_ids.len(),
        2,
        "expected 2 volume mounts, got {anon_ids:?}"
    );
    assert_eq!(
        anon_ids[0], anon_ids[1],
        "same Volume object mounted on different containers should have the same anon_id"
    );
}

// l[verify app.resources.context.anonymous]
#[test]
fn distinct_anon_volumes_get_distinct_ids() {
    let progress = run_action_with_volumes(
        r#"
        app.on_action("goo", |rt| {
            let vol1 = app.volume();
            let vol2 = app.volume();
            let j = app.job().image("busybox").mount("/a", vol1).mount("/b", vol2);
            rt.start(j);
        });
    "#,
        "goo",
    );

    let mut anon_ids: Vec<String> = Vec::new();
    for (_inst, resource) in &progress.dynamic_defs {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for (_path, vm) in &container.volume_mounts {
                if let defs::container::VolumeMount::Volume(v) = vm {
                    if let Some(id) = &v.anon_id {
                        anon_ids.push(id.clone());
                    }
                }
            }
        }
    }

    assert_eq!(
        anon_ids.len(),
        2,
        "expected 2 volume mounts, got {anon_ids:?}"
    );
    assert_ne!(
        anon_ids[0], anon_ids[1],
        "different Volume objects should get different anon_ids"
    );
}

// l[verify app.resources.context.anonymous]
#[test]
fn anon_volume_writes_preserved_through_action() {
    let progress = run_action_with_volumes(
        r#"
        app.on_action("goo", |rt| {
            let vol = app.volume();
            vol.write("/init.sql", "CREATE TABLE t;");
            vol.write("/seed.sql", "INSERT INTO t VALUES (1);");
            let j = app.job().image("busybox").mount("/docker-entrypoint-initdb.d", vol);
            rt.start(j);
        });
    "#,
        "goo",
    );

    let mut found_writes = false;
    for (_inst, resource) in &progress.dynamic_defs {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for (_path, vm) in &container.volume_mounts {
                if let defs::container::VolumeMount::Volume(v) = vm {
                    let vol_def = v.def.lock();
                    assert_eq!(vol_def.writes.len(), 2);
                    assert_eq!(vol_def.writes[0].0, "/init.sql");
                    assert_eq!(vol_def.writes[0].1, "CREATE TABLE t;");
                    assert_eq!(vol_def.writes[1].0, "/seed.sql");
                    assert_eq!(vol_def.writes[1].1, "INSERT INTO t VALUES (1);");
                    found_writes = true;
                }
            }
        }
    }
    assert!(found_writes, "should have found volume with writes");
}

// l[verify app.resources.context.named]
#[test]
fn frozen_static_volume_cannot_be_modified_in_action() {
    use crate::runtime::{
        EphemeralInstanceRegistry, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
    };

    let (engine, mut scope, app, ast) = run_test_script(
        r#"
        let v = app.volume("data");
        app.on_action("goo", |rt| {
            app.volume("data").write("/x", "y");
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let op = OperationId::new();
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());

    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "goo",
            log: &log,
            world: oracle,
            registry,
            active_progress: None,
            tick_notify: None,
            install_requirements: None,
            is_shell: false,
            db: None,
        },
        &mut scope,
    );

    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("cannot modify") || msg.contains("static"),
                "error should mention immutability, got: {msg}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}
