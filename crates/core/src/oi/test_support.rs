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
use crate::runtime::scheduler::Scheduler;
use crate::runtime::secrets::Cipher;
use crate::runtime::tls::issuance::Coordinator;
use crate::system::System;

/// A fully-wired in-memory OI: call [`TestOi::call`] with a method path and
/// params to exercise the same dispatch path the QUIC server uses.
pub(crate) struct TestOi {
    pub state: Arc<OiState>,
    pub ctx: RequestCtx,
    /// Handlers spawn tokio tasks (lifecycle operations, re-evaluations), so
    /// keep a runtime alive and enter it around every dispatch.
    rt: tokio::runtime::Runtime,
    _data_dir: tempfile::TempDir,
}

impl TestOi {
    pub fn new() -> Self {
        let data_dir = tempfile::tempdir().expect("create temp data dir");
        let (driver, _caddy_admin) =
            System::setup_stubbed(data_dir.path(), false).expect("stub system setup");
        let db = DbHandle::open_in_memory().expect("open in-memory db");
        let cipher = Arc::new(Cipher::for_tests());
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
            db: db.clone(),
            scheduler: Arc::new(Mutex::new(Scheduler::new())),
            tick_notify: Arc::new(tokio::sync::Notify::new()),
            db_path: data_dir.path().join("seedling.db"),
            trusted_keys: crate::oi::auth::new_trusted_keys(),
            shells: crate::oi::shells::ShellRegistry::new(),
            forwards: crate::oi::forwards::ForwardRegistry::new(),
            container_runtime: Arc::clone(&driver.container),
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
        });

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
            ctx,
            rt,
            _data_dir: data_dir,
        }
    }

    /// Dispatch `method` with `params` and return the parsed `result` value,
    /// or the error `(code, message)` pair.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, (String, String)> {
        let request = serde_json::to_vec(&json!({ "method": method, "params": params }))
            .expect("request serialisation never fails");
        let _guard = self.rt.enter();
        let response = dispatch(&self.state, &request, &self.ctx);
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

    // i[verify status.ping]
    #[test]
    fn ping_round_trips_through_dispatch() {
        let oi = TestOi::new();
        let result = oi.call("/server/ping", json!({}));
        assert_eq!(result.unwrap(), json!({}));
    }

    // i[verify status.get]
    #[test]
    fn status_reports_empty_registry() {
        let oi = TestOi::new();
        let status = oi.call("/server/status", json!({})).unwrap();
        assert_eq!(status["apps_total"], 0);
        assert_eq!(status["active_faults"], 0);
        assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    }

    // i[verify wire.request]
    #[test]
    fn unknown_method_returns_not_found() {
        let oi = TestOi::new();
        let (code, message) = oi.call("/no/such/method", json!({})).unwrap_err();
        assert_eq!(code, "not_found");
        assert!(message.contains("/no/such/method"), "message: {message}");
    }
}
