use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc, time::Duration};

use parking_lot::Mutex;
use seedling_protocol::{
    events::{EventSenderWithActor, OperationTarget},
    names::{ActionName, AppName, SessionId, ShellName},
};
use tokio::{io::AsyncWriteExt, sync::Notify};

use super::registry::{ShellSession, StopSignal};
use crate::{
    defs::{app::App, resource::ResourceKind},
    oi::state::OiState,
    runtime::{
        AppPhase,
        apps::{DynamicSlot, SessionSlots},
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, Subject, run_operation},
            shell::{
                ShellAttachCtx, ShellExecTarget, ShellExit, ShellHost, ShellProgress,
                clear_shell_attach_ctx, set_shell_attach_ctx,
            },
        },
        dynamic_cleanup::cleanup_dynamic_resources,
        identity::{InstanceVariant, ResourceInstance},
        param_map::ParamMap,
        registry::{DbInstanceRegistry, InstanceRegistry, RegistryError},
    },
    system::{
        translate::container::{ConfigStamp, job_spec},
        types::ExecHandle,
    },
};

/// The three streams of a shell session: the session stream the handshake and
/// exit frame travel on, and the stdout and stderr streams.
pub(super) struct SessionStreams {
    pub(super) send: quinn::SendStream,
    pub(super) stdout: quinn::SendStream,
    pub(super) stderr: quinn::SendStream,
}

impl SessionStreams {
    // spec: SHL#session-end
    /// Ends the session: writes the exit frame, finishes the session stream and
    /// then finishes stderr; stdout is finished too if the attachment has not
    /// already. `reason` is why a session that failed ended, and is written to
    /// stderr for the client to show.
    pub(super) async fn end(mut self, exit_code: i32, reason: Option<&str>) {
        let _ = self.stdout.finish();
        if let Some(reason) = reason {
            let _ = self
                .stderr
                .write_all(format!("{reason}\n").as_bytes())
                .await;
        }
        let _ = write_frame(&mut self.send, &exit_frame(exit_code)).await;
        let _ = self.send.finish();
        let _ = self.stderr.finish();
    }
}

/// A session's streams as a whole: the three it ends with, and the client's
/// side of the session stream, which carries stdin while the job is attached.
pub(super) struct SessionIo {
    pub(super) recv: quinn::RecvStream,
    pub(super) leftover_stdin: Vec<u8>,
    pub(super) streams: SessionStreams,
}

// spec: WIRE#shell-sessions
/// A frame as it goes on the wire: one JSON line, newline included.
fn frame_bytes(frame: &serde_json::Value) -> Vec<u8> {
    let mut line = frame.to_string().into_bytes();
    line.push(b'\n');
    line
}

fn exit_frame(exit_code: i32) -> serde_json::Value {
    serde_json::json!({ "exit_code": exit_code })
}

fn error_frame(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

pub(super) async fn write_frame(
    send: &mut quinn::SendStream,
    frame: &serde_json::Value,
) -> Result<(), quinn::WriteError> {
    send.write_all(&frame_bytes(frame)).await
}

/// Answers a `/shells/start` that failed before the session opened.
pub(super) async fn reject(send: &mut quinn::SendStream, code: &str, message: impl AsRef<str>) {
    let _ = write_frame(send, &error_frame(code, message.as_ref())).await;
    let _ = send.finish();
}

pub(crate) async fn open_shell_session(
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    recv: quinn::RecvStream,
    leftover_stdin: Vec<u8>,
    initial_line: Vec<u8>,
    state: Arc<OiState>,
    events: EventSenderWithActor,
) {
    // spec: SHL#opening-an-app-shell
    #[derive(serde::Deserialize)]
    struct Params {
        app: AppName,
        name: ShellName,
        rows: u16,
        cols: u16,
        #[serde(default)]
        params: Option<serde_json::Map<String, serde_json::Value>>,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(default)]
        params: serde_json::Value,
    }

    let req: Request = match serde_json::from_slice(&initial_line) {
        Ok(r) => r,
        Err(e) => {
            reject(
                &mut send,
                "requirements_invalid",
                format!("invalid request: {e}"),
            )
            .await;
            return;
        }
    };
    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            reject(
                &mut send,
                "requirements_invalid",
                format!("invalid params: {e}"),
            )
            .await;
            return;
        }
    };
    let app_name = params.app;
    let shell_name = params.name;
    let initial_rows = params.rows;
    let initial_cols = params.cols;
    let submitted_params = params.params.unwrap_or_default();

    // spec: SHL#opening-an-app-shell
    // spec: ACN#param-delivery
    if let Some(reserved) = submitted_params
        .keys()
        .find(|k| k.ends_with("_volume") || k.ends_with("_filename"))
    {
        reject(
            &mut send,
            "requirements_invalid",
            format!(
                "param key {reserved:?} is reserved (keys ending in _volume or _filename are reserved)"
            ),
        )
        .await;
        return;
    }

    let session_id = SessionId::generate();
    // All registry access is done in a synchronous closure so no lock guard
    // crosses an await point (parking_lot guards are not Send).
    let lookup: Result<_, (&str, String)> = (|| {
        let reg = state.registry.read();
        let Some(entry) = reg.get(app_name.as_str()) else {
            return Err(("not_found", format!("app not found: {app_name}")));
        };
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(("not_installed", format!("app is not installed: {app_name}")));
        }
        let shell_params_schema = {
            let def = entry.app.def.load();
            match def.shells.get(&shell_name) {
                Some(s) => s.params.clone(),
                None => return Err(("not_found", format!("shell not found: {shell_name}"))),
            }
        };
        // spec: RCN#desired-state
        // spec: APPS#removal
        // The slot is opened under the same look at the registry that found
        // the app installed, so the app cannot be removed between the two,
        // and stands until the session's dynamic resources are gone after it
        // has ended.
        let slot = entry.sessions.open(session_id);
        Ok((
            entry.app.clone(),
            shell_params_schema,
            Arc::clone(&entry.sessions),
            slot,
            Arc::clone(&entry.tick_notify),
        ))
    })();
    let (app, shell_params_schema, sessions, slot, tick_notify) = match lookup {
        Ok(v) => v,
        Err((code, msg)) => {
            reject(&mut send, code, msg).await;
            return;
        }
    };
    // spec: OPS#shell-sessions
    // However the session ends, refused before it began, panicking or dropped
    // included, its resources are torn down and its slot let go.
    let teardown = SessionTeardown {
        state: Arc::clone(&state),
        app_name: app_name.clone(),
        session_id,
        sessions,
        slot: Arc::clone(&slot),
        tick_notify: Arc::clone(&tick_notify),
    };

    // spec: ACN#param-delivery
    // spec: SHL#opening-an-app-shell
    // A shell's params are validated as an action's are, by the same code:
    // the schema fills defaults and checks kinds, `volume` params must name a
    // site volume, and params the schema does not declare reach the closure.
    let validated_params =
        match validate_shell_params(&state, &shell_params_schema, submitted_params) {
            Ok(params) => params,
            Err(e) => {
                let code = serde_json::to_value(e.code)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "requirements_invalid".to_owned());
                reject(&mut send, &code, e.message).await;
                return;
            }
        };

    // spec: WIRE#shell-sessions
    let (stdout_send, stdout_stream_id) = match conn.open_uni().await {
        Ok(s) => {
            let id = s.id().index();
            (s, id)
        }
        Err(e) => {
            tracing::warn!("open stdout uni stream: {e}");
            let _ = send.finish();
            return;
        }
    };
    let (stderr_send, stderr_stream_id) = match conn.open_uni().await {
        Ok(s) => {
            let id = s.id().index();
            (s, id)
        }
        Err(e) => {
            tracing::warn!("open stderr uni stream: {e}");
            let _ = send.finish();
            return;
        }
    };

    let stop = StopSignal::new();
    let session = ShellSession::starting(
        session_id,
        app_name.clone(),
        shell_name.as_str().to_owned(),
        Some((*events.actor).clone()),
        (initial_rows, initial_cols),
        stop.clone(),
    );
    if !begin_session(
        &state,
        &events,
        &mut send,
        session,
        &shell_name,
        stdout_stream_id,
        stderr_stream_id,
    )
    .await
    {
        return;
    }
    let host = Arc::new(AppShellHost {
        handle: tokio::runtime::Handle::current(),
        state: Arc::clone(&state),
        session_id,
        app_name: app_name.clone(),
        shell_name: shell_name.clone(),
        stop: stop.clone(),
        io: Mutex::new(Some(SessionIo {
            recv,
            leftover_stdin,
            streams: SessionStreams {
                send,
                stdout: stdout_send,
                stderr: stderr_send,
            },
        })),
        job_exit: Mutex::new(None),
    });

    let run = run_app_shell(
        &state,
        app,
        &app_name,
        &shell_name,
        &shell_params_schema,
        validated_params,
        &stop,
        Arc::clone(&host) as Arc<dyn ShellHost>,
        SessionProgress {
            id: session_id,
            slot: Arc::clone(&slot),
            tick_notify: Arc::clone(&tick_notify),
        },
    )
    .await;
    let outcome = session_outcome(&run, stop.is_requested(), *host.job_exit.lock());

    state.shells.remove(&session_id);
    // spec: SHL#session-end
    events.shell_exited(session_id, outcome.exit_code);
    let io = host.io.lock().take();
    if let Some(io) = io {
        io.streams
            .end(outcome.exit_code, outcome.reason.as_deref())
            .await;
    }

    tracing::info!(
        app = %app_name, shell = %shell_name, exit_code = %outcome.exit_code,
        "shell session ended"
    );

    // spec: OPS#shell-sessions
    // The session is over for its client; tearing its dynamic resources down
    // takes as long as it takes.
    drop(teardown);
}

/// Starts tearing down a session's dynamic resources when it is dropped,
/// which is when the session has ended, by whatever path.
pub(super) struct SessionTeardown {
    pub(super) state: Arc<OiState>,
    pub(super) app_name: AppName,
    pub(super) session_id: SessionId,
    pub(super) sessions: Arc<SessionSlots>,
    pub(super) slot: DynamicSlot,
    pub(super) tick_notify: Arc<Notify>,
}

impl Drop for SessionTeardown {
    // spec: OPS#shell-sessions
    fn drop(&mut self) {
        let teardown = tear_down_session_resources(
            Arc::clone(&self.state),
            self.app_name.clone(),
            self.session_id,
            Arc::clone(&self.sessions),
            Arc::clone(&self.slot),
            Arc::clone(&self.tick_notify),
        );
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(teardown);
            }
            // Only when the runtime itself is going: the records stay, and
            // the next start tears their resources down.
            Err(e) => {
                tracing::error!(
                    app = %self.app_name, session = %self.session_id,
                    "cannot tear down the session's dynamic resources now: {e}"
                );
            }
        }
    }
}

/// Tears down the dynamic resources a session that has ended created, then
/// lets go of its slot in the app's desired state.
// spec: APP#dynamic-resource-lifetime
// spec: OPS#shell-sessions
pub(super) async fn tear_down_session_resources(
    state: Arc<OiState>,
    app_name: AppName,
    session_id: SessionId,
    sessions: Arc<SessionSlots>,
    slot: DynamicSlot,
    tick_notify: Arc<Notify>,
) {
    cleanup_dynamic_resources(
        &state.db,
        &state.driver,
        &app_name,
        &session_id.to_string(),
        &slot,
        &tick_notify,
    )
    .await;
    sessions.close(&session_id);
    tick_notify.notify_one();
}

/// Validate the params a client submitted for a shell, as an action's are.
///
/// The schema fills defaults and checks kinds, `volume` params must name a
/// site volume, and params the schema does not declare reach the closure.
// spec: ACN#param-delivery
pub(super) fn validate_shell_params(
    state: &OiState,
    schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::param_def::ParamDef,
    >,
    submitted: serde_json::Map<String, serde_json::Value>,
) -> Result<ParamMap, seedling_protocol::error::OiError> {
    let params = crate::runtime::action_params::apply_schema(
        schema,
        submitted,
        &mut crate::defs::regex::Budget::of(&state.script_limits.helpers()),
    )
    .map_err(crate::oi::handler::actions::validation_error_to_oi)?;
    crate::runtime::action_params::validate_volume_params(
        schema,
        &params,
        &crate::oi::handler::actions::StateVolumeLookup(state),
    )
    .map_err(crate::oi::handler::actions::validation_error_to_oi)?;
    Ok(params)
}

/// Why a session ended before its job was running.
pub(super) struct StartupFailure {
    /// What went wrong, for the client; `None` when the session was stopped.
    pub(super) reason: Option<String>,
    /// The pod network to remove, if one had been created.
    network: Option<String>,
}

impl StartupFailure {
    pub(super) fn stopped(network: Option<String>) -> Self {
        Self {
            reason: None,
            network,
        }
    }

    pub(super) fn failed(reason: impl Into<String>, network: Option<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            network,
        }
    }
}

// spec: SHL#session-startup
/// Registers the session under its id, then tells the client the id. The id is
/// never given out for a session that cannot be listed, resized or stopped.
/// Returns `false`, with the session gone again, if the client could not be
/// told.
pub(super) async fn begin_session(
    state: &OiState,
    events: &EventSenderWithActor,
    send: &mut quinn::SendStream,
    session: ShellSession,
    event_name: &ShellName,
    stdout_stream_id: u64,
    stderr_stream_id: u64,
) -> bool {
    let session_id = session.session_id;
    let app = session.app.clone();
    state.shells.insert(session);

    // spec: SHL#opening-an-app-shell
    let handshake = write_frame(
        send,
        &serde_json::json!({
            "result": {
                "session_id": session_id.to_string(),
                "stdout_stream_id": stdout_stream_id,
                "stderr_stream_id": stderr_stream_id,
            }
        }),
    )
    .await;
    if let Err(e) = handshake {
        tracing::warn!("write handshake: {e}");
        state.shells.remove(&session_id);
        let _ = send.finish();
        return false;
    }

    // spec: SHL#session-events
    events.shell_started(session_id, &app, event_name);
    true
}

/// Ends a session whose job never ran: it was stopped, or something it needed
/// failed.
pub(super) async fn abort_startup(
    state: &OiState,
    events: &EventSenderWithActor,
    session_id: SessionId,
    failure: StartupFailure,
    streams: SessionStreams,
) {
    if let Some(network) = &failure.network {
        super::remove_session_network(state, network).await;
    }
    state.shells.remove(&session_id);
    // spec: SHL#session-end
    events.shell_exited(session_id, -1);
    streams.end(-1, failure.reason.as_deref()).await;
}

/// Pulls the shell's image if it is not already present.
async fn ensure_image(
    state: &OiState,
    app_name: &AppName,
    shell_name: &ShellName,
    image: &str,
) -> Result<(), String> {
    match state.container_runtime.image_exists(image).await {
        Ok(false) => {
            tracing::info!(app = %app_name, shell = %shell_name, %image, "pulling shell image");
            state
                .container_runtime
                .pull_image(image)
                .await
                .map_err(|e| {
                    tracing::warn!(app = %app_name, shell = %shell_name, %image, "image pull failed: {e}");
                    format!("image pull failed: {e}")
                })
        }
        Err(e) => {
            tracing::warn!(app = %app_name, %image, "image_exists check failed: {e}");
            Ok(())
        }
        Ok(true) => Ok(()),
    }
}

/// How a shell's closure ended.
pub(super) enum ScriptEnd {
    Completed,
    Failed(String),
    /// The session was stopped while the script waited.
    Cancelled,
}

/// A shell's closure run to its end, and what it did.
pub(super) struct ShellRun {
    pub(super) end: ScriptEnd,
    pub(super) progress: Arc<ShellProgress>,
}

/// What a session's closure shares with the reconciler: the session's id, the
/// slot its dynamic resources are placed in, and the wake-up for the
/// reconciler.
pub(super) struct SessionProgress {
    pub(super) id: SessionId,
    pub(super) slot: DynamicSlot,
    pub(super) tick_notify: Arc<Notify>,
}

/// How a session ended: the exit code for its exit frame, and why if it failed.
pub(super) struct SessionOutcome {
    pub(super) exit_code: i32,
    pub(super) reason: Option<String>,
}

// spec: SHL#session-end
// spec: SHL#session-startup
/// Reads how a session ended from how its closure did.
///
/// The session ends when the closure returns. A closure that failed, called
/// `shell.error`, or returned without attaching, is a failed session; so is a
/// job that could not be attached. Otherwise the code is the job's.
pub(super) fn session_outcome(
    run: &ShellRun,
    stopped: bool,
    job_exit: Option<i32>,
) -> SessionOutcome {
    let failed = |reason: String| SessionOutcome {
        exit_code: -1,
        reason: Some(reason),
    };
    if let Some(message) = run.progress.error() {
        return failed(message);
    }
    match &run.end {
        ScriptEnd::Failed(reason) => failed(
            run.progress
                .attach_failure()
                .unwrap_or_else(|| reason.clone()),
        ),
        ScriptEnd::Cancelled => SessionOutcome {
            exit_code: job_exit.unwrap_or(-1),
            reason: None,
        },
        ScriptEnd::Completed if run.progress.attached() => SessionOutcome {
            exit_code: job_exit.unwrap_or(-1),
            reason: None,
        },
        ScriptEnd::Completed if stopped => SessionOutcome {
            exit_code: -1,
            reason: None,
        },
        ScriptEnd::Completed => failed("the shell did not attach".to_owned()),
    }
}

// spec: ACN#shell-control
/// Runs the shell's closure to its end, on a thread of its own. The closure
/// suspends in `shell.attach` while `host` runs the job, so this returns only
/// once the closure has returned; the session ends then.
#[expect(
    clippy::too_many_arguments,
    reason = "a shell session's whole state, each piece of which the run uses"
)]
pub(super) async fn run_app_shell(
    state: &OiState,
    app: App,
    app_name: &AppName,
    shell_name: &ShellName,
    schema: &std::collections::BTreeMap<
        seedling_protocol::names::ParamName,
        crate::defs::param_def::ParamDef,
    >,
    validated_params: ParamMap,
    stop: &StopSignal,
    host: Arc<dyn ShellHost>,
    session: SessionProgress,
) -> ShellRun {
    let progress = Arc::new(ShellProgress::default());
    // spec: OPS#shell-sessions
    // The session's id stands where an operation's does, in the action log and
    // against the dynamic resources the closure creates.
    let operation_id = OperationId(session.id.to_string());
    let SessionProgress {
        slot, tick_notify, ..
    } = session;
    // spec: ACN#volume-params
    // A binding that cannot be made fails the session without running the
    // closure.
    let crate::runtime::operations::BoundParams {
        params: validated_params,
        bindings: operation_volume_bindings,
    } = match crate::runtime::operations::bind_volume_params(
        &state.engine,
        schema,
        validated_params,
        &operation_id.0,
    ) {
        Ok(bound) => bound,
        Err(why) => {
            return ShellRun {
                end: ScriptEnd::Failed(format!("a volume param could not be bound: {why}")),
                progress,
            };
        }
    };
    let progress_for_task = Arc::clone(&progress);
    let db_for_task = state.db.clone();
    let script_limits = state.script_limits.clone();
    let cipher = Arc::clone(&state.cipher);
    let op_id_for_log = operation_id.clone();
    let app_name_for_task = app_name.clone();
    let shell_name_for_task = shell_name.clone();
    let cancel = stop.cancel_token();
    let gate_runtime = Arc::clone(&state.gate_runtime);
    let apps = Arc::clone(&state.registry);

    let script = tokio::task::spawn_blocking(move || {
        let (engine, mut scope, _) =
            match crate::setup_language(&script_limits, Arc::clone(&app.bundle)) {
                Ok(language) => language,
                // spec: CONST#host-constants
                Err(e) => {
                    tracing::error!(
                        app = %app_name_for_task, shell = %shell_name_for_task,
                        "shell script cannot be evaluated: {e}"
                    );
                    return ScriptEnd::Failed(format!(
                        "the shell's script cannot be evaluated: {e}"
                    ));
                }
            };
        let compiled = app
            .bundle
            .script()
            .map_err(|e| e.to_string())
            .and_then(|script| {
                crate::defs::compile(&engine, script.text())
                    .map_err(|e| script.remap_error(&e.to_string()))
            });
        let ast = match compiled {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(
                    app = %app_name_for_task, shell = %shell_name_for_task,
                    "shell script compile error: {e}"
                );
                return ScriptEnd::Failed(format!("the shell's script does not compile: {e}"));
            }
        };
        let db = db_for_task;
        let registry: Arc<dyn crate::runtime::InstanceRegistry> =
            Arc::new(DbInstanceRegistry::new(db.clone()));
        // Shell names follow the same bsl.name rules as action names; reuse
        // ActionName as the identifier written into the action log.
        let shell_as_action =
            OperationTarget::Action(ActionName::new_unchecked(shell_name_for_task.as_str()));
        let log = DbActionLog::new(
            db.clone(),
            op_id_for_log.clone(),
            app_name_for_task.clone(),
            shell_as_action,
        );
        let world = Arc::new(DbWorldOracle::new(db.clone()));
        // spec: SEC#operation-seeds — a session's seed lives as long as the
        // session and is never persisted.
        let operation_seed = match crate::runtime::seeds::Seed::draw() {
            Ok(seed) => seed,
            Err(e) => {
                tracing::error!(
                    app = %app_name_for_task, shell = %shell_name_for_task,
                    "no seed could be drawn for the shell session: {e:?}"
                );
                return ScriptEnd::Failed("no seed could be drawn for the session".into());
            }
        };
        // spec: SEC#matching
        let sensitive = {
            let (name, cipher, def, stored) = (
                app_name_for_task.clone(),
                Arc::clone(&cipher),
                app.def.load_full(),
                app.stored.lock().clone(),
            );
            match db.call(move |db| {
                crate::runtime::apps::sensitive_values(db, &cipher, &name, &def, &stored)
            }) {
                Ok(values) => crate::runtime::redact::Scope::new(values),
                Err(e) => {
                    tracing::error!(
                        app = %app_name_for_task, shell = %shell_name_for_task,
                        "the app's sensitive values cannot be read: {e}"
                    );
                    return ScriptEnd::Failed("the app's sensitive values cannot be read".into());
                }
            }
        };
        {
            let def = app.def.load();
            let schema = def.shells.get(&shell_name_for_task).map(|s| &s.params);
            sensitive.insert_secret_params(validated_params.values(), |name| {
                schema
                    .and_then(|schema| schema.get(name))
                    .is_some_and(|p| p.is_secret())
            });
        }
        set_shell_attach_ctx(ShellAttachCtx {
            app_name: app_name_for_task.clone(),
            host,
            progress: progress_for_task,
        });

        let end = loop {
            // spec: RT#resources-a-gate-holds
            // What gates keep from the closure is read for each pass, as an
            // operation's is, so a pass that follows a wait sees the gates as
            // they stand then.
            let exclusions = {
                let (name, def, cipher, remembered) = (
                    app_name_for_task.clone(),
                    app.def.load_full(),
                    Arc::clone(&cipher),
                    gate_runtime.remembered(&app_name_for_task),
                );
                let current =
                    crate::runtime::gates::CurrentDefinition::of(&apps, &app_name_for_task);
                db.call(move |db| {
                    crate::runtime::gates::Exclusions::for_closure(
                        db,
                        &name,
                        &def,
                        current.as_ref(),
                        &cipher,
                        &remembered,
                        None,
                    )
                })
            };
            let result = run_operation(
                OperationContext {
                    engine: &engine,
                    script_ast: &ast,
                    operation_id: op_id_for_log.clone(),
                    app: &app,
                    subject: Subject::Shell(shell_name_for_task.as_str()),
                    operation_seed: operation_seed.clone(),
                    sensitive: sensitive.clone(),
                    log: &log,
                    world: Arc::clone(&world),
                    registry: Arc::clone(&registry),
                    active_progress: Some(Arc::clone(&slot)),
                    tick_notify: Some(Arc::clone(&tick_notify)),
                    params: validated_params.clone(),
                    // spec: OPS#shell-sessions
                    // The dynamic resources the closure starts are recorded
                    // against the session before they are started.
                    db: Some(db.clone()),
                    cipher: Some(Arc::clone(&cipher)),
                    operation_volume_bindings: operation_volume_bindings.clone(),
                    // Shell sessions don't participate in operator-initiated
                    // cancellation; the token fires only when the session is
                    // stopped, which ends the script early.
                    cancel_token: Arc::clone(&cancel),
                    container_signaler: None,
                    volume_writer: None,
                    executor: None,
                    exclusions,
                },
                &mut scope,
            );
            match result {
                OperationResult::Completed => break ScriptEnd::Completed,
                OperationResult::Failed(e) => {
                    // spec: SEC#withheld-values
                    let why = sensitive.values().mask(&e.to_string()).into_owned();
                    tracing::error!(
                        app = %app_name_for_task, shell = %shell_name_for_task,
                        "shell closure failed: {why}"
                    );
                    break ScriptEnd::Failed(format!("the shell's script failed: {why}"));
                }
                // spec: OPS#cancellation
                OperationResult::Cancelled => break ScriptEnd::Cancelled,
                OperationResult::Suspended(_) => cancel.wait_for(Duration::from_secs(2)),
            }
        };

        clear_shell_attach_ctx();
        end
    });

    // spec: SHL#session-startup
    // The script's thread cannot be abandoned: a stop reaches it at its next
    // wait, or through the attachment it is blocked in, and until it returns
    // it may still be creating things the teardown would miss.
    let end = script
        .await
        .unwrap_or_else(|e| ScriptEnd::Failed(format!("the shell's script panicked: {e}")));
    ShellRun { end, progress }
}

/// Runs a shell's job for its closure's `shell.attach`.
struct AppShellHost {
    handle: tokio::runtime::Handle,
    state: Arc<OiState>,
    session_id: SessionId,
    app_name: AppName,
    shell_name: ShellName,
    stop: StopSignal,
    /// The session's streams, held here while no attachment is using them.
    io: Mutex<Option<SessionIo>>,
    /// The exit code the job gave, once it has been attached and ended.
    job_exit: Mutex<Option<i32>>,
}

impl ShellHost for AppShellHost {
    // spec: ACN#shell-control
    fn attach(&self, target: ShellExecTarget) -> Result<ShellExit, String> {
        let Some(mut io) = self.io.lock().take() else {
            return Err("the session's streams are no longer available".to_owned());
        };
        let attached = self.handle.block_on(self.run_job(target, &mut io));
        // spec: SHL#session-end
        let _ = io.streams.stdout.finish();
        *self.io.lock() = Some(io);
        match attached? {
            Some(end) => {
                *self.job_exit.lock() = Some(end.exit_code);
                Ok(if end.exited {
                    ShellExit::exited(end.exit_code)
                } else {
                    ShellExit::ended()
                })
            }
            None => Ok(ShellExit::ended()),
        }
    }
}

impl AppShellHost {
    /// Starts the target's job and runs the attachment to its end. Returns
    /// `None` if the session was stopped before the job ran.
    ///
    /// A stop is acted on wherever the startup waits, and between the steps
    /// that are not interrupted part-way: creating the network and starting the
    /// job.
    async fn run_job(
        &self,
        exec_target: ShellExecTarget,
        io: &mut SessionIo,
    ) -> Result<Option<AttachmentEnd>, String> {
        let state = &*self.state;
        let app_name = &self.app_name;
        let shell_name = &self.shell_name;
        let stop = &self.stop;

        let instance = ResourceInstance {
            id: exec_target.instance_id,
            app: exec_target.app_name.clone(),
            kind: ResourceKind::Job,
            name: Some(exec_target.job_name.clone()),
            variant: InstanceVariant::Scaled,
            display_name: format!(
                "{}-{}-{}",
                exec_target.app_name,
                exec_target.job_name,
                exec_target.instance_id.display_suffix()
            ),
        };
        let container_name = instance.display_name.clone();

        let image = exec_target
            .job_def
            .pod
            .lock()
            .container
            .lock()
            .image
            .clone()
            .unwrap_or_default();
        if !image.is_empty() {
            // spec: SHL#session-startup
            tokio::select! {
                _ = stop.requested() => return Ok(None),
                ensured = ensure_image(state, app_name, shell_name, &image) => ensured?,
            }
        }

        if stop.is_requested() {
            return Ok(None);
        }

        let net_name = format!("seedling-{}", instance.display_name);
        let net_prefix = super::allocate_session_prefix(state, &net_name, instance.kind)
            .map_err(|e| format!("create network failed: {e}"))?;
        if let Err(e) = state
            .container_runtime
            .create_network(&net_name, net_prefix, None)
            .await
        {
            super::remove_session_network(state, &net_name).await;
            tracing::warn!(app = %app_name, shell = %shell_name, %net_name, "create network failed: {e}");
            return Err(format!("create network failed: {e}"));
        }

        let resolved_mounts: Vec<(u16, Ipv6Addr, u16)> = {
            let service_mounts = exec_target.job_def.pod.lock().service_mounts.clone();
            if service_mounts.is_empty() {
                vec![]
            } else {
                let mount_registry = DbInstanceRegistry::new(state.db.clone());
                let result: Result<Vec<_>, RegistryError> = service_mounts
                    .iter()
                    .map(|sp| {
                        let svc_instance = mount_registry.get_or_create_singleton(
                            &exec_target.app_name,
                            ResourceKind::Service,
                            Some(sp.service.name().as_str()),
                        )?;
                        let svc_ip =
                            mount_registry.service_address(&state.node_prefix, &svc_instance)?;
                        Ok((sp.port.get(), svc_ip, sp.port.get()))
                    })
                    .collect();
                match result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(app = %app_name, shell = %shell_name, "service mount registry lookup failed: {e}");
                        super::remove_session_network(state, &net_name).await;
                        return Err(format!("service mount lookup failed: {e}"));
                    }
                }
            }
        };

        // spec: ACN#shells
        // Match the reconciler's volume/external-volume resolution so a shell
        // session sees the same on-disk state as a Job (bind-mounts under the
        // volume store directory, same external-volume mappings). Previously we
        // passed None for volumes_dir and an empty external_volumes map, which
        // silently swapped bind-mounts for fresh podman-managed volumes with
        // matching names — the shell would open inside a pristine empty volume
        // and operators would see no data from the app's real state.
        let external_volumes = crate::system::actuator::resolve_external_volumes(
            &state.db,
            &state.driver.volume_store,
            app_name,
        );
        let mut container_spec = job_spec(
            &exec_target.job_def,
            &instance,
            &BTreeMap::new(),
            &(net_name.clone(), net_prefix),
            &resolved_mounts,
            &state.dns_servers,
            Some(state.driver.volume_store.volumes_dir()),
            &external_volumes,
            &ConfigStamp::default(),
        );
        container_spec.health = None;
        container_spec.labels.insert(
            crate::system::types::SESSION_LABEL.to_string(),
            "shell".to_string(),
        );

        // spec: SHL#session-startup
        if stop.is_requested() {
            super::remove_session_network(state, &net_name).await;
            return Ok(None);
        }

        match state.container_runtime.exec(container_spec).await {
            Ok(exec) => Ok(Some(
                run_attachment(
                    state,
                    self.session_id,
                    stop,
                    LiveShell {
                        exec,
                        container_name,
                        net_name,
                    },
                    io,
                )
                .await,
            )),
            Err(e) => {
                tracing::warn!(app = %app_name, shell = %shell_name, "exec failed: {e}");
                super::remove_session_network(state, &net_name).await;
                Err(format!("exec failed: {e}"))
            }
        }
    }
}

/// A started shell: its running job and the names of what must be removed once
/// the session ends.
pub(super) struct LiveShell {
    pub(super) exec: ExecHandle,
    pub(super) container_name: String,
    pub(super) net_name: String,
}

/// How an attachment ended.
pub(super) struct AttachmentEnd {
    /// Whether the job's process exited, as opposed to the attachment ending
    /// first.
    pub(super) exited: bool,
    /// The job's exit status, or the one forced on it when the session failed.
    pub(super) exit_code: i32,
}

// spec: SHL#session-end
/// The exit status of a finished process: its exit code, or 128 plus the
/// signal that ended it.
fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(-1)
}

/// Relays the session's input and output to its job until the attachment ends,
/// then stops the job, finishes stdout and removes the job's network.
///
/// Every way an attachment can end comes through here, so none can leave the
/// job or its network behind.
pub(super) async fn run_attachment(
    state: &OiState,
    session_id: SessionId,
    stop: &StopSignal,
    mut live: LiveShell,
    io: &mut SessionIo,
) -> AttachmentEnd {
    // The job has exited on its own, with this status, or it has to be stopped
    // and the exit code is the one its termination gives (or the one forced).
    let job_exit: Result<i32, Option<i32>> = 'run: {
        // spec: SHL#session-startup
        // A stop that arrived while the job was starting has already removed
        // the session; the job is stopped as soon as it is known.
        let attached = state.shells.attach(
            &session_id,
            live.container_name.clone(),
            live.exec.pty_master_fd,
        );
        if !attached {
            break 'run Err(Some(-1));
        }

        if !io.leftover_stdin.is_empty()
            && live.exec.stdin.write_all(&io.leftover_stdin).await.is_err()
        {
            break 'run Err(Some(-1));
        }

        // spec: SHL#session-end
        match relay_session(&mut io.recv, &mut live.exec, &mut io.streams.stdout, stop).await {
            Some(Ok(status)) => Ok(exit_status_code(status)),
            Some(Err(_)) | None => Err(None),
        }
    };

    let (exited, exit_code) = match job_exit {
        Ok(code) => (true, code),
        Err(forced) => {
            // spec: SHL#session-end
            if let Some(pid) = live.exec.child.id() {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }

            let graceful =
                tokio::time::timeout(Duration::from_secs(5), live.exec.child.wait()).await;
            let terminated = match graceful {
                Ok(status) => status.map_or(-1, exit_status_code),
                Err(_timeout) => {
                    let _ = state
                        .container_runtime
                        .remove_container(&live.container_name, true)
                        .await;
                    let _ = live.exec.child.kill().await;
                    live.exec.child.wait().await.map_or(-1, exit_status_code)
                }
            };
            (false, forced.unwrap_or(terminated))
        }
    };

    // spec: SHL#session-end
    let _ = io.streams.stdout.finish();
    state.shells.detach(&session_id);
    super::remove_session_network(state, &live.net_name).await;
    AttachmentEnd { exited, exit_code }
}

/// Runs a registered session whose job is already started to its end and tears
/// down everything it holds: the job, the session's registry entry, its pod
/// network and its streams. Returns the exit code reported to the client.
pub(super) async fn run_to_exit(
    state: &OiState,
    events: &EventSenderWithActor,
    session_id: SessionId,
    stop: &StopSignal,
    live: LiveShell,
    mut io: SessionIo,
) -> i32 {
    let end = run_attachment(state, session_id, stop, live, &mut io).await;
    state.shells.remove(&session_id);
    // spec: SHL#session-end
    events.shell_exited(session_id, end.exit_code);
    io.streams.end(end.exit_code, None).await;
    end.exit_code
}

// spec: WIRE#shell-sessions
/// Relay stdin from the session stream into the job and the job's output onto
/// the stdout stream until the session ends. Returns the job's exit status when
/// the job exiting is what ended it, and `None` when the client closed stdin,
/// the output ended or failed, or a stop was requested.
///
/// Each direction is its own future, so output the client is slow to take
/// never stops stdin being relayed, nor the job's exit or a stop being seen.
pub(super) async fn relay_session(
    recv: &mut quinn::RecvStream,
    exec: &mut ExecHandle,
    stdout_send: &mut quinn::SendStream,
    stop: &StopSignal,
) -> Option<std::io::Result<std::process::ExitStatus>> {
    let ExecHandle {
        stdin,
        stdout,
        child,
        ..
    } = exec;
    tokio::select! {
        status = child.wait() => Some(status),
        _ = tokio::io::copy(recv, stdin) => None,
        _ = tokio::io::copy(stdout, stdout_send) => None,
        _ = stop.requested() => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: WIRE#shell-sessions
    #[test]
    fn every_frame_is_one_newline_terminated_line() {
        for frame in [
            exit_frame(0),
            exit_frame(-1),
            error_frame("not_found", "no such shell"),
            serde_json::json!({ "result": { "session_id": "x" } }),
        ] {
            let bytes = frame_bytes(&frame);
            assert_eq!(bytes.last(), Some(&b'\n'), "{frame}");
            assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1, "{frame}");
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(parsed, frame);
        }
    }

    // spec: WIRE#shell-sessions
    #[test]
    fn error_text_with_newlines_stays_on_one_line() {
        let bytes = frame_bytes(&error_frame("shell_error", "line one\nline two"));
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    }

    // spec: SHL#session-end
    #[test]
    fn exit_frame_carries_only_the_exit_code() {
        assert_eq!(exit_frame(-1), serde_json::json!({ "exit_code": -1 }));
    }
}
