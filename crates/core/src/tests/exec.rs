use std::sync::Arc;

use parking_lot::Mutex;

use super::*;
use crate::runtime::barrier::CallKind;
use crate::runtime::barrier::replay::{
    ActionLog, InMemoryActionLog, OperationContext, OperationResult, run_operation,
};

#[derive(Debug, Clone)]
struct RecordedExec {
    name: String,
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<RecordedExec>>,
    /// Exit code to return on each call. If the queue is exhausted, returns 0.
    exit_codes: Mutex<Vec<i32>>,
}

impl RecordingExecutor {
    fn with_exits(codes: Vec<i32>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            exit_codes: Mutex::new(codes),
        }
    }
}

impl crate::runtime::barrier::Executor for RecordingExecutor {
    fn exec(
        &self,
        name: &str,
        argv: &[String],
        extra_env: &[(String, String)],
    ) -> Result<i32, String> {
        self.calls.lock().push(RecordedExec {
            name: name.to_owned(),
            argv: argv.to_vec(),
            env: extra_env.to_vec(),
        });
        let code = if self.exit_codes.lock().is_empty() {
            0
        } else {
            self.exit_codes.lock().remove(0)
        };
        Ok(code)
    }
}

fn run_action_with_executor(
    script: &str,
    action_name: &str,
    executor: Arc<RecordingExecutor>,
    log: &InMemoryActionLog,
) -> OperationResult {
    use crate::runtime::{EphemeralInstanceRegistry, TestWorldOracle, barrier::OperationId};

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
            volume_writer: None,
            executor: Some(executor as Arc<dyn crate::runtime::barrier::Executor>),
        },
        &mut scope,
    )
}

// l[verify rt.exec]
#[test]
fn rt_exec_named_deployment_scale_one_runs_command() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("ping", |rt, _param| {
            rt.exec(api, ["echo", "hi"]);
        });
        "#,
        "ping",
        Arc::clone(&exec),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let calls = exec.calls.lock().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].argv, vec!["echo", "hi"]);
    assert!(calls[0].env.is_empty());
    assert!(
        calls[0].name.contains("api"),
        "container target name should reference the deployment, got {}",
        calls[0].name
    );
}

// l[verify rt.exec]
#[test]
fn rt_exec_via_started_runs_against_same_instance() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        app.on_action("setup", |rt, _param| {
            let host = app.job().image("docker.io/library/busybox:latest").command(IDLE_CMD);
            let started = rt.start(host);
            rt.exec(started, ["echo", "step1"]);
            rt.exec(started, ["echo", "step2"]);
        });
        "#,
        "setup",
        Arc::clone(&exec),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let calls = exec.calls.lock().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].argv, vec!["echo", "step1"]);
    assert_eq!(calls[1].argv, vec!["echo", "step2"]);
    assert_eq!(
        calls[0].name, calls[1].name,
        "both execs must hit the same container instance"
    );
}

// l[verify rt.exec]
#[test]
fn rt_exec_rejects_scale_greater_than_one() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let workers = app.deployment("worker")
            .image("docker.io/library/busybox:latest")
            .scale(3);
        app.on_action("nope", |rt, _param| {
            rt.exec(workers, ["echo", "hi"]);
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("scale") && msg.contains("1"),
                "error should mention scale must be 1, got: {msg}"
            );
        }
        other => panic!("expected Failed for scale > 1, got {other:?}"),
    }
    assert!(exec.calls.lock().is_empty());
}

// l[verify rt.exec]
#[test]
fn rt_exec_rejects_scale_range_with_upper_above_one() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api")
            .image("docker.io/library/busybox:latest")
            .scale(1..3);
        app.on_action("nope", |rt, _param| {
            rt.exec(api, ["echo", "hi"]);
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("scale"),
                "error should mention scale, got: {msg}"
            );
        }
        other => panic!("expected Failed for scale upper > 1, got {other:?}"),
    }
}

// l[verify rt.exec]
#[test]
fn rt_exec_accepts_zero_to_one_range() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api")
            .image("docker.io/library/busybox:latest")
            .scale(0..1);
        app.on_action("ping", |rt, _param| {
            rt.exec(api, ["echo", "hi"]);
        });
        "#,
        "ping",
        Arc::clone(&exec),
        &log,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "scale(0..1) must be accepted, got {result:?}"
    );
    assert_eq!(exec.calls.lock().len(), 1);
}

// l[verify rt.exec]
#[test]
fn rt_exec_anonymous_deployment_directly_is_error() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        app.on_action("nope", |rt, _param| {
            let dep = app.deployment().image("docker.io/library/busybox:latest");
            rt.exec(dep, ["echo", "hi"]);
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("anonymous") || msg.contains("Started"),
                "error should suggest using Started, got: {msg}"
            );
        }
        other => panic!("expected Failed for anonymous deployment, got {other:?}"),
    }
}

// l[verify rt.exec]
#[test]
fn rt_exec_outside_action_is_script_error() {
    let result = std::panic::catch_unwind(|| {
        let _ = run_test_script_app(
            r#"
            let api = app.deployment("api").image("docker.io/library/busybox:latest");
            rt.exec(api, ["echo", "hi"]);
            "#,
        );
    });
    assert!(
        result.is_err(),
        "rt.exec at top level must error during script eval"
    );
}

// l[verify rt.exec]
#[test]
fn rt_exec_empty_argv_is_error() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("nope", |rt, _param| {
            rt.exec(api, []);
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            assert!(
                e.to_string().contains("non-empty"),
                "error should mention argv must be non-empty, got: {e}"
            );
        }
        other => panic!("expected Failed for empty argv, got {other:?}"),
    }
}

// l[verify rt.exec]
#[test]
fn rt_exec_env_option_passes_through() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("env", |rt, _param| {
            rt.exec(api, ["echo", "hi"], #{
                env: #{ FOO: "bar", BAZ: "qux" },
            });
        });
        "#,
        "env",
        Arc::clone(&exec),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed));

    let calls = exec.calls.lock().clone();
    assert_eq!(calls.len(), 1);
    let mut env = calls[0].env.clone();
    env.sort();
    assert_eq!(
        env,
        vec![
            ("BAZ".to_owned(), "qux".to_owned()),
            ("FOO".to_owned(), "bar".to_owned()),
        ]
    );
}

// l[verify rt.exec]
#[test]
fn rt_exec_rejects_unknown_option() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("nope", |rt, _param| {
            rt.exec(api, ["echo", "hi"], #{ working_dir: "/tmp" });
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            assert!(
                e.to_string().contains("working_dir"),
                "error should name the unknown option, got: {e}"
            );
        }
        other => panic!("expected Failed for unknown option, got {other:?}"),
    }
}

// l[verify rt.exec]
#[test]
fn rt_exec_rejects_invalid_env_var_name() {
    let exec = Arc::new(RecordingExecutor::default());
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("nope", |rt, _param| {
            rt.exec(api, ["echo", "hi"], #{
                env: #{ "1BAD": "value" },
            });
        });
        "#,
        "nope",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            assert!(
                e.to_string().contains("env var name"),
                "error should mention the bad env var name, got: {e}"
            );
        }
        other => panic!("expected Failed for invalid env var, got {other:?}"),
    }
}

// l[verify rt.executed.type] l[verify rt.executed.exit-code] l[verify rt.executed.success]
#[test]
fn executed_methods_reflect_exit_code() {
    let exec = Arc::new(RecordingExecutor::with_exits(vec![0, 7]));
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("twice", |rt, _param| {
            let a = rt.exec(api, ["echo", "first"]);
            if !a.success() { throw "first should succeed"; }
            if a.exit_code() != 0 { throw "first exit_code should be 0"; }
            let b = rt.exec(api, ["false"]);
            if b.success() { throw "second should fail"; }
            if b.exit_code() != 7 { throw "second exit_code should be 7"; }
        });
        "#,
        "twice",
        Arc::clone(&exec),
        &log,
    );
    assert!(
        matches!(result, OperationResult::Completed),
        "all assertions inside the action must pass; got {result:?}"
    );
}

// l[verify rt.executed.ensure-success]
#[test]
fn executed_ensure_success_throws_on_nonzero() {
    let exec = Arc::new(RecordingExecutor::with_exits(vec![3]));
    let log = InMemoryActionLog::new();
    let result = run_action_with_executor(
        r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("strict", |rt, _param| {
            rt.exec(api, ["false"]).ensure_success();
        });
        "#,
        "strict",
        Arc::clone(&exec),
        &log,
    );
    match result {
        OperationResult::Failed(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("exit code 3"),
                "ensure_success error should mention exit code 3, got: {msg}"
            );
        }
        other => panic!("expected Failed from ensure_success, got {other:?}"),
    }
}

// l[verify rt.exec] r[verify rt.exec]
#[test]
fn rt_exec_skipped_on_replay_recovers_exit_code() {
    let exec = Arc::new(RecordingExecutor::with_exits(vec![5]));
    let log = InMemoryActionLog::new();
    let script = r#"
        let api = app.deployment("api").image("docker.io/library/busybox:latest");
        app.on_action("once", |rt, _param| {
            let r = rt.exec(api, ["echo", "hi"]);
            if r.exit_code() != 5 { throw `expected 5, got ${r.exit_code()}`; }
        });
    "#;

    let result = run_action_with_executor(script, "once", Arc::clone(&exec), &log);
    assert!(matches!(result, OperationResult::Completed));
    assert_eq!(exec.calls.lock().len(), 1);

    let entries = log.load().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].call_kind, CallKind::Exec);
    assert_eq!(entries[0].extra.as_deref(), Some("5"));

    // Second pass on the same log: the exec must NOT be re-issued, but the
    // recovered exit code must be the same so the assertion still holds.
    let result2 = run_action_with_executor(script, "once", Arc::clone(&exec), &log);
    assert!(
        matches!(result2, OperationResult::Completed),
        "replay must recover exit code 5; got {result2:?}"
    );
    assert_eq!(
        exec.calls.lock().len(),
        1,
        "rt.exec must be at-most-once across replays"
    );
}
