//! In-memory OI harness for handler tests: a real [`OiState`] wired to the
//! stub system fleet, an in-memory database, and a test cipher, so tests can
//! drive [`dispatch`](super::handler::dispatch) end to end without podman,
//! systemd, Caddy, or the network.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use seedling_protocol::actor::Actor;
use seedling_protocol::events::{EventSenderWithActor, new_event_channel};
use serde_json::{Value, json};

use crate::ScriptLimits;
use crate::oi::handler::{RequestCtx, dispatch};
use crate::oi::state::OiState;
use crate::runtime::apps::AppRegistry;
use crate::runtime::db::DbHandle;
use crate::runtime::definition::fetch::testing::FakeRegistry;
use crate::runtime::scheduler::Scheduler;
use crate::runtime::secrets::Cipher;
use crate::runtime::tls::issuance::Coordinator;
use crate::system::System;

/// Minimal BSL script: one app with one deployment and a scalable container.
pub(crate) const MINIMAL_SCRIPT: &str = r#"
    app.deployment("web")
        .image("docker.io/library/nginx:1.29")
        .scale(1..=4);
"#;

/// [`MINIMAL_SCRIPT`] plus a plaintext and a secret param.
pub(crate) const PARAMS_SCRIPT: &str = r#"
    app.param("greeting")
        .required(false)
        .description("plain text param");
    app.param("api-key")
        .kind("password")
        .required(false);
    app.deployment("web")
        .image("docker.io/library/nginx:1.29");
"#;

/// A fully-wired in-memory OI: call [`TestOi::call`] with a method path and
/// params to exercise the same dispatch path the QUIC server uses.
pub(crate) struct TestOi {
    pub state: Arc<OiState>,
    /// The in-memory registry definitions are fetched from.
    pub registry: Arc<FakeRegistry>,
    pub ctx: RequestCtx,
    /// Handlers spawn tokio tasks (lifecycle operations, re-evaluations), so
    /// keep a runtime alive and enter it around every dispatch.
    rt: tokio::runtime::Runtime,
    _data_dir: tempfile::TempDir,
}

impl TestOi {
    pub fn new() -> Self {
        Self::build(None, None)
    }

    /// A harness running over `driver` rather than the stub system.
    pub fn with_driver(driver: Arc<System>) -> Self {
        Self::build(Some(driver), None)
    }

    /// A harness whose container runtime fails every call, as when podman
    /// could not be reached.
    pub fn with_unreadable_containers() -> Self {
        Self::build(
            None,
            Some(Arc::new(
                crate::system::unavailable::UnavailableContainerRuntime,
            )),
        )
    }

    fn build(
        driver: Option<Arc<System>>,
        container_runtime: Option<Arc<dyn crate::system::ContainerRuntime>>,
    ) -> Self {
        let data_dir = tempfile::tempdir().expect("create temp data dir");
        let driver = driver.unwrap_or_else(|| {
            System::setup_stubbed(data_dir.path(), false)
                .expect("stub system setup")
                .0
        });
        let db = DbHandle::open_in_memory().expect("open in-memory db");
        let cipher = Arc::new(Cipher::for_tests());
        let registry = Arc::new(FakeRegistry::default());
        let event_tx = new_event_channel();
        let actor = Arc::new(Actor {
            kind: Some("test".into()),
            id: Some("test-suite".into()),
            display: None,
            session: None,
        });

        let state = Arc::new(OiState {
            registry: Arc::new(RwLock::new(AppRegistry::new())),
            spki_fingerprint: OnceLock::new(),
            start_time: Instant::now(),
            started_at: jiff::Timestamp::now(),
            db: db.clone(),
            scheduler: Arc::new(Mutex::new(Scheduler::new())),
            tick_notify: Arc::new(tokio::sync::Notify::new()),
            db_path: data_dir.path().join("seedling.db"),
            trusted_keys: crate::oi::auth::new_trusted_keys(),
            shells: crate::oi::shells::ShellRegistry::new(),
            forwards: crate::oi::forwards::ForwardRegistry::new(),
            canopy: Arc::new(crate::oi::canopy::CanopyState::new()),
            container_runtime: container_runtime.unwrap_or_else(|| Arc::clone(&driver.container)),
            driver,
            node_prefix: "fd5e:ed11:9000::/48".parse().expect("valid /48"),
            event_tx: event_tx.clone(),
            script_limits: ScriptLimits::default(),
            dns_servers: Vec::new(),
            cipher: Arc::clone(&cipher),
            tls_coordinator: Coordinator::new(db, cipher),
            caddy_data_path: tokio::sync::OnceCell::new(),
            tailscale_provider: None,
            site_resolver: None,
            definition_registry: Arc::clone(&registry) as _,
        });

        // As the daemon does once its state exists: without it the coordinator
        // can make no chain decision, and a test would see the answer given to
        // not knowing rather than the one the daemon gives.
        state
            .tls_coordinator
            .set_environment(Arc::new(crate::oi::state::OiEnvironment::new(&state)));

        let ctx = RequestCtx {
            events: EventSenderWithActor::new(event_tx, actor),
        };

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build tokio runtime");

        Self {
            state,
            registry,
            ctx,
            rt,
            _data_dir: data_dir,
        }
    }

    /// A fresh harness with `name` already registered from [`MINIMAL_SCRIPT`].
    pub fn with_app(name: &str) -> Self {
        let oi = Self::new();
        oi.call(
            "/apps/create",
            json!({ "app": name, "script": MINIMAL_SCRIPT }),
        )
        .expect("register app");
        oi
    }

    /// Mark `name` installed through the immediate (no `on_install`) path.
    ///
    /// Installing starts the app's start action, which holds the scheduler
    /// until it ends; this waits for that, so the app is left installed and
    /// nothing is in progress.
    pub fn install(&self, name: &str) {
        self.call("/apps/install/invoke", json!({ "app": name }))
            .expect("install app");
        self.block_on(async {
            let idle = async {
                while self.state.scheduler.lock().active().is_some() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(10), idle)
                .await
                .expect("the start action ends");
        });
    }

    /// Drive a future on the harness's own runtime.
    ///
    /// For handlers that must await — those dispatched from the stream path
    /// rather than the shared sync table. A `#[tokio::test]` cannot be used for
    /// them instead, because dropping this harness drops the runtime it holds,
    /// which panics inside an async context.
    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    /// Dispatch `method` with `params` and return the parsed `result` value,
    /// or the error `(code, message)` pair.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, (String, String)> {
        let request = serde_json::to_vec(&json!({ "method": method, "params": params }))
            .expect("request serialisation never fails");
        self.call_raw(&request)
    }

    /// Dispatch a raw request payload (possibly malformed) through the same
    /// path the QUIC server uses.
    pub fn call_raw(&self, request: &[u8]) -> Result<Value, (String, String)> {
        let _guard = self.rt.enter();
        let response = dispatch(&self.state, request, &self.ctx);
        let mut response: Value =
            serde_json::from_slice(&response).expect("response is valid JSON");
        if let Some(error) = response.get("error") {
            Err((
                error["code"].as_str().unwrap_or_default().to_owned(),
                error["message"].as_str().unwrap_or_default().to_owned(),
            ))
        } else {
            Ok(response["result"].take())
        }
    }
}

mod tests {
    use super::*;

    // spec: STS#liveness
    #[test]
    fn ping_round_trips_through_dispatch() {
        let oi = TestOi::new();
        let result = oi.call("/server/ping", json!({}));
        assert_eq!(result.unwrap(), json!({}));
    }

    // spec: STS#server-status
    #[test]
    fn status_reports_empty_registry() {
        let oi = TestOi::new();
        let status = oi.call("/server/status", json!({})).unwrap();
        assert_eq!(status["apps_total"], 0);
        assert_eq!(status["active_faults"], 0);
        assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    }

    // spec: STS#server-status
    // The field was the literal 0, so the summary reported no operation in
    // progress however busy the runtime was. Driving one through the
    // scheduler is what distinguishes a derived count from a constant.
    #[test]
    fn status_counts_an_operation_in_progress() {
        use seedling_protocol::names::{ActionName, AppName};

        let oi = TestOi::new();
        let idle = oi.call("/server/status", json!({})).unwrap();
        assert_eq!(idle["active_operations"], 0, "idle runtime");

        let app = AppName::new("statusapp").expect("app name");
        let action = ActionName::new("install").expect("action name");
        oi.state.scheduler.lock().admit(
            &app,
            &action,
            serde_json::Map::new(),
            1,
            1,
            "test",
            &parking_lot::Mutex::new(crate::runtime::apps::AppPhase::Installed),
        );

        let busy = oi.call("/server/status", json!({})).unwrap();
        assert_eq!(busy["active_operations"], 1, "one operation active");
    }

    const ACTION_SCRIPT: &str = r#"
        app.deployment("web").image("docker.io/library/nginx:1.29");
        app.on_action("refresh", |rt, _p| {}, #{
            params: #{ token: #{ kind: "text", secret: true, required: false } },
        });
    "#;

    /// Register `app`, installed, with an action, and hold the scheduler with
    /// an operation of another app so that a request for `app` queues.
    fn oi_with_a_busy_scheduler(app: &str) -> TestOi {
        use seedling_protocol::names::{ActionName, AppName};
        let oi = TestOi::new();
        oi.call(
            "/apps/create",
            json!({ "app": app, "script": ACTION_SCRIPT }),
        )
        .unwrap();
        *oi.state.registry.read().get(app).unwrap().phase.lock() =
            crate::runtime::apps::AppPhase::Installed;
        oi.state.scheduler.lock().admit(
            &AppName::new("blocker").unwrap(),
            &ActionName::new("install").unwrap(),
            serde_json::Map::new(),
            1,
            1,
            "test",
            &parking_lot::Mutex::new(crate::runtime::apps::AppPhase::Installed),
        );
        oi
    }

    // spec: OPS#concurrency
    #[test]
    fn a_queued_request_is_persisted_with_its_params_protected_and_cleared_when_it_starts() {
        let oi = oi_with_a_busy_scheduler("demo");
        let result = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "refresh", "params": { "token": "hunter2" } }),
            )
            .unwrap();
        assert_eq!(result["schedule"], "queued");
        let id = result["operation_id"].as_str().unwrap().to_owned();

        let (count, ciphertext, actor): (i64, Vec<u8>, Option<String>) = oi.state.db.call(|db| {
            db.conn
                .query_row(
                    "SELECT COUNT(*), params_ciphertext, actor FROM queued_operations",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap()
        });
        assert_eq!(count, 1);
        assert!(
            !String::from_utf8_lossy(&ciphertext).contains("hunter2"),
            "the params are stored protected"
        );
        assert!(
            actor.unwrap().contains("test-suite"),
            "the requesting actor is kept"
        );

        let restored = oi.state.db.call({
            let cipher = std::sync::Arc::clone(&oi.state.cipher);
            move |db| crate::runtime::history::load_queued_operations(db, &cipher).unwrap()
        });
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].operation_id.0, id);
        assert_eq!(restored[0].params["token"], "hunter2");
        assert_eq!(
            restored[0].actor.as_ref().unwrap().id.as_deref(),
            Some("test-suite")
        );
    }

    // spec: SCHED#fire-times
    #[test]
    fn a_queued_fire_is_stamped_once_persisted_and_left_unstamped_if_refused() {
        use crate::runtime::{
            db as rdb, scheduler::ScheduleResult, schedules::check_due_schedules,
        };
        let oi = oi_with_a_busy_scheduler("demo");
        let app = seedling_protocol::names::AppName::new("demo").unwrap();
        let action = seedling_protocol::names::ActionName::new("refresh").unwrap();
        oi.state.db.call({
            let (app, action) = (app.clone(), action.clone());
            move |db| {
                rdb::ensure_schedules(db, &app, &[(action.clone(), "0 * * * *".to_owned())])
                    .unwrap();
                rdb::upsert_schedule_fired(db, &app, &action, "0 * * * *", "2026-04-21T17:05:00Z")
                    .unwrap();
            }
        });
        let now: jiff::Timestamp = "2026-04-21T18:37:22Z".parse().unwrap();
        let phase = Arc::clone(&oi.state.registry.read().get("demo").unwrap().phase);
        let fire = |oi: &TestOi| {
            let mut fired = oi.state.db.call({
                let sched = Arc::clone(&oi.state.scheduler);
                let phase = Arc::clone(&phase);
                move |db| {
                    check_due_schedules(db, &mut sched.lock(), now, &|_| {
                        Some((1, Arc::clone(&phase), Default::default()))
                    })
                }
            });
            assert_eq!(fired.len(), 1);
            assert!(
                !fired[0].accepted,
                "it queued behind the other app's operation"
            );
            fired.remove(0)
        };
        let last_fired = |oi: &TestOi| -> String {
            oi.state.db.call(|db| {
                db.conn
                    .query_row("SELECT last_fired_at FROM action_schedules", [], |r| {
                        r.get(0)
                    })
                    .unwrap()
            })
        };

        // Refused: not persisted, so not stamped, and off the queue.
        oi.state.db.call(|db| {
            db.conn
                .execute_batch("DROP TABLE queued_operations;")
                .unwrap()
        });
        let first = fire(&oi);
        crate::oi::handler::actions::lifecycle::persist_queued_fire(&oi.state, &first, now);
        assert_eq!(last_fired(&oi), "2026-04-21T17:05:00Z");
        assert_eq!(oi.state.scheduler.lock().queue_iter().count(), 0);

        // Persisted: stamped, so a restart restores it rather than firing again.
        oi.state.db.call(|db| {
            db.conn
                .execute_batch(
                    "CREATE TABLE queued_operations (
                        seq INTEGER PRIMARY KEY AUTOINCREMENT, operation_id TEXT NOT NULL UNIQUE,
                        app TEXT NOT NULL, action TEXT NOT NULL, trigger TEXT NOT NULL,
                        source_generation INTEGER NOT NULL, target_generation INTEGER NOT NULL,
                        params_ciphertext BLOB NOT NULL, actor TEXT);",
                )
                .unwrap()
        });
        let second = fire(&oi);
        crate::oi::handler::actions::lifecycle::persist_queued_fire(&oi.state, &second, now);
        assert_eq!(last_fired(&oi), now.to_string());
        let _ = ScheduleResult::Queued;
    }

    // spec: OPS#concurrency
    #[test]
    fn a_request_that_cannot_be_persisted_is_refused_and_not_queued() {
        let oi = oi_with_a_busy_scheduler("demo");
        oi.state.db.call(|db| {
            db.conn
                .execute_batch("DROP TABLE queued_operations;")
                .unwrap();
        });
        let (code, _) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "refresh" }),
            )
            .unwrap_err();
        assert_eq!(code, "internal");
        assert_eq!(
            oi.state.scheduler.lock().queue_iter().count(),
            0,
            "a refused request must not run later"
        );
    }

    // spec: OPS#concurrency
    #[test]
    fn a_restored_request_for_an_unregistered_app_is_discarded_and_the_rest_are_kept() {
        let oi = oi_with_a_busy_scheduler("demo");
        let queued = |app: &str, id: &str| crate::runtime::scheduler::QueuedOperation {
            app: seedling_protocol::names::AppName::new(app).unwrap(),
            action: seedling_protocol::names::ActionName::new("refresh").unwrap(),
            operation_id: crate::runtime::barrier::OperationId(id.to_owned()),
            params: serde_json::Map::new(),
            source_generation: 1,
            target_generation: 1,
            trigger: "operator".to_owned(),
            actor: None,
        };
        let ghost = queued("ghost", "op-ghost");
        let real = queued("demo", "op-real");
        for op in [&ghost, &real] {
            let (op, cipher) = (op.clone(), std::sync::Arc::clone(&oi.state.cipher));
            oi.state
                .db
                .call(move |db| crate::runtime::history::save_queued_operation(db, &cipher, &op))
                .unwrap();
        }

        crate::oi::handler::actions::lifecycle::restore_queue(&oi.state, vec![ghost, real]);

        let queue: Vec<String> = oi
            .state
            .scheduler
            .lock()
            .queue_iter()
            .map(|q| q.operation_id.0.clone())
            .collect();
        assert_eq!(queue, vec!["op-real"]);
        let rows: i64 = oi.state.db.call(|db| {
            db.conn
                .query_row("SELECT COUNT(*) FROM queued_operations", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(rows, 1, "the discarded request's record is gone");
    }

    // spec: OPS#interrupted-operations
    // spec: OPS#stuck-install-recovery
    #[test]
    fn an_install_that_cannot_be_replayed_is_recovered_as_a_stuck_install() {
        let oi = TestOi::with_app("demo");
        *oi.state.registry.read().get("demo").unwrap().phase.lock() =
            crate::runtime::apps::AppPhase::Installing;
        let name = seedling_protocol::names::AppName::new("demo").unwrap();

        crate::oi::handler::actions::lifecycle::recover_unreplayable_install(&oi.state, &name);

        assert_eq!(
            *oi.state.registry.read().get("demo").unwrap().phase.lock(),
            crate::runtime::apps::AppPhase::Uninstalling,
            "teardown, not NotInstalled, is the way out"
        );
        let faults = oi.call("/faults/list", json!({ "app": "demo" })).unwrap();
        assert!(
            faults
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["kind"] == "install_interrupted")
        );
    }

    // spec: OPS#unreadable-operation-state
    #[test]
    fn nothing_starts_while_the_persisted_operation_state_is_unreadable() {
        let oi = TestOi::new();
        oi.call(
            "/apps/create",
            json!({ "app": "demo", "script": ACTION_SCRIPT }),
        )
        .unwrap();
        oi.install("demo");
        oi.state.scheduler.lock().set_state_unreadable(true);
        let (code, _) = oi
            .call(
                "/apps/action/invoke",
                json!({ "app": "demo", "name": "refresh" }),
            )
            .unwrap_err();
        assert_eq!(code, "internal");
        assert!(
            oi.state
                .scheduler
                .lock()
                .active()
                .is_none_or(|a| a.action != "refresh"),
            "the refused request did not start"
        );
        oi.state.scheduler.lock().set_state_unreadable(false);
    }

    // spec: WIRE#error-codes
    // A failed read is a failure inside the server, never an answer about the
    // thing asked for.
    #[test]
    fn a_failed_database_read_is_internal() {
        let oi = TestOi::with_app("demo");
        oi.state.db.call(|db| {
            db.conn
                .execute_batch(
                    "DROP TABLE faults; DROP TABLE authorized_keys; DROP TABLE allowed_registries;
                     DROP TABLE tls_dns_providers;",
                )
                .unwrap();
        });
        for (method, params) in [
            ("/faults/list", json!({})),
            ("/keys/list", json!({})),
            ("/registries/list", json!({})),
            ("/tls/dns-providers/list", json!({})),
        ] {
            let (code, _) = oi.call(method, params).unwrap_err();
            assert_eq!(code, "internal", "{method}");
        }
    }

    // spec: IMGM#image-discovery
    #[test]
    fn discovery_with_a_proposed_script_that_fails_is_a_script_error() {
        let oi = TestOi::with_app("demo");
        let (code, _) = oi
            .call(
                "/apps/images/discover",
                json!({ "app": "demo", "proposed_script": "throw \"broken\";" }),
            )
            .unwrap_err();
        assert_eq!(code, "script_error");
    }

    // spec: APPS#removal
    // spec: TRD#deregistration
    #[test]
    fn a_removal_that_cannot_discard_the_apps_state_fails_and_keeps_the_app() {
        let oi = TestOi::with_app("demo");
        oi.state.db.call(|db| {
            db.conn
                .execute_batch("DROP TABLE action_schedules;")
                .unwrap();
        });
        let (code, _) = oi
            .call("/apps/remove", json!({ "app": "demo" }))
            .unwrap_err();
        assert_eq!(code, "internal");
        let listed = oi.call("/apps/list", json!({})).unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["name"] == "demo"),
            "the app is still registered"
        );
    }

    // spec: FWD#stopping-a-forward
    #[test]
    fn stopping_a_forward_names_a_malformed_id_and_a_missing_one_apart() {
        let oi = TestOi::new();
        let (code, _) = oi
            .call("/forwards/stop", json!({ "forward_id": "not-a-uuid" }))
            .unwrap_err();
        assert_eq!(code, "requirements_invalid");
        let (code, _) = oi
            .call(
                "/forwards/stop",
                json!({ "forward_id": "6f1c1f5e-3b0b-4c8e-9a43-2d6f2b8f6a10" }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");
    }

    // spec: WIRE#control-requests
    #[test]
    fn unknown_method_returns_not_found() {
        let oi = TestOi::new();
        let (code, message) = oi.call("/no/such/method", json!({})).unwrap_err();
        assert_eq!(code, "not_found");
        assert!(message.contains("/no/such/method"), "message: {message}");
    }

    // spec: WIRE#control-requests
    #[test]
    fn malformed_request_returns_error_envelope() {
        let oi = TestOi::new();
        let (code, message) = oi.call_raw(b"this is not json").unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(message.contains("invalid request"), "message: {message}");
    }

    // spec: WIRE#control-requests
    #[test]
    fn invalid_params_shape_is_rejected() {
        let oi = TestOi::new();
        // Missing required field.
        let (code, message) = oi.call("/apps/show", json!({})).unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(message.contains("invalid params"), "message: {message}");
        // Wrong type.
        let (code, _) = oi.call("/apps/show", json!({ "app": 42 })).unwrap_err();
        assert_eq!(code, "requirements_invalid");
    }
}
