use std::{collections::BTreeMap, net::Ipv6Addr, sync::Arc, time::Duration};

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    defs::resource::ResourceKind,
    oi::state::OiState,
    runtime::{
        AppPhase,
        barrier::{
            OperationId,
            oracle::DbWorldOracle,
            replay::{DbActionLog, OperationContext, OperationResult, run_operation},
            shell::{
                ShellAttachCtx, ShellExecTarget, clear_shell_attach_ctx, set_shell_attach_ctx,
            },
        },
        identity::{InstanceVariant, ResourceInstance},
        registry::DbInstanceRegistry,
        registry::InstanceRegistry,
    },
    system::translate::{
        container::job_spec,
        proxy::{instance_ipv6, pod_network_prefix},
    },
};

use super::registry::ShellSession;

pub(crate) async fn open_shell_session(
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    leftover_stdin: Vec<u8>,
    initial_line: Vec<u8>,
    state: Arc<OiState>,
) {
    #[derive(serde::Deserialize)]
    struct Params {
        app: String,
        name: String,
        rows: u16,
        cols: u16,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(default)]
        params: serde_json::Value,
    }

    let req: Request = match serde_json::from_slice(&initial_line) {
        Ok(r) => r,
        Err(e) => {
            let resp = serde_json::to_vec(&serde_json::json!({
                "error": { "code": "not_found", "message": format!("invalid request: {e}") }
            }))
            .unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }
    };
    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            let resp = serde_json::to_vec(&serde_json::json!({
                "error": { "code": "requirements_invalid", "message": format!("invalid params: {e}") }
            }))
            .unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }
    };
    let app_name = params.app;
    let shell_name = params.name;
    let initial_rows = params.rows;
    let initial_cols = params.cols;

    // All registry access is done in a synchronous closure so no lock guard
    // crosses an await point (parking_lot guards are not Send).
    let lookup: Result<_, (&str, String)> = (|| {
        let reg = state.registry.read();
        let Some(entry) = reg.get(&app_name) else {
            return Err(("not_found", format!("app not found: {app_name}")));
        };
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(("not_installed", format!("app is not installed: {app_name}")));
        }
        {
            let def = entry.app.def.lock();
            if !def.shells.contains_key(&shell_name) {
                return Err(("not_found", format!("shell not found: {shell_name}")));
            }
        }
        Ok((entry.app.clone(), entry.script.clone()))
    })();
    let (app, script) = match lookup {
        Ok(v) => v,
        Err((code, msg)) => {
            let resp = serde_json::to_vec(&serde_json::json!({
                "error": { "code": code, "message": msg }
            }))
            .unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }
    };

    let (mut stdout_send, stdout_stream_id) = match conn.open_uni().await {
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
    let (mut stderr_send, stderr_stream_id) = match conn.open_uni().await {
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

    let session_id = Uuid::new_v4();

    {
        let mut resp = serde_json::to_vec(&serde_json::json!({
            "result": {
                "session_id": session_id.to_string(),
                "stdout_stream_id": stdout_stream_id,
                "stderr_stream_id": stderr_stream_id,
            }
        }))
        .unwrap_or_default();
        resp.push(b'\n');
        if let Err(e) = send.write_all(&resp).await {
            tracing::warn!("write handshake: {e}");
            let _ = send.finish();
            return;
        }
    }

    let result_slot: Arc<Mutex<Option<ShellExecTarget>>> = Arc::new(Mutex::new(None));
    let result_slot_for_task = Arc::clone(&result_slot);
    let db_path = state.db_path.clone();
    let operation_id = OperationId::new();
    let op_id_for_log = operation_id.clone();
    let app_name_for_task = app_name.clone();
    let shell_name_for_task = shell_name.clone();

    let run_result = tokio::task::spawn_blocking(move || {
        let (engine, mut scope, _) = crate::setup_language();
        let ast = match engine.compile(&script) {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(
                    app = %app_name_for_task, shell = %shell_name_for_task,
                    "shell script compile error: {e}"
                );
                return false;
            }
        };
        let action_log_db = match crate::runtime::db::Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(app = %app_name_for_task, "open action-log db for shell: {e}");
                return false;
            }
        };
        let world_db = match crate::runtime::db::Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(app = %app_name_for_task, "open world db for shell: {e}");
                return false;
            }
        };
        let instance_db = match crate::runtime::db::Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(app = %app_name_for_task, "open instance db for shell: {e}");
                return false;
            }
        };
        let registry: Arc<dyn crate::runtime::InstanceRegistry> =
            Arc::new(DbInstanceRegistry::new(instance_db));
        let log = DbActionLog::new(
            action_log_db,
            op_id_for_log.clone(),
            app_name_for_task.clone(),
            shell_name_for_task.clone(),
        );
        let world = Arc::new(DbWorldOracle::new(world_db));

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
                    action_name: &shell_name_for_task,
                    log: &log,
                    world: Arc::clone(&world),
                    registry: Arc::clone(&registry),
                    active_progress: None,
                    tick_notify: None,
                    install_requirements: None,
                    is_shell: true,
                    db: None,
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
                OperationResult::Suspended(_) => {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        };

        clear_shell_attach_ctx();
        success
    })
    .await
    .unwrap_or(false);

    if !run_result {
        let resp = serde_json::to_vec(&serde_json::json!({ "exit_code": -1 })).unwrap_or_default();
        let _ = send.write_all(&resp).await;
        let _ = send.finish();
        return;
    }

    let exec_target_opt = result_slot.lock().take();
    let exec_target = match exec_target_opt {
        Some(t) => t,
        None => {
            tracing::warn!(
                app = %app_name, shell = %shell_name,
                "shell closure completed but attach was not called"
            );
            let resp =
                serde_json::to_vec(&serde_json::json!({ "exit_code": -1 })).unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
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
        match state.container_runtime.image_exists(&image).await {
            Ok(false) => {
                tracing::info!(app = %app_name, shell = %shell_name, %image, "pulling shell image");
                if let Err(e) = state.container_runtime.pull_image(&image).await {
                    tracing::warn!(app = %app_name, shell = %shell_name, %image, "image pull failed: {e}");
                    let resp = serde_json::to_vec(&serde_json::json!({ "exit_code": -1 }))
                        .unwrap_or_default();
                    let _ = send.write_all(&resp).await;
                    let _ = send.finish();
                    return;
                }
            }
            Err(e) => tracing::warn!(app = %app_name, %image, "image_exists check failed: {e}"),
            Ok(true) => {}
        }
    }

    let net_name = format!("seedling-{}", instance.display_name);
    let net_prefix = pod_network_prefix(&state.node_prefix, &instance);
    if let Err(e) = state
        .container_runtime
        .create_network(&net_name, net_prefix, None)
        .await
    {
        tracing::warn!(app = %app_name, shell = %shell_name, %net_name, "create network failed: {e}");
        let resp = serde_json::to_vec(&serde_json::json!({ "exit_code": -1 })).unwrap_or_default();
        let _ = send.write_all(&resp).await;
        let _ = send.finish();
        return;
    }

    let resolved_mounts: Vec<(u16, Ipv6Addr, u16)> = {
        let service_mounts = exec_target.job_def.pod.lock().service_mounts.clone();
        if service_mounts.is_empty() {
            vec![]
        } else {
            match crate::runtime::db::Db::open(&state.db_path) {
                Ok(db) => {
                    let mount_registry = DbInstanceRegistry::new(db);
                    service_mounts
                        .iter()
                        .map(|sp| {
                            let svc_instance = mount_registry.get_or_create_singleton(
                                &exec_target.app_name,
                                ResourceKind::Service,
                                Some(sp.service.name.as_str()),
                            );
                            let svc_ip = instance_ipv6(&state.node_prefix, &svc_instance);
                            (sp.port, svc_ip, sp.port)
                        })
                        .collect()
                }
                Err(e) => {
                    tracing::warn!(app = %app_name, "open db for service mounts: {e}");
                    vec![]
                }
            }
        }
    };

    let mut container_spec = job_spec(
        &exec_target.job_def,
        &instance,
        &BTreeMap::new(),
        &(net_name.clone(), net_prefix),
        &resolved_mounts,
    );
    container_spec.health = None;

    let mut exec_handle = match state.container_runtime.exec(container_spec).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(app = %app_name, shell = %shell_name, "exec failed: {e}");
            let _ = state.container_runtime.remove_network(&net_name).await;
            let resp =
                serde_json::to_vec(&serde_json::json!({ "exit_code": -1 })).unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }
    };

    {
        let ws = libc::winsize {
            ws_row: initial_rows,
            ws_col: initial_cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: pty_master_fd is valid until exec_handle is dropped.
        unsafe { libc::ioctl(exec_handle.pty_master_fd, libc::TIOCSWINSZ as _, &ws) };
    }

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let pty_master_fd = exec_handle.pty_master_fd;
    state.shells.insert(ShellSession {
        session_id,
        app: app_name.clone(),
        name: shell_name.clone(),
        opened_at: jiff::Timestamp::now(),
        pty_master_fd,
        stop_tx,
    });

    let _ = stderr_send.finish();

    let mut stdin_buf = vec![0u8; 4096];
    let exit_code: i32;

    if !leftover_stdin.is_empty() && exec_handle.stdin.write_all(&leftover_stdin).await.is_err() {
        state.shells.remove(&session_id);
        let resp = serde_json::to_vec(&serde_json::json!({ "exit_code": -1 })).unwrap_or_default();
        let _ = send.write_all(&resp).await;
        let _ = send.finish();
        return;
    }

    // i[shell.close]
    // i[shell.exit]
    loop {
        let mut stdout_buf = vec![0u8; 4096];
        tokio::select! {
            n = recv.read(&mut stdin_buf) => {
                match n {
                    Ok(Some(n)) if n > 0 => {
                        if exec_handle.stdin.write_all(&stdin_buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            n = exec_handle.stdout.read(&mut stdout_buf) => {
                match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if stdout_send.write_all(&stdout_buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
            status = exec_handle.child.wait() => {
                exit_code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
                state.shells.remove(&session_id);
                let _ = state.container_runtime.remove_network(&net_name).await;
                let mut exit_frame =
                    serde_json::to_vec(&serde_json::json!({ "exit_code": exit_code }))
                        .unwrap_or_default();
                exit_frame.push(b'\n');
                let _ = send.write_all(&exit_frame).await;
                let _ = send.finish();
                let _ = stdout_send.finish();
                tracing::info!(
                    app = %app_name, shell = %shell_name, %exit_code,
                    "shell session ended"
                );
                return;
            }
            _ = &mut stop_rx => {
                break;
            }
        }
    }

    // i[shell.cleanup]
    if let Some(pid) = exec_handle.child.id() {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }

    let graceful = tokio::time::timeout(Duration::from_secs(5), exec_handle.child.wait()).await;

    exit_code = match graceful {
        Ok(status) => status.ok().and_then(|s| s.code()).unwrap_or(-1),
        Err(_timeout) => {
            let _ = state
                .container_runtime
                .remove_container(&container_name, true)
                .await;
            let _ = exec_handle.child.kill().await;
            exec_handle
                .child
                .wait()
                .await
                .ok()
                .and_then(|s| s.code())
                .unwrap_or(-1)
        }
    };

    state.shells.remove(&session_id);
    let _ = state.container_runtime.remove_network(&net_name).await;

    let mut exit_frame =
        serde_json::to_vec(&serde_json::json!({ "exit_code": exit_code })).unwrap_or_default();
    exit_frame.push(b'\n');
    let _ = send.write_all(&exit_frame).await;
    let _ = send.finish();
    let _ = stdout_send.finish();

    tracing::info!(app = %app_name, shell = %shell_name, %exit_code, "shell session ended");
}
