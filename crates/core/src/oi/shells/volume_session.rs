use std::{path::PathBuf, sync::Arc, time::Duration};

use seedling_protocol::names::{AppName, HeldVolumeId, SessionId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::registry::ShellSession;
use crate::{
    defs::resource::ResourceKind,
    oi::state::OiState,
    runtime::identity::{InstanceId, InstanceVariant, ResourceInstance},
    system::{
        translate::proxy::pod_network_prefix,
        types::{ContainerSpec, Mount, MountSource},
    },
};

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum VolumeRef {
    Site { name: String },
    App { app: AppName, volume: String },
    // Inspect a held volume's data. The `id` is the one returned by
    // /volumes/held/list. Mounted read-write so the operator can cherry-
    // pick files back out into another volume if they want to recover a
    // subset before deleting the rest.
    Held { id: HeldVolumeId },
}

struct ResolvedMount {
    source: PathBuf,
    target: String,
    read_only: bool,
    display_name: String,
}

fn sanitise_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches(|c: char| c == '.' || c == '-')
        .to_owned()
}

// i[impl volumes.shell]
pub(crate) async fn open_volume_shell_session(
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    leftover_stdin: Vec<u8>,
    initial_line: Vec<u8>,
    state: Arc<OiState>,
) {
    #[derive(serde::Deserialize)]
    struct Params {
        volumes: Vec<VolumeRef>,
        rows: u16,
        cols: u16,
    }
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(default)]
        actor: Option<seedling_protocol::actor::Actor>,
        #[serde(default)]
        params: serde_json::Value,
    }

    macro_rules! err {
        ($code:expr, $msg:expr) => {{
            let resp = serde_json::to_vec(&serde_json::json!({
                "error": { "code": $code, "message": $msg }
            }))
            .unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }};
    }
    macro_rules! fail {
        ($code:expr) => {{
            let resp = serde_json::to_vec(&serde_json::json!({ "exit_code": $code }))
                .unwrap_or_default();
            let _ = send.write_all(&resp).await;
            let _ = send.finish();
            return;
        }};
    }

    let req: Request = match serde_json::from_slice(&initial_line) {
        Ok(r) => r,
        Err(e) => err!("not_found", format!("invalid request: {e}")),
    };
    let req_actor = req.actor;
    let params: Params = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => err!("requirements_invalid", format!("invalid params: {e}")),
    };

    if params.volumes.is_empty() {
        err!(
            "requirements_invalid",
            "at least one volume must be specified"
        );
    }

    // Resolve each volume reference to a host path.
    let vol_store = &state.driver.volume_store;
    let mut resolved: Vec<ResolvedMount> = Vec::with_capacity(params.volumes.len());

    for vol_ref in &params.volumes {
        let (path, read_only, display_name) = match vol_ref {
            VolumeRef::Site { name } => {
                let name_owned = name.clone();
                let def = match tokio::task::block_in_place(|| {
                    state
                        .db
                        .call(move |db| crate::runtime::site_volumes::get(db, &name_owned))
                }) {
                    Ok(Some(d)) => d,
                    Ok(None) => err!("not_found", format!("site volume not found: {name}")),
                    Err(e) => err!("internal_error", format!("db error: {e}")),
                };

                use crate::runtime::site_volumes::SiteVolumeKind;
                let read_only = def.is_read_only();
                let path = match &def.kind {
                    SiteVolumeKind::Managed | SiteVolumeKind::Snapshot { .. } => {
                        let p = vol_store.site_path(name);
                        if !p.exists() {
                            err!(
                                "not_found",
                                format!("site volume storage not found: {name}")
                            );
                        }
                        p
                    }
                    SiteVolumeKind::Bind { host_path } => {
                        let p = PathBuf::from(host_path);
                        if !p.exists() {
                            err!(
                                "not_found",
                                format!("bind volume host path not found: {host_path}")
                            );
                        }
                        p
                    }
                };
                let display_name = sanitise_name(name);
                (path, read_only, display_name)
            }

            VolumeRef::App { app, volume } => {
                let on_disk_name =
                    crate::runtime::identity::VolumeName::for_app(app.as_str(), volume);
                let path = vol_store.path(&on_disk_name);
                if !path.exists() {
                    err!(
                        "not_found",
                        format!("app volume storage not found: {app}/{volume}")
                    );
                }
                let display_name = sanitise_name(&format!("{app}.{volume}"));
                (path, false, display_name)
            }

            VolumeRef::Held { id } => {
                let path = match vol_store.held_path(id) {
                    Some(p) => p,
                    None => err!("not_found", format!("held volume not found: {id}")),
                };
                // Look up the meta so the mount point carries a human
                // name instead of a raw UUID.
                let held_list = match vol_store.list_held() {
                    Ok(v) => v,
                    Err(e) => err!("internal_error", format!("list held: {e}")),
                };
                let meta = held_list.iter().find(|m| m.id == *id);
                let label = meta
                    .map(|m| format!("held-{}-{}", m.app, m.volume_name))
                    .unwrap_or_else(|| format!("held-{id}"));
                let display_name = sanitise_name(&label);
                (path, false, display_name)
            }
        };

        resolved.push(ResolvedMount {
            source: path,
            target: format!("/mnt/{display_name}"),
            read_only,
            display_name,
        });
    }

    let session_name = resolved
        .iter()
        .map(|r| r.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    // Build instance identity for network naming.
    let instance_id = InstanceId::generate();
    let instance = ResourceInstance {
        id: instance_id,
        app: AppName::new_unchecked("_volumes"),
        kind: ResourceKind::Job,
        name: Some("volume-shell".into()),
        variant: InstanceVariant::Scaled,
        display_name: format!("volumes-shell-{}", instance_id.display_suffix()),
    };
    let container_name = instance.display_name.clone();

    let image = "ubuntu:latest";
    match state.container_runtime.image_exists(image).await {
        Ok(false) => {
            tracing::info!(%image, "pulling volume shell image");
            if let Err(e) = state.container_runtime.pull_image(image).await {
                tracing::warn!(%image, "image pull failed: {e}");
                fail!(-1);
            }
        }
        Err(e) => tracing::warn!(%image, "image_exists check failed: {e}"),
        Ok(true) => {}
    }

    let net_name = format!("seedling-{container_name}");
    let net_prefix = pod_network_prefix(&state.node_prefix, &instance);
    if let Err(e) = state
        .container_runtime
        .create_network(&net_name, net_prefix, None)
        .await
    {
        tracing::warn!(%net_name, "volume shell: create network failed: {e}");
        fail!(-1);
    }

    // Open daemon stdout/stderr uni streams.
    let (mut stdout_send, stdout_stream_id) = match conn.open_uni().await {
        Ok(s) => {
            let id = s.id().index();
            (s, id)
        }
        Err(e) => {
            tracing::warn!("volume shell: open stdout uni: {e}");
            let _ = send.finish();
            let _ = state.container_runtime.remove_network(&net_name).await;
            return;
        }
    };
    let (mut stderr_send, stderr_stream_id) = match conn.open_uni().await {
        Ok(s) => {
            let id = s.id().index();
            (s, id)
        }
        Err(e) => {
            tracing::warn!("volume shell: open stderr uni: {e}");
            let _ = send.finish();
            let _ = state.container_runtime.remove_network(&net_name).await;
            return;
        }
    };

    let session_id = SessionId::generate();

    // Write handshake.
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
            tracing::warn!("volume shell: write handshake: {e}");
            let _ = send.finish();
            let _ = state.container_runtime.remove_network(&net_name).await;
            return;
        }
    }

    // Drop the operator at the most useful directory: the single mount
    // point when they're inspecting one volume, or /mnt when several are
    // mounted side-by-side so they can `ls` to see what's available.
    let workdir = if resolved.len() == 1 {
        Some(resolved[0].target.clone())
    } else {
        Some("/mnt".to_owned())
    };

    let mounts: Vec<Mount> = resolved
        .into_iter()
        .map(|r| Mount {
            source: MountSource::Bind(r.source),
            target: r.target,
            read_only: r.read_only,
        })
        .collect();

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: image.to_owned(),
        command: vec!["/bin/bash".into()],
        entrypoint: vec![],
        env: vec![],
        mounts,
        network: net_name.clone(),
        labels: {
            let mut m = std::collections::BTreeMap::new();
            m.insert("seedling.session".into(), "volume-shell".into());
            m
        },
        health: None,
        hosts: vec![],
        dns_servers: state.dns_servers.clone(),
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: true,
        pids_limit: 1024,
        workdir,
    };

    let mut exec_handle = match state.container_runtime.exec(spec).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("volume shell: exec failed: {e}");
            let _ = state.container_runtime.remove_network(&net_name).await;
            fail!(-1);
        }
    };

    {
        let ws = libc::winsize {
            ws_row: params.rows,
            ws_col: params.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: pty_master_fd is valid until exec_handle is dropped.
        unsafe { libc::ioctl(exec_handle.pty_master_fd, libc::TIOCSWINSZ as _, &ws) };
    }

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let pty_master_fd = exec_handle.pty_master_fd;
    let volumes_app = AppName::new_unchecked("_volumes");
    state.shells.insert(ShellSession {
        session_id,
        app: volumes_app.clone(),
        name: session_name.clone(),
        opened_at: jiff::Timestamp::now(),
        actor: req_actor,
        container_name: container_name.clone(),
        pty_master_fd,
        stop_tx,
    });
    // i[impl volumes.shell]
    state
        .event_tx
        .shell_started(session_id, &volumes_app, &session_name);

    let _ = stderr_send.finish();

    let mut stdin_buf = vec![0u8; 4096];
    let exit_code: i32;

    if !leftover_stdin.is_empty() && exec_handle.stdin.write_all(&leftover_stdin).await.is_err() {
        state.shells.remove(&session_id);
        state.event_tx.shell_exited(session_id, -1);
        let _ = state.container_runtime.remove_network(&net_name).await;
        fail!(-1);
    }

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
                state.event_tx.shell_exited(session_id, exit_code);
                let _ = state.container_runtime.remove_network(&net_name).await;
                let mut exit_frame =
                    serde_json::to_vec(&serde_json::json!({ "exit_code": exit_code }))
                        .unwrap_or_default();
                exit_frame.push(b'\n');
                let _ = send.write_all(&exit_frame).await;
                let _ = send.finish();
                let _ = stdout_send.finish();
                tracing::info!(
                    %session_name, %exit_code,
                    "volume shell session ended"
                );
                return;
            }
            _ = &mut stop_rx => {
                break;
            }
        }
    }

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
    state.event_tx.shell_exited(session_id, exit_code);
    let _ = state.container_runtime.remove_network(&net_name).await;

    let mut exit_frame =
        serde_json::to_vec(&serde_json::json!({ "exit_code": exit_code })).unwrap_or_default();
    exit_frame.push(b'\n');
    let _ = send.write_all(&exit_frame).await;
    let _ = send.finish();
    let _ = stdout_send.finish();

    tracing::info!(%session_name, %exit_code, "volume shell session ended");
}
