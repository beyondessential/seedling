use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use ipnet::Ipv6Net;
use rusqlite::OptionalExtension;
use snafu::{ResultExt, Snafu};

use super::config::build_caddy_config;
use super::proxy::{CaddyAddrs, CaddyProxy};
use crate::system::{
    ContainerRuntime, NetworkProxy, ProcessManager,
    types::{ContainerStatus, ProxyConfig, TransientRestart, TransientUnitSpec},
};

pub(crate) const CADDY_BLUE: &str = "seedling-caddy-blue";
pub(crate) const CADDY_GREEN: &str = "seedling-caddy-green";
pub(crate) const CADDY_IMAGE: &str = "localhost/seedling-caddy:latest";
pub(crate) const CADDY_DATA_VOLUME: &str = "seedling-caddy-data";
pub(crate) const PROXY_NETWORK: &str = "seedling-proxy";

/// Caddy JSON config that binds the admin API on the per-slot Unix socket.
const CADDY_ADMIN_JSON: &str = r#"{"admin":{"listen":"unix//run/caddy-admin/admin.sock"}}"#;

/// Returns the host-side directory that is bind-mounted into the container
/// as `/run/caddy-admin/`. The socket file (`admin.sock`) is created inside
/// this directory by the Caddy process.
fn socket_run_dir(data_dir: &Path, container_name: &str) -> PathBuf {
    let slot = if container_name.ends_with("-blue") {
        "blue"
    } else {
        "green"
    };
    data_dir.join("caddy-run").join(slot)
}

/// Returns the host-side path of the admin Unix socket for `container_name`.
pub(crate) fn admin_socket_path(data_dir: &Path, container_name: &str) -> PathBuf {
    socket_run_dir(data_dir, container_name).join("admin.sock")
}

const CADDY_CONTAINERFILE: &str = "\
FROM docker.io/library/caddy:2.11.2-builder AS builder\n\
RUN xcaddy build --with github.com/mholt/caddy-l4\n\
FROM docker.io/library/caddy:2.11.2\n\
COPY --from=builder /usr/bin/caddy /usr/bin/caddy\n";

// r[impl infra.proxy.startup]
/// Returns the /64 infrastructure prefix for the seedling-proxy network.
///
/// The network sits at `fd5e:edXX:XXXX:ff00::/64` within the node's /48,
/// using `0xff` as the subnet discriminant (above the resource-kind range 0–9).
pub(crate) fn proxy_network_prefix(node_prefix: &Ipv6Net) -> Ipv6Net {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xff;
    Ipv6Net::new(std::net::Ipv6Addr::from(addr), 64).expect("64 is a valid IPv6 prefix length")
}

/// Fixed IPv4 /24 subnet for the seedling-proxy network, enabling
/// dual-stack ingress without IPv4 on pod networks.
pub(crate) fn proxy_ipv4_subnet() -> ipnet::Ipv4Net {
    "10.89.255.0/24".parse().expect("valid IPv4 subnet")
}

#[derive(Debug, Snafu)]
pub(crate) enum CaddyStartupError {
    #[snafu(display("container runtime error: {source}"))]
    Container {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("process manager error: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("I/O error writing admin config: {source}"))]
    Io {
        source: std::io::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("Caddy did not become healthy within the timeout"))]
    Timeout { backtrace: snafu::Backtrace },
    #[snafu(display("database error: {source}"))]
    Db {
        source: rusqlite::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image ID unavailable for {reference}"))]
    ImageId {
        reference: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image build failed: {message}"))]
    Build {
        message: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("failed to build caddy admin HTTP client: {source}"))]
    ClientBuild {
        source: reqwest::Error,
        backtrace: snafu::Backtrace,
    },
    // r[impl infra.proxy.upgrade.rollback]
    #[snafu(display("upgraded Caddy rejected the replayed proxy config: {source}"))]
    ConfigRejected {
        source: super::proxy::CaddyError,
        backtrace: snafu::Backtrace,
    },
}

/// Build the custom Caddy image from the embedded Containerfile.
async fn build_caddy_image(data_dir: &std::path::Path) -> Result<(), CaddyStartupError> {
    let containerfile_path = data_dir.join("Containerfile.caddy");
    std::fs::write(&containerfile_path, CADDY_CONTAINERFILE).context(IoSnafu)?;

    // podman build needs a context directory; the Containerfile has no
    // local COPY instructions so an empty temp dir suffices.
    let context_dir = data_dir.join("caddy-build-ctx");
    std::fs::create_dir_all(&context_dir).context(IoSnafu)?;

    tracing::info!("building custom Caddy image (this may take a moment)");

    let output = tokio::process::Command::new("podman")
        .args([
            "build",
            // Run the build's RUN steps in the host network namespace so we can
            // be fairly sure egress will work even if default podman net is broken.
            "--network=host",
            "-t",
            CADDY_IMAGE,
            "-f",
            &containerfile_path.to_string_lossy(),
            &context_dir.to_string_lossy(),
        ])
        .output()
        .await
        .context(IoSnafu)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return BuildSnafu {
            message: format!("podman build exited {}: {}", output.status, stderr.trim()),
        }
        .fail();
    }

    tracing::info!("custom Caddy image built successfully");
    Ok(())
}

fn caddy_db_open(data_dir: &std::path::Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    let conn = rusqlite::Connection::open(data_dir.join("seedling.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    Ok(conn)
}

fn read_active_container(conn: &rusqlite::Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT active_container FROM caddy_state WHERE singleton = 1",
        [],
        |r| r.get(0),
    )
    .optional()
}

fn write_active_container(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO caddy_state (singleton, active_container)
         VALUES (1, ?1)
         ON CONFLICT(singleton) DO UPDATE SET active_container = excluded.active_container",
        rusqlite::params![name],
    )?;
    Ok(())
}

// r[impl infra.proxy.upgrade.cache]
/// Read the cached proxy config, if any. The cache stores the
/// Seedling-internal `ProxyConfig` rather than the post-build Caddy JSON,
/// so the replay path can rebuild the JSON in whatever format the current
/// daemon emits — avoiding format drift between the cached value and a
/// freshly-upgraded Caddy.
pub(crate) fn read_cached_proxy_config(
    data_dir: &std::path::Path,
) -> Result<Option<ProxyConfig>, rusqlite::Error> {
    let conn = caddy_db_open(data_dir)?;
    let json_str: Option<String> = conn
        .query_row(
            "SELECT config_json FROM caddy_proxy_config WHERE singleton = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json_str.and_then(|s| serde_json::from_str(&s).ok()))
}

// r[impl infra.proxy.upgrade.cache]
pub(crate) fn write_cached_proxy_config(
    data_dir: &std::path::Path,
    config: &ProxyConfig,
) -> Result<(), rusqlite::Error> {
    let conn = caddy_db_open(data_dir)?;
    let json_str = serde_json::to_string(config).unwrap_or_default();
    conn.execute(
        "INSERT INTO caddy_proxy_config (singleton, config_json)
         VALUES (1, ?1)
         ON CONFLICT(singleton) DO UPDATE SET config_json = excluded.config_json",
        rusqlite::params![json_str],
    )?;
    Ok(())
}

/// Returns the name of the other caddy slot.
fn other_slot(active: &str) -> &'static str {
    if active == CADDY_BLUE {
        CADDY_GREEN
    } else {
        CADDY_BLUE
    }
}

/// Returns the systemd unit name for a caddy container slot.
fn slot_unit(container: &str) -> String {
    format!("{container}.service")
}

/// Start a Caddy container in the given slot (container name) as a transient
/// systemd unit, and return once the unit has been successfully started.
/// Does not wait for health — the caller polls separately.
#[tracing::instrument(skip_all, fields(%container_name))]
async fn start_slot(
    container_name: &str,
    _container: &dyn ContainerRuntime,
    process: &dyn ProcessManager,
    data_dir: &Path,
) -> Result<(), CaddyStartupError> {
    let unit_name = &slot_unit(container_name);

    // StartTransientUnit fails with UnitExists if the unit is still loaded in
    // systemd's memory. Two cases:
    // - transient units linger briefly after reaching inactive (GC delay)
    // - unit hit its start rate limit and is stuck in failed/start-limit-hit;
    //   reset_failed_unit clears that so it can be unloaded.
    if process.unit_state(unit_name).await.ok().flatten().is_some() {
        let _ = process.reset_failed_unit(unit_name).await;
        let _ = process.stop_unit(unit_name).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            if process.unit_state(unit_name).await.ok().flatten().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let admin_config_path = data_dir.join("caddy-admin.json");
    let admin_config_str = admin_config_path.to_string_lossy().into_owned();

    // Create the per-slot socket directory on the host so podman can
    // bind-mount it into the container as /run/caddy-admin/.
    let run_dir = socket_run_dir(data_dir, container_name);
    std::fs::create_dir_all(&run_dir).context(IoSnafu)?;
    let run_dir_str = run_dir.to_string_lossy().into_owned();

    process
        .start_transient(TransientUnitSpec {
            name: unit_name.clone(),
            description: "seedling Caddy proxy".to_owned(),
            exec_start: vec![
                "podman".to_owned(),
                "run".to_owned(),
                "--rm".to_owned(),
                "--cap-drop=ALL".to_owned(),
                "--cap-add=NET_BIND_SERVICE".to_owned(),
                "--security-opt".to_owned(),
                "no-new-privileges".to_owned(),
                "--read-only".to_owned(),
                "--tmpfs".to_owned(),
                "/tmp".to_owned(),
                "--tmpfs".to_owned(),
                "/config".to_owned(),
                "--pids-limit".to_owned(),
                "64".to_owned(),
                "--ulimit".to_owned(),
                "nofile=65536:65536".to_owned(),
                "--name".to_owned(),
                container_name.to_owned(),
                "--network".to_owned(),
                PROXY_NETWORK.to_owned(),
                "--volume".to_owned(),
                format!("{CADDY_DATA_VOLUME}:/data"),
                "--volume".to_owned(),
                format!("{admin_config_str}:/etc/caddy/admin.json:ro"),
                "--volume".to_owned(),
                format!("{run_dir_str}:/run/caddy-admin"),
                CADDY_IMAGE.to_owned(),
                "caddy".to_owned(),
                "run".to_owned(),
                "--config".to_owned(),
                "/etc/caddy/admin.json".to_owned(),
            ],
            restart: TransientRestart::Always,
            log_extra_fields: vec![("SEEDLING_INFRA".to_owned(), "proxy".to_owned())],
            kill_signal: None,
            timeout_stop_secs: None,
            restart_sec: Some(5),
            start_limit_interval_sec: Some(600),
            start_limit_burst: Some(10),
        })
        .await
        .context(ProcessSnafu)
}

/// Stop and remove a Caddy container slot. Errors are ignored — the caller
/// is doing cleanup and should not fail if the unit or container is already gone.
#[tracing::instrument(skip_all, fields(%container_name))]
async fn stop_slot(
    container_name: &str,
    process: &dyn ProcessManager,
    container: &dyn ContainerRuntime,
) {
    let unit = slot_unit(container_name);
    let _ = process.stop_unit(&unit).await;
    let _ = container.remove_container(container_name, true).await;
}

/// Tear down all Caddy infrastructure: stop both blue/green slots and remove
/// the proxy network. Called when no apps are installed so the system is fully
/// clean. The data volume is intentionally kept — it holds ACME certificates
/// that are expensive to re-obtain.
#[tracing::instrument(skip_all)]
pub(crate) async fn teardown_caddy(container: &dyn ContainerRuntime, process: &dyn ProcessManager) {
    for slot in [CADDY_BLUE, CADDY_GREEN] {
        if container.inspect(slot).await.ok().flatten().is_some() {
            tracing::info!(container = slot, "tearing down caddy slot");
            stop_slot(slot, process, container).await;
        }
    }

    if container
        .network_exists(PROXY_NETWORK)
        .await
        .unwrap_or(false)
    {
        let _ = container.remove_network(PROXY_NETWORK).await;
    }
}

/// Poll `container_name` until it is running and Caddy's admin API responds
/// on the Unix socket, or until the deadline elapses. Returns `CaddyAddrs`
/// on success.
#[tracing::instrument(skip_all, fields(%container_name))]
async fn poll_until_healthy(
    container_name: &str,
    container: &dyn ContainerRuntime,
    deadline: tokio::time::Instant,
    socket_path: &Path,
) -> Result<CaddyAddrs, CaddyStartupError> {
    let proxy = CaddyProxy::new(socket_path).context(ClientBuildSnafu)?;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return TimeoutSnafu.fail();
        }
        if let Ok(Some(state)) = container.inspect(container_name).await
            && matches!(state.status, ContainerStatus::Running)
            && let Some(ip) = state.pod_addr
            && proxy.is_healthy().await.unwrap_or(false)
        {
            return Ok(CaddyAddrs {
                v6: ip,
                v4: state.pod_addr_v4,
                admin_socket: socket_path.to_owned(),
            });
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// r[impl infra.proxy.startup]
/// Ensure the Caddy proxy container is running and healthy.
///
/// Implements a blue/green upgrade strategy: if the active container is running
/// but uses a different image digest than the locally-available image, a new
/// container is started in the other slot, the cached proxy config is applied
/// to it, and the database is updated to record the new active slot.
///
/// 1. Creates the `seedling-proxy` network if absent.
/// 2. Writes a minimal admin-API config to `{data_dir}/caddy-admin.json`.
/// 3. Creates the `seedling-caddy-data` volume if absent.
/// 4. Reads the active slot from the database (defaults to `seedling-caddy-blue`).
/// 5. Cleans up any stale container in the non-active slot.
/// 6. Ensures the Caddy image is present locally.
/// 7. If the active container is running with the correct image and is healthy,
///    returns its admin `SocketAddr` immediately.
/// 8. If the active container is running but with a different image, starts the
///    new image in the other slot and performs a blue/green handoff.
/// 9. Otherwise, force-removes any stale container and starts fresh.
#[tracing::instrument(skip_all, level = "debug")]
pub(crate) async fn ensure_caddy_running(
    container: &dyn ContainerRuntime,
    process: &dyn ProcessManager,
    node_prefix: &Ipv6Net,
    data_dir: &Path,
) -> Result<CaddyAddrs, CaddyStartupError> {
    // 1. Ensure the proxy network exists.
    let proxy_prefix = proxy_network_prefix(node_prefix);
    if !container
        .network_exists(PROXY_NETWORK)
        .await
        .context(ContainerSnafu)?
    {
        let _ = container
            .create_network(PROXY_NETWORK, proxy_prefix, Some(proxy_ipv4_subnet()))
            .await
            .context(ContainerSnafu)?;
    }

    // 2. Write admin config so Caddy binds the admin API on all interfaces.
    let admin_config_path = data_dir.join("caddy-admin.json");
    std::fs::write(&admin_config_path, CADDY_ADMIN_JSON).context(IoSnafu)?;

    // 3. Ensure the data volume exists.
    if !container
        .volume_exists(CADDY_DATA_VOLUME)
        .await
        .context(ContainerSnafu)?
    {
        container
            .create_volume(CADDY_DATA_VOLUME, false)
            .await
            .context(ContainerSnafu)?;
    }

    // 4. Read active container name from DB (default to blue slot).
    //    Normalize legacy names ("seedling-caddy", "seedling-caddy-next")
    //    to the blue/green scheme so upgrades from older installations
    //    converge cleanly.
    let active = {
        let conn = caddy_db_open(data_dir).context(DbSnafu)?;
        let raw = read_active_container(&conn)
            .context(DbSnafu)?
            .unwrap_or_else(|| CADDY_BLUE.to_owned());
        if raw != CADDY_BLUE && raw != CADDY_GREEN {
            tracing::info!(
                old = %raw,
                new = CADDY_BLUE,
                "migrating legacy caddy slot name to blue/green"
            );
            // Stop the legacy container so it doesn't linger.
            if container.inspect(&raw).await.ok().flatten().is_some() {
                stop_slot(&raw, process, container).await;
            }
            // Persist the normalized name.
            let conn = caddy_db_open(data_dir).context(DbSnafu)?;
            write_active_container(&conn, CADDY_BLUE).context(DbSnafu)?;
            CADDY_BLUE.to_owned()
        } else {
            raw
        }
    };

    // 5. Determine the other slot.
    let other = other_slot(&active);

    // 6. Clean up stale other-slot container (from a previously completed upgrade).
    if container.inspect(other).await.ok().flatten().is_some() {
        stop_slot(other, process, container).await;
    }

    // 7. Ensure the image is present; build from embedded Containerfile if missing.
    if !container
        .image_exists(CADDY_IMAGE)
        .await
        .context(ContainerSnafu)?
    {
        build_caddy_image(data_dir).await?;
    }

    // 8. Get the desired image ID.
    let desired_id = container
        .local_image_id(CADDY_IMAGE)
        .await
        .context(ContainerSnafu)?
        .ok_or_else(|| {
            ImageIdSnafu {
                reference: CADDY_IMAGE.to_owned(),
            }
            .build()
        })?;

    // 9. Inspect the active container.
    let active_state = container.inspect(&active).await.context(ContainerSnafu)?;

    match active_state {
        Some(state) if matches!(state.status, ContainerStatus::Running) => {
            if state.image_id.as_deref() == Some(&desired_id) {
                // Correct image — check if already healthy.
                let socket_path = admin_socket_path(data_dir, &active);
                if let Some(ip) = state.pod_addr {
                    let healthy = match CaddyProxy::new(&socket_path) {
                        Ok(p) => p.is_healthy().await.unwrap_or(false),
                        Err(_) => false,
                    };
                    if healthy {
                        return Ok(CaddyAddrs {
                            v6: ip,
                            v4: state.pod_addr_v4,
                            admin_socket: socket_path,
                        });
                    }
                    tracing::warn!(
                        container = %active,
                        "caddy running with correct image but health check failed; restarting"
                    );
                } else {
                    tracing::warn!(
                        container = %active,
                        "caddy running with correct image but has no pod IPv6 address; restarting"
                    );
                }
                // Not healthy — stop and fall through to fresh start.
                stop_slot(&active, process, container).await;
            } else {
                tracing::info!(
                    container = %active,
                    running_image = ?state.image_id,
                    desired_image = %desired_id,
                    "caddy image mismatch; performing blue/green upgrade"
                );
                // r[impl infra.proxy.upgrade]
                start_slot(other, container, process, data_dir).await?;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                let other_socket = admin_socket_path(data_dir, other);
                let new_addrs =
                    poll_until_healthy(other, container, deadline, &other_socket).await?;

                // r[impl infra.proxy.upgrade.cache]
                // Replay the cached proxy config against the new container,
                // rebuilt fresh via build_caddy_config so the JSON we POST
                // is in whatever format the current daemon emits — not
                // whatever was cached against the previous Caddy version.
                if let Some(cached) = read_cached_proxy_config(data_dir).context(DbSnafu)? {
                    let caddy_json = build_caddy_config(&cached);
                    let proxy =
                        CaddyProxy::new(&new_addrs.admin_socket).context(ClientBuildSnafu)?;
                    // r[impl infra.proxy.upgrade.rollback]
                    if let Err(e) = proxy.apply_raw_json(&caddy_json).await {
                        tracing::warn!(
                            error = %e,
                            "upgraded Caddy rejected replayed config; rolling back"
                        );
                        stop_slot(other, process, container).await;
                        return Err(e).context(ConfigRejectedSnafu);
                    }
                }

                // Record the new active slot.
                let conn = caddy_db_open(data_dir).context(DbSnafu)?;
                write_active_container(&conn, other).context(DbSnafu)?;

                return Ok(new_addrs);
            }
        }
        Some(state) => {
            tracing::warn!(
                container = %active,
                status = ?state.status,
                "caddy container exists but is not running; removing and restarting"
            );
            let _ = container.remove_container(&active, true).await;
        }
        None => {
            tracing::info!(container = %active, "caddy container not found; starting fresh");
        }
    }

    // 10. Fresh start.
    start_slot(&active, container, process, data_dir).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let active_socket = admin_socket_path(data_dir, &active);
    let addrs = poll_until_healthy(&active, container, deadline, &active_socket).await?;

    // r[impl infra.proxy.upgrade.cache]
    // Replay the cached proxy config, rebuilt fresh via build_caddy_config
    // so the POSTed JSON is in the format the running daemon emits. A
    // rejection here is non-fatal — the next reconciler tick will push the
    // current config — but we still warn so it is visible in logs.
    if let Ok(Some(cached)) = read_cached_proxy_config(data_dir)
        && let Ok(proxy) = CaddyProxy::new(&addrs.admin_socket)
    {
        let caddy_json = build_caddy_config(&cached);
        if let Err(e) = proxy.apply_raw_json(&caddy_json).await {
            tracing::warn!(
                error = %e,
                "fresh Caddy rejected replayed config; reconciler will push current state"
            );
        }
    }

    let conn = caddy_db_open(data_dir).context(DbSnafu)?;
    write_active_container(&conn, &active).context(DbSnafu)?;

    Ok(addrs)
}
