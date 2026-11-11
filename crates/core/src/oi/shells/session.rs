use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc, time::Duration};

use parking_lot::Mutex;
use seedling_protocol::names::{ActionName, AppName, SessionId, ShellName};
use tokio::io::AsyncWriteExt;

use super::registry::{ShellSession, StopSignal};
use crate::{
    defs::{app::App, resource::ResourceKind},
    oi::state::OiState,
    runtime::{
        AppPhase,
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, run_operation},
            shell::{ShellAttachCtx, ShellOutcome, clear_shell_attach_ctx, set_shell_attach_ctx},
        },
        identity::{InstanceVariant, ResourceInstance},
        registry::{DbInstanceRegistry, InstanceRegistry, RegistryError},
    },
    system::{
        translate::{
            container::job_spec,
            proxy::{instance_ipv6, pod_network_prefix},
        },
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
    /// Ends the session: writes the exit frame and closes all three streams.
    /// `reason` is why a session that failed to start ended, and is written to
    /// stderr for the client to show.
    pub(super) async fn end(mut self, exit_code: i32, reason: Option<&str>) {
        if let Some(reason) = reason {
            let _ = self
                .stderr
                .write_all(format!("{reason}\n").as_bytes())
                .await;
        }
        let _ = self.stderr.finish();
        let _ = self.stdout.finish();
        let _ = write_frame(&mut self.send, &exit_frame(exit_code)).await;
        let _ = self.send.finish();
    }
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
    mut recv: quinn::RecvStream,
    leftover_stdin: Vec<u8>,
    initial_line: Vec<u8>,
    state: Arc<OiState>,
) {
    // spec: SHL#opening-an-app-shell
    #[derive(serde::Deserialize)]
    struct Params {
        app: AppName,
        name: ShellName,
        rows: u16,
        cols: u16,
        #[serde(default)]
        params: Option<std::collections::BTreeMap<String, String>>,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(default)]
        actor: Option<seedling_protocol::actor::Actor>,
        #[serde(default)]
        params: serde_json::Value,
    }

    let req: Request = match serde_json::from_slice(&initial_line) {
        Ok(r) => r,
        Err(e) => {
            reject(&mut send, "not_found", format!("invalid request: {e}")).await;
            return;
        }
    };
    let req_actor = req.actor;
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
        Ok((entry.app.clone(), shell_params_schema))
    })();
    let (app, shell_params_schema) = match lookup {
        Ok(v) => v,
        Err((code, msg)) => {
            reject(&mut send, code, msg).await;
            return;
        }
    };

    let validated_params: serde_json::Map<String, serde_json::Value> =
        if shell_params_schema.is_empty() {
            serde_json::Map::new()
        } else {
            match crate::oi::handler::actions::install::validate_requirements(
                &shell_params_schema,
                &submitted_params,
            ) {
                Ok(filled) => filled
                    .into_iter()
                    .map(|(k, v)| (k, serde_json::Value::String(v)))
                    .collect(),
                Err(e) => {
                    reject(&mut send, "requirements_invalid", e.message).await;
                    return;
                }
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

    let session_id = SessionId::generate();
    let stop = StopSignal::new();
    let session = ShellSession::starting(
        session_id,
        app_name.clone(),
        shell_name.as_str().to_owned(),
        req_actor,
        (initial_rows, initial_cols),
        stop.clone(),
    );
    if !begin_session(
        &state,
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
    let streams = SessionStreams {
        send,
        stdout: stdout_send,
        stderr: stderr_send,
    };

    let live =
        match start_app_shell(&state, app, &app_name, &shell_name, validated_params, &stop).await {
            Ok(live) => live,
            Err(failure) => {
                abort_startup(&state, session_id, failure, streams).await;
                return;
            }
        };

    let exit_code = run_to_exit(
        &state,
        session_id,
        &stop,
        live,
        &mut recv,
        &leftover_stdin,
        streams,
    )
    .await;

    tracing::info!(app = %app_name, shell = %shell_name, %exit_code, "shell session ended");
}

/// Why a session ended before its job was running.
pub(super) struct StartupFailure {
    /// What went wrong, for the client; `None` when the session was stopped.
    reason: Option<String>,
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
    state.event_tx.shell_started(session_id, &app, event_name);
    true
}

/// Ends a session whose job never ran: it was stopped, or something it needed
/// failed.
pub(super) async fn abort_startup(
    state: &OiState,
    session_id: SessionId,
    failure: StartupFailure,
    streams: SessionStreams,
) {
    if let Some(network) = &failure.network {
        let _ = state.container_runtime.remove_network(network).await;
    }
    state.shells.remove(&session_id);
    // spec: SHL#session-end
    state.event_tx.shell_exited(session_id, -1);
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

/// Runs the shell's script and starts the job it attaches to, ending early if
/// a stop is requested.
///
/// A stop is acted on wherever the startup waits, and between the steps that
/// are not interrupted part-way: creating the network and starting the job.
async fn start_app_shell(
    state: &OiState,
    app: App,
    app_name: &AppName,
    shell_name: &ShellName,
    validated_params: serde_json::Map<String, serde_json::Value>,
    stop: &StopSignal,
) -> Result<LiveShell, StartupFailure> {
    let result_slot: Arc<Mutex<Option<ShellOutcome>>> = Arc::new(Mutex::new(None));
    let result_slot_for_task = Arc::clone(&result_slot);
    let db_for_task = state.db.clone();
    let script_limits = state.script_limits.clone();
    let operation_id = OperationId::new();
    let op_id_for_log = operation_id.clone();
    let app_name_for_task = app_name.clone();
    let shell_name_for_task = shell_name.clone();
    let cancel = stop.cancel_token();

    let mut script = tokio::task::spawn_blocking(move || {
        let (engine, mut scope, _) = crate::setup_language(&script_limits, Arc::clone(&app.bundle));
        let compiled = app
            .bundle
            .script()
            .map_err(|e| e.to_string())
            .and_then(|script| {
                engine
                    .compile(script.text())
                    .map_err(|e| script.remap_error(&e.to_string()))
            });
        let ast = match compiled {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(
                    app = %app_name_for_task, shell = %shell_name_for_task,
                    "shell script compile error: {e}"
                );
                return false;
            }
        };
        let db = db_for_task;
        let registry: Arc<dyn crate::runtime::InstanceRegistry> =
            Arc::new(DbInstanceRegistry::new(db.clone()));
        // Shell names follow the same bsl.name rules as action names; reuse
        // ActionName as the identifier written into the action log.
        let shell_as_action = ActionName::new_unchecked(shell_name_for_task.as_str());
        let log = DbActionLog::new(
            db.clone(),
            op_id_for_log.clone(),
            app_name_for_task.clone(),
            shell_as_action,
        );
        let world = Arc::new(DbWorldOracle::new(db.clone()));

        set_shell_attach_ctx(ShellAttachCtx {
            app_name: app_name_for_task.clone(),
            result: Arc::clone(&result_slot_for_task),
        });

        let success = loop {
            let result = run_operation(
                OperationContext {
                    engine: &engine,
                    script_ast: &ast,
                    operation_id: op_id_for_log.clone(),
                    app: &app,
                    action_name: shell_name_for_task.as_str(),
                    log: &log,
                    world: Arc::clone(&world),
                    registry: Arc::clone(&registry),
                    active_progress: None,
                    tick_notify: None,
                    params: validated_params.clone(),
                    // spec: OPS#shell-sessions
                    is_shell: true,
                    // spec: OPS#shell-sessions
                    // Passing None means rt.start_* calls do not persist
                    // dynamic resources to the desired state, so resources
                    // created within a shell session disappear when the
                    // session's replay context is dropped.
                    db: None,
                    source_generation: 0,
                    target_generation: 0,
                    script_limits: None,
                    cipher: None,
                    operation_volume_bindings: std::collections::HashMap::new(),
                    // Shell sessions don't participate in operator-initiated
                    // cancellation; the token fires only when the session is
                    // stopped, which ends the script early.
                    cancel_token: Arc::clone(&cancel),
                    container_signaler: None,
                    volume_writer: None,
                    executor: None,
                },
                &mut scope,
            );
            match result {
                OperationResult::Completed => break true,
                OperationResult::Failed(e) => {
                    tracing::error!(
                        app = %app_name_for_task, shell = %shell_name_for_task,
                        "shell closure failed: {e}"
                    );
                    break false;
                }
                // spec: OPS#cancellation
                // The session was stopped while the script waited.
                OperationResult::Cancelled => break false,
                OperationResult::Suspended(_) => cancel.wait_for(Duration::from_secs(2)),
            }
        };

        clear_shell_attach_ctx();
        success
    });

    // spec: SHL#session-startup
    let script_ok = tokio::select! {
        _ = stop.requested() => {
            // The script's thread cannot be abandoned: it sees the stop at its
            // next wait and exits, and until then it may still be creating
            // things the teardown would miss.
            let _ = (&mut script).await;
            return Err(StartupFailure::stopped(None));
        }
        done = &mut script => done.unwrap_or(false),
    };
    if stop.is_requested() {
        return Err(StartupFailure::stopped(None));
    }
    if !script_ok {
        return Err(StartupFailure::failed("the shell's script failed", None));
    }

    let shell_outcome = result_slot.lock().take();
    let exec_target = match shell_outcome {
        Some(ShellOutcome::Attach(target)) => target,
        Some(ShellOutcome::Error(msg)) => {
            tracing::warn!(app = %app_name, shell = %shell_name, "shell error: {msg}");
            return Err(StartupFailure::failed(msg, None));
        }
        None => {
            tracing::warn!(
                app = %app_name, shell = %shell_name,
                "shell closure completed but attach was not called"
            );
            return Err(StartupFailure::failed("the shell did not attach", None));
        }
    };

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
            _ = stop.requested() => return Err(StartupFailure::stopped(None)),
            ensured = ensure_image(state, app_name, shell_name, &image) => {
                ensured.map_err(|reason| StartupFailure::failed(reason, None))?;
            }
        }
    }

    if stop.is_requested() {
        return Err(StartupFailure::stopped(None));
    }

    let net_name = format!("seedling-{}", instance.display_name);
    let net_prefix = pod_network_prefix(&state.node_prefix, &instance);
    if let Err(e) = state
        .container_runtime
        .create_network(&net_name, net_prefix, None)
        .await
    {
        tracing::warn!(app = %app_name, shell = %shell_name, %net_name, "create network failed: {e}");
        return Err(StartupFailure::failed(
            format!("create network failed: {e}"),
            None,
        ));
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
                    let svc_ip = instance_ipv6(&state.node_prefix, &svc_instance);
                    Ok((sp.port.get(), svc_ip, sp.port.get()))
                })
                .collect();
            match result {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(app = %app_name, shell = %shell_name, "service mount registry lookup failed: {e}");
                    return Err(StartupFailure::failed(
                        format!("service mount lookup failed: {e}"),
                        Some(net_name),
                    ));
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
        0,
    );
    container_spec.health = None;
    container_spec.labels.insert(
        crate::system::types::SESSION_LABEL.to_string(),
        "shell".to_string(),
    );

    // spec: SHL#session-startup
    if stop.is_requested() {
        return Err(StartupFailure::stopped(Some(net_name)));
    }

    match state.container_runtime.exec(container_spec).await {
        Ok(exec) => Ok(LiveShell {
            exec,
            container_name,
            net_name,
        }),
        Err(e) => {
            tracing::warn!(app = %app_name, shell = %shell_name, "exec failed: {e}");
            Err(StartupFailure::failed(
                format!("exec failed: {e}"),
                Some(net_name),
            ))
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

/// Runs a registered session to its end and tears everything it holds down:
/// the job, the session's registry entry, its pod network and its streams.
/// Returns the exit code reported to the client.
///
/// Every way a session can end comes through here, so none can leave the job or
/// its network behind.
pub(super) async fn run_to_exit(
    state: &OiState,
    session_id: SessionId,
    stop: &StopSignal,
    mut live: LiveShell,
    recv: &mut quinn::RecvStream,
    leftover_stdin: &[u8],
    mut streams: SessionStreams,
) -> i32 {
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
            break 'run Err(None);
        }

        if !leftover_stdin.is_empty() && live.exec.stdin.write_all(leftover_stdin).await.is_err() {
            break 'run Err(Some(-1));
        }

        // spec: SHL#session-end
        match relay_session(recv, &mut live.exec, &mut streams.stdout, stop).await {
            Some(status) => Ok(status.ok().and_then(|s| s.code()).unwrap_or(-1)),
            None => Err(None),
        }
    };

    let exit_code = match job_exit {
        Ok(code) => code,
        Err(forced) => {
            // spec: SHL#session-end
            if let Some(pid) = live.exec.child.id() {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }

            let graceful =
                tokio::time::timeout(Duration::from_secs(5), live.exec.child.wait()).await;
            let terminated = match graceful {
                Ok(status) => status.ok().and_then(|s| s.code()).unwrap_or(-1),
                Err(_timeout) => {
                    let _ = state
                        .container_runtime
                        .remove_container(&live.container_name, true)
                        .await;
                    let _ = live.exec.child.kill().await;
                    live.exec
                        .child
                        .wait()
                        .await
                        .ok()
                        .and_then(|s| s.code())
                        .unwrap_or(-1)
                }
            };
            forced.unwrap_or(terminated)
        }
    };

    state.shells.remove(&session_id);
    // spec: SHL#session-end
    state.event_tx.shell_exited(session_id, exit_code);
    let _ = state.container_runtime.remove_network(&live.net_name).await;
    streams.end(exit_code, None).await;
    exit_code
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
