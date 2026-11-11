use std::{path::PathBuf, sync::Arc};

use seedling_protocol::names::{
    AppName, AppVolumeName, HeldVolumeId, SessionId, ShellName, SiteVolumeName,
};

use super::{
    registry::{ShellSession, StopSignal},
    session::{
        LiveShell, SessionStreams, StartupFailure, abort_startup, begin_session, reject,
        run_to_exit,
    },
};
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
enum VolumeShellMount {
    Site { name: SiteVolumeName },
    App { app: AppName, volume: AppVolumeName },
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

// spec: SHL#volume-shells
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
        volumes: Vec<VolumeShellMount>,
        rows: u16,
        cols: u16,
        // spec: SHL#volume-shells — when true, every resolved bind mount is
        // forced read-only inside the shell container, regardless of whether
        // the underlying volume is inherently writable.
        #[serde(default)]
        read_only: bool,
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
            reject(&mut send, $code, $msg).await;
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
            VolumeShellMount::Site { name } => {
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
                        let p = vol_store.site_path(name.as_str());
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
                let display_name = sanitise_name(name.as_str());
                (path, read_only, display_name)
            }

            VolumeShellMount::App { app, volume } => {
                let on_disk_name =
                    crate::runtime::identity::VolumeName::for_app(app.as_str(), volume.as_str());
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

            VolumeShellMount::Held { id } => {
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

    let image = VOLUME_SHELL_IMAGE;
    match state.container_runtime.image_exists(image).await {
        Ok(true) => {}
        Ok(false) => {
            // spec: SHL#volume-shell-image
            // Build the alpine-based shell image on first use. We avoid
            // pulling a generic distro image so operators can rely on a
            // known toolset (editors, rsync, etc.) being available.
            tracing::info!(%image, "building volume shell image");
            let data_dir = state
                .db_path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            if let Err(e) = build_volume_shell_image(&data_dir).await {
                tracing::warn!(%image, "image build failed: {e}");
                err!("shell_error", format!("image build failed: {e}"));
            }
        }
        Err(e) => tracing::warn!(%image, "image_exists check failed: {e}"),
    }

    let net_name = format!("seedling-{container_name}");
    let net_prefix = pod_network_prefix(&state.node_prefix, &instance);
    if let Err(e) = state
        .container_runtime
        .create_network(&net_name, net_prefix, None)
        .await
    {
        tracing::warn!(%net_name, "volume shell: create network failed: {e}");
        err!("shell_error", format!("create network failed: {e}"));
    }

    // Open daemon stdout/stderr uni streams.
    let (stdout_send, stdout_stream_id) = match conn.open_uni().await {
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
    let (stderr_send, stderr_stream_id) = match conn.open_uni().await {
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

    let stop = StopSignal::new();
    let volumes_app = AppName::new_unchecked("_volumes");
    // spec: SHL#volume-shells
    let session = ShellSession::starting(
        session_id,
        volumes_app,
        session_name.clone(),
        req_actor,
        (params.rows, params.cols),
        stop.clone(),
    );
    let event_name = ShellName::new_unchecked("volume-shell");
    if !begin_session(
        &state,
        &mut send,
        session,
        &event_name,
        stdout_stream_id,
        stderr_stream_id,
    )
    .await
    {
        let _ = state.container_runtime.remove_network(&net_name).await;
        return;
    }
    let streams = SessionStreams {
        send,
        stdout: stdout_send,
        stderr: stderr_send,
    };

    // Drop the operator at the most useful directory: the single mount
    // point when they're inspecting one volume, or /mnt when several are
    // mounted side-by-side so they can `ls` to see what's available.
    let workdir = if resolved.len() == 1 {
        Some(resolved[0].target.clone())
    } else {
        Some("/mnt".to_owned())
    };

    let force_ro = params.read_only;
    let mounts: Vec<Mount> = resolved
        .into_iter()
        .map(|r| Mount {
            source: MountSource::Bind(r.source),
            target: r.target,
            // spec: SHL#volume-shells — `force_ro` overrides the per-volume
            // default; the inherent read-only flag still wins for snapshot
            // site volumes when `force_ro` is false.
            read_only: r.read_only || force_ro,
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
            m.insert(
                crate::system::types::SESSION_LABEL.into(),
                "volume-shell".into(),
            );
            m
        },
        health: None,
        hosts: vec![],
        dns_servers: state.dns_servers.clone(),
        memory: None,
        cpus: None,
        // The operator opens a volume shell to inspect or fix files on the
        // mounted volume(s). With seedling's default `--cap-drop=ALL`,
        // root inside the container is still subject to DAC checks, so
        // listing or editing files owned by other UIDs (e.g. postgres's
        // `0700 999:999` data dir) fails. Grant the four caps needed for
        // root to actually behave like root over the filesystem.
        // spec: SHL#volume-shell-privileges
        extra_caps: vec![
            "DAC_OVERRIDE".to_owned(),
            "DAC_READ_SEARCH".to_owned(),
            "CHOWN".to_owned(),
            "FOWNER".to_owned(),
        ],
        writable_rootfs: true,
        pids_limit: 1024,
        workdir,
        user: None,
        stop_signal: None,
        stop_timeout_secs: None,
    };

    // spec: SHL#session-startup
    if stop.is_requested() {
        abort_startup(
            &state,
            session_id,
            StartupFailure::stopped(Some(net_name)),
            streams,
        )
        .await;
        return;
    }

    let exec_handle = match state.container_runtime.exec(spec).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("volume shell: exec failed: {e}");
            let failure = StartupFailure::failed(format!("exec failed: {e}"), Some(net_name));
            abort_startup(&state, session_id, failure, streams).await;
            return;
        }
    };

    let exit_code = run_to_exit(
        &state,
        session_id,
        &stop,
        LiveShell {
            exec: exec_handle,
            container_name,
            net_name,
        },
        &mut recv,
        &leftover_stdin,
        streams,
    )
    .await;

    tracing::info!(%session_name, %exit_code, "volume shell session ended");
}

// spec: SHL#volume-shell-image
/// Container image used for volume-shell sessions. Built locally from
/// [`VOLUME_SHELL_CONTAINERFILE`] on first use; the `localhost/` prefix
/// keeps the image off the registry-allowlist path.
pub const VOLUME_SHELL_IMAGE: &str = "localhost/seedling-volume-shell:latest";

/// Containerfile used to build [`VOLUME_SHELL_IMAGE`]. Alpine + bash and the
/// utilities operators reach for when poking at volume contents: editors
/// (nvim, nano), file transfer (rsync, curl, wget), archive tools (tar,
/// gzip, zstd, xz), and a few ergonomic extras (less, jq, htop, ripgrep,
/// tree, file). The repo also keeps a copy at `Containerfile.volume-shell`
/// for documentation; the source-of-truth for the daemon is this constant.
const VOLUME_SHELL_CONTAINERFILE: &str = "\
FROM docker.io/library/alpine:3.21\n\
RUN apk add --no-cache \
bash \
coreutils \
curl \
file \
grep \
gzip \
htop \
jq \
less \
nano \
neovim \
procps \
ripgrep \
rsync \
tar \
tree \
wget \
xz \
zstd\n";

/// Build the volume-shell image from the embedded Containerfile. Mirrors
/// [`build_caddy_image`](crate::system::caddy) — the Containerfile and an
/// empty build context are dropped into `data_dir` and `podman build` is
/// invoked.
pub async fn build_volume_shell_image(
    data_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let containerfile_path = data_dir.join("Containerfile.volume-shell");
    std::fs::write(&containerfile_path, VOLUME_SHELL_CONTAINERFILE)?;

    let context_dir = data_dir.join("volume-shell-build-ctx");
    std::fs::create_dir_all(&context_dir)?;

    let output = tokio::process::Command::new("podman")
        .args([
            "build",
            "-t",
            VOLUME_SHELL_IMAGE,
            "-f",
            &containerfile_path.to_string_lossy(),
            &context_dir.to_string_lossy(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("podman build exited {}: {}", output.status, stderr.trim()).into());
    }

    tracing::info!("volume shell image built successfully");
    Ok(())
}
