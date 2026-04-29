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
    let def = app.def.load();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Volume && &*id.name == "data")
    );
}

// l[verify volume.type]
#[test]
fn volume_anonymous_disallowed_at_top_level() {
    let (engine, mut scope, _app) = crate::setup_language(&crate::ScriptLimits::default());
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

// l[verify volume.tmpfs]
#[test]
fn volume_tmpfs() {
    let app = run_test_script_app(r#"let v = app.volume("cache").tmpfs();"#);
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cache")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        assert!(vol.def.lock().tmpfs);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.tmpfs]
#[test]
fn volume_tmpfs_defaults_false() {
    let app = run_test_script_app(r#"let v = app.volume("data");"#);
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "data")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        assert!(!vol.def.lock().tmpfs);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.exported]
#[test]
fn volume_exported_marks_export() {
    let app = run_test_script_app(r#"let v = app.volume("pub").exported();"#);
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "pub")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        assert!(vol.def.lock().exported.is_some());
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.exported]
#[test]
fn volume_exported_with_description() {
    let app = run_test_script_app(
        r#"let v = app.volume("pub").exported(#{ description: "public data" });"#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "pub")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        let vdef = vol.def.lock();
        let export = vdef.exported.as_ref().expect("should be exported");
        assert_eq!(export.description.as_deref(), Some("public data"));
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.readonly]
#[test]
fn volume_readonly() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg").readonly();
    "#,
    );
    let def = app.def.load();
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
    let def = app.def.load();
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
    let def = app.def.load();
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
    let def = app.def.load();
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
            .image("docker.io/library/nginx:latest")
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
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
        app.on_action("goo", |rt, _param| {
            let vol = app.volume();
            let j = app.job().image("docker.io/library/busybox:latest").mount("/data", vol);
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
    for resource in progress.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
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
        app.on_action("goo", |rt, _param| {
            let vol = app.volume();
            vol.write("/config.txt", "hello");
            let j1 = app.job().image("docker.io/library/busybox:latest").mount("/a", vol);
            let j2 = app.job().image("docker.io/library/busybox:latest").mount("/b", vol);
            rt.start(j1);
            rt.start(j2);
        });
    "#,
        "goo",
    );

    // Collect the anon_ids from all volume mounts across all dynamic resources.
    let mut anon_ids: Vec<String> = Vec::new();
    for resource in progress.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
                if let defs::container::VolumeMount::Volume(v) = vm
                    && let Some(id) = &v.anon_id
                {
                    anon_ids.push(id.clone());
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
        app.on_action("goo", |rt, _param| {
            let vol1 = app.volume();
            let vol2 = app.volume();
            let j = app.job().image("docker.io/library/busybox:latest").mount("/a", vol1).mount("/b", vol2);
            rt.start(j);
        });
    "#,
        "goo",
    );

    let mut anon_ids: Vec<String> = Vec::new();
    for resource in progress.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
                if let defs::container::VolumeMount::Volume(v) = vm
                    && let Some(id) = &v.anon_id
                {
                    anon_ids.push(id.clone());
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
        app.on_action("goo", |rt, _param| {
            let vol = app.volume();
            vol.write("/init.sql", "CREATE TABLE t;");
            vol.write("/seed.sql", "INSERT INTO t VALUES (1);");
            let j = app.job().image("docker.io/library/busybox:latest").mount("/docker-entrypoint-initdb.d", vol);
            rt.start(j);
        });
    "#,
        "goo",
    );

    let mut found_writes = false;
    for resource in progress.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
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

// ---------------------------------------------------------------------------
// Operation-scoped external volume bindings
// ---------------------------------------------------------------------------

// l[verify volume.external.dynamic]
#[test]
fn external_volume_in_action_picks_up_operation_binding() {
    use crate::defs::container::VolumeMount;
    use crate::defs::volume::OperationVolumeBinding;
    use crate::runtime::barrier::runtime::{ActionClosureGuard, set_operation_volume_bindings};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let (engine, mut scope, app, ast) = run_test_script(
        r#"
        app.on_action("backup", |rt, _param| {
            let vol = app.external_volume("op-src-vol");
            let j = app.job().image("docker.io/library/busybox:latest").mount("/src", vol);
            rt.start(j);
        });
    "#,
    );

    let oracle = Arc::new(crate::runtime::TestWorldOracle::new());
    let log = crate::runtime::barrier::replay::InMemoryActionLog::new();
    let op = crate::runtime::barrier::OperationId::new();
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(crate::runtime::EphemeralInstanceRegistry::new());
    let progress = Arc::new(parking_lot::RwLock::new(None));

    // Populate the operation-scoped binding before running the action.
    let mut bindings = HashMap::new();
    bindings.insert(
        "op-src-vol".to_string(),
        OperationVolumeBinding {
            host_path: PathBuf::from("/btrfs/snapshots/abc123"),
            read_only: true,
        },
    );
    set_operation_volume_bindings(bindings.clone());

    let _guard = ActionClosureGuard::new(
        std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
            crate::defs::app::AppDef::default(),
        ))),
        String::new(),
        std::collections::HashMap::new(),
    );

    let result = crate::runtime::barrier::replay::run_operation(
        crate::runtime::barrier::replay::OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: op,
            app: &app,
            action_name: "backup",
            log: &log,
            world: oracle,
            registry,
            active_progress: Some(Arc::clone(&progress)),
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: bindings,
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
        },
        &mut scope,
    );

    assert!(
        matches!(
            result,
            crate::runtime::barrier::replay::OperationResult::Completed
                | crate::runtime::barrier::replay::OperationResult::Suspended(_)
        ),
        "expected Completed or Suspended, got {result:?}"
    );

    let prog = progress.read().clone().expect("progress should be set");
    for resource in prog.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
                if let VolumeMount::ExternalVolume(ev) = vm {
                    let binding = ev
                        .operation_binding
                        .as_ref()
                        .expect("operation_binding should be set for op-src-vol");
                    assert_eq!(
                        binding.host_path,
                        PathBuf::from("/btrfs/snapshots/abc123"),
                        "host_path should match injected binding"
                    );
                    assert!(binding.read_only, "read_only should be true");
                    return;
                }
            }
        }
    }
    panic!("expected Job with ExternalVolume mount in dynamic defs");
}

// l[verify volume.external.dynamic]
#[test]
fn external_volume_without_binding_has_no_operation_binding() {
    use crate::defs::container::VolumeMount;

    let progress = run_action_with_volumes(
        r#"
        app.on_action("no-binding", |rt, _param| {
            let vol = app.external_volume("static-vol");
            let j = app.job().image("docker.io/library/busybox:latest").mount("/mnt", vol);
            rt.start(j);
        });
    "#,
        "no-binding",
    );

    for resource in progress.dynamic_defs.values() {
        if let defs::resource::Resource::Job(job) = resource {
            let def = job.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            for vm in container.volume_mounts.values() {
                if let VolumeMount::ExternalVolume(ev) = vm {
                    assert!(
                        ev.operation_binding.is_none(),
                        "ExternalVolume without injected binding should have no operation_binding"
                    );
                    return;
                }
            }
        }
    }
    panic!("expected Job with ExternalVolume mount in dynamic defs");
}

// l[verify volume.external.dynamic]
#[test]
fn operation_bindings_cleared_after_guard_drops() {
    use crate::defs::volume::OperationVolumeBinding;
    use crate::runtime::barrier::runtime::{
        get_operation_volume_binding, set_operation_volume_bindings,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    let mut bindings = HashMap::new();
    bindings.insert(
        "temp-vol".to_string(),
        OperationVolumeBinding {
            host_path: PathBuf::from("/tmp/snapshot"),
            read_only: false,
        },
    );
    set_operation_volume_bindings(bindings);
    assert!(
        get_operation_volume_binding("temp-vol").is_some(),
        "binding should be present after set"
    );

    {
        let _guard = crate::runtime::barrier::runtime::ActionClosureGuard::new(
            std::sync::Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                crate::defs::app::AppDef::default(),
            ))),
            String::new(),
            std::collections::HashMap::new(),
        );
        // Guard replaces bindings with empty map on construction.
        assert!(
            get_operation_volume_binding("temp-vol").is_none(),
            "guard should replace bindings with its own map on construction"
        );
    }
    // After drop, bindings are cleared.
    assert!(
        get_operation_volume_binding("temp-vol").is_none(),
        "bindings should be cleared after guard drops"
    );
}

// l[verify app.resources.context.named]
// l[verify app.resources.context.immutable]
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
        app.on_action("goo", |rt, _param| {
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
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
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

// l[verify app.resources.context.immutable]
#[test]
fn captured_static_volume_cannot_be_modified_in_action() {
    use crate::runtime::{
        EphemeralInstanceRegistry, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{InMemoryActionLog, OperationContext, OperationResult, run_operation},
    };

    let (engine, mut scope, app, ast) = run_test_script(
        r#"
        let vol = app.volume("foo");
        vol.write("/outside", "content");
        app.on_action("act", |_rt, _param| {
            vol.write("/inside", "content");
        });
    "#,
    );

    let oracle = Arc::new(TestWorldOracle::new());
    let log = InMemoryActionLog::new();
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());

    let result = run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name: "act",
            log: &log,
            world: oracle,
            registry,
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: None,
            executor: None,
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
        other => panic!("expected Failed for captured static volume write, got {other:?}"),
    }

    // The static-context write outside the action must still take effect.
    let def = app.def.load();
    let vol_def = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Volume(v)
                if v.name.as_deref().map(|n| n.as_str()) == Some("foo") =>
            {
                Some(v.def.lock().clone())
            }
            _ => None,
        })
        .expect("foo volume should exist");
    assert_eq!(
        vol_def.writes,
        vec![("/outside".to_owned(), "content".to_owned())],
        "static-context write should be present, /inside must not be persisted"
    );
}

// ---------------------------------------------------------------------------
// rt.write — runtime-time write to a volume
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RecordedWrite {
    target: crate::runtime::barrier::VolumeWriteTarget,
    path: String,
    contents: String,
}

#[derive(Default)]
struct RecordingVolumeWriter {
    writes: parking_lot::Mutex<Vec<RecordedWrite>>,
}

impl crate::runtime::barrier::VolumeWriter for RecordingVolumeWriter {
    fn write(
        &self,
        _app: &str,
        target: crate::runtime::barrier::VolumeWriteTarget,
        path: &str,
        contents: &str,
    ) -> Result<(), String> {
        self.writes.lock().push(RecordedWrite {
            target,
            path: path.to_owned(),
            contents: contents.to_owned(),
        });
        Ok(())
    }
}

fn run_action_with_writer(
    script: &str,
    action_name: &str,
    writer: Arc<RecordingVolumeWriter>,
    log: &crate::runtime::barrier::replay::InMemoryActionLog,
) -> crate::runtime::barrier::replay::OperationResult {
    use crate::runtime::{
        EphemeralInstanceRegistry, TestWorldOracle,
        barrier::OperationId,
        barrier::replay::{OperationContext, run_operation},
    };

    let (engine, mut scope, app, ast) = run_test_script(script);
    let oracle = Arc::new(TestWorldOracle::new());
    let registry: Arc<dyn crate::runtime::InstanceRegistry> =
        Arc::new(EphemeralInstanceRegistry::new());
    run_operation(
        OperationContext {
            engine: &engine,
            script_ast: &ast,
            operation_id: OperationId::new(),
            app: &app,
            action_name,
            log,
            world: oracle,
            registry,
            active_progress: None,
            tick_notify: None,
            params: serde_json::Map::new(),
            is_shell: false,
            db: None,
            source_generation: 0,
            target_generation: 0,
            script_limits: None,
            cipher: None,
            operation_volume_bindings: std::collections::HashMap::new(),
            cancel_token: Arc::new(crate::runtime::barrier::CancelToken::new()),
            container_signaler: None,
            volume_writer: Some(writer as Arc<dyn crate::runtime::barrier::VolumeWriter>),
            executor: None,
        },
        &mut scope,
    )
}

// l[verify rt.write]
#[test]
fn rt_write_named_volume_invokes_writer() {
    use crate::runtime::barrier::VolumeWriteTarget;
    use crate::runtime::barrier::replay::{InMemoryActionLog, OperationResult};

    let writer = Arc::new(RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_writer(
        r#"
        let cfg = app.volume("cfg");
        app.on_action("seed", |rt, _param| {
            rt.write(cfg, "/etc/app.conf", "key=value");
        });
        "#,
        "seed",
        Arc::clone(&writer),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let writes = writer.writes.lock().clone();
    assert_eq!(writes.len(), 1);
    let w = &writes[0];
    assert_eq!(w.path, "/etc/app.conf");
    assert_eq!(w.contents, "key=value");
    match &w.target {
        VolumeWriteTarget::NamedVolume { name, tmpfs } => {
            assert_eq!(name, "cfg");
            assert!(!tmpfs);
        }
        other => panic!("expected NamedVolume, got {other:?}"),
    }
}

// l[verify rt.write]
#[test]
fn rt_write_anonymous_volume_invokes_writer() {
    use crate::runtime::barrier::VolumeWriteTarget;
    use crate::runtime::barrier::replay::{InMemoryActionLog, OperationResult};

    let writer = Arc::new(RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_writer(
        r#"
        app.on_action("seed", |rt, _param| {
            let scratch = app.volume();
            rt.write(scratch, "/work/note", "hi");
        });
        "#,
        "seed",
        Arc::clone(&writer),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let writes = writer.writes.lock().clone();
    assert_eq!(writes.len(), 1);
    match &writes[0].target {
        VolumeWriteTarget::AnonymousVolume { anon_id, tmpfs } => {
            assert!(anon_id.starts_with("seedling-anon-"));
            assert!(!tmpfs);
        }
        other => panic!("expected AnonymousVolume, got {other:?}"),
    }
}

// l[verify rt.write]
#[test]
fn rt_write_tmpfs_volume_is_allowed() {
    use crate::runtime::barrier::VolumeWriteTarget;
    use crate::runtime::barrier::replay::{InMemoryActionLog, OperationResult};

    let writer = Arc::new(RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_writer(
        r#"
        let scratch = app.volume("scratch").tmpfs();
        app.on_action("seed", |rt, _param| {
            rt.write(scratch, "/note", "ephemeral");
        });
        "#,
        "seed",
        Arc::clone(&writer),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let writes = writer.writes.lock().clone();
    assert_eq!(writes.len(), 1);
    match &writes[0].target {
        VolumeWriteTarget::NamedVolume { tmpfs, .. } => assert!(tmpfs),
        other => panic!("expected NamedVolume tmpfs, got {other:?}"),
    }
}

// l[verify rt.write]
#[test]
fn rt_write_rejects_path_traversal() {
    use crate::runtime::barrier::replay::{InMemoryActionLog, OperationResult};

    let writer = Arc::new(RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_writer(
        r#"
        let cfg = app.volume("cfg");
        app.on_action("seed", |rt, _param| {
            rt.write(cfg, "/../escape", "evil");
        });
        "#,
        "seed",
        Arc::clone(&writer),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(msg.contains("'..'"), "error should mention dotdot, got: {msg}");
        }
        other => panic!("expected Failed for path traversal, got {other:?}"),
    }
    assert!(writer.writes.lock().is_empty());
}

// l[verify rt.write]
#[test]
fn rt_write_outside_action_is_script_error() {
    let result = std::panic::catch_unwind(|| {
        let _ = run_test_script_app(
            r#"
            let cfg = app.volume("cfg");
            rt.write(cfg, "/foo", "bar");
            "#,
        );
    });
    assert!(
        result.is_err(),
        "rt.write at top level must error during script eval"
    );
}

// l[verify const.idle-cmd]
#[test]
fn idle_cmd_constant_usable_as_command() {
    let app = run_test_script_app(
        r#"
        app.deployment("idle-host")
            .image("docker.io/library/busybox:latest")
            .command(IDLE_CMD);
        "#,
    );
    let def = app.def.load();
    let dep = def
        .resources
        .values()
        .find_map(|r| match r {
            defs::resource::Resource::Deployment(d) if &*d.name == "idle-host" => Some(d.clone()),
            _ => None,
        })
        .expect("idle-host deployment");
    let pod = dep.def.lock().pod.clone();
    let cmd = pod.lock().container.lock().command.clone();
    assert_eq!(
        cmd,
        Some(vec!["sleep".to_owned(), "infinity".to_owned()]),
        "IDLE_CMD must wire through to the container command"
    );
}

// l[verify rt.write] r[verify rt.write]
#[test]
fn rt_write_skipped_on_replay() {
    use crate::runtime::barrier::CallKind;
    use crate::runtime::barrier::replay::{ActionLog, InMemoryActionLog, OperationResult};

    let writer = Arc::new(RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let script = r#"
        let cfg = app.volume("cfg");
        app.on_action("seed", |rt, _param| {
            rt.write(cfg, "/a.conf", "first");
        });
    "#;

    let result = run_action_with_writer(script, "seed", Arc::clone(&writer), &log);
    assert!(matches!(result, OperationResult::Completed));
    assert_eq!(writer.writes.lock().len(), 1);

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].call_kind, CallKind::Write);
    assert_eq!(entries[0].extra.as_deref(), Some("/a.conf"));

    // Second pass on the same log: the write must NOT be re-issued.
    let result2 = run_action_with_writer(script, "seed", Arc::clone(&writer), &log);
    assert!(matches!(result2, OperationResult::Completed));
    assert_eq!(
        writer.writes.lock().len(),
        1,
        "rt.write must be at-most-once across replays"
    );
}
