use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use ipnet::Ipv6Net;
use reqwest::Client;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use snafu::Snafu;
use tokio::sync::RwLock;

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime, NetworkProxy, ProcessManager,
    types::{
        ContainerStatus, L4Proto, ProxyConfig, ProxyListenerProto, TransientRestart,
        TransientUnitSpec, VirtualHost,
    },
};

// ---------------------------------------------------------------------------
// CaddyAddrs — returned by ensure_caddy_running
// ---------------------------------------------------------------------------

/// Addresses at which Caddy's admin API is reachable.
pub(crate) struct CaddyAddrs {
    pub v6: SocketAddr,
    pub v4: Option<SocketAddr>,
}

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum CaddyError {
    #[snafu(display("Caddy admin API returned HTTP {status}: {body}"))]
    Api { status: u16, body: String },
    #[snafu(display("HTTP request to Caddy admin API failed: {source}"))]
    Http { source: reqwest::Error },
}

// ---------------------------------------------------------------------------
// CaddyProxy
// ---------------------------------------------------------------------------

/// `NetworkProxy` implementation that drives Caddy via its JSON admin API
/// (`POST /config/`).
///
/// Caddy is managed out of band as infrastructure: it is not tracked in
/// `resource_instances` and does not go through the normal `Actuator`
/// start/stop path. Seedling starts it at startup and manages it directly.
///
/// The admin API is accessed at `http://[<caddy-ip>]:2019` on the
/// `seedling-proxy` network. The current admin address is stored in an
/// `Arc<tokio::sync::RwLock<SocketAddr>>` so it can be updated atomically
/// during a blue/green Caddy upgrade without restarting `CaddyProxy`.
pub(crate) struct CaddyProxy {
    admin_addr: Arc<RwLock<SocketAddr>>,
    client: Client,
}

impl CaddyProxy {
    /// Create a `CaddyProxy` pointed at the given Caddy admin API address.
    pub(crate) fn new(admin_addr: SocketAddr) -> Self {
        Self {
            admin_addr: Arc::new(RwLock::new(admin_addr)),
            client: Client::new(),
        }
    }

    /// Returns a handle to the shared admin address, so the caller can swap
    /// it atomically during a blue/green Caddy upgrade.
    pub(crate) fn admin_addr_handle(&self) -> Arc<RwLock<SocketAddr>> {
        Arc::clone(&self.admin_addr)
    }

    async fn admin_url(&self, path: &str) -> String {
        let addr = *self.admin_addr.read().await;
        // SocketAddr formats IPv6 addresses with brackets: [fd5e:ed...]:2019
        format!("http://{}{}", addr, path)
    }
}

impl CaddyProxy {
    async fn is_healthy_impl(&self) -> Result<bool, CaddyError> {
        let url = self.admin_url("/config/").await;
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn apply_config_impl(&self, config: &ProxyConfig) -> Result<(), CaddyError> {
        let caddy_json = build_caddy_config(config);
        let url = self.admin_url("/config/").await;

        let resp = self
            .client
            .post(&url)
            .json(&caddy_json)
            .send()
            .await
            .map_err(|e| CaddyError::Http { source: e })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(CaddyError::Api { status, body });
        }

        Ok(())
    }

    pub(crate) async fn apply_raw_json(&self, json: &serde_json::Value) -> Result<(), CaddyError> {
        let url = self.admin_url("/config/").await;
        let resp = self
            .client
            .post(&url)
            .json(json)
            .send()
            .await
            .map_err(|e| CaddyError::Http { source: e })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(CaddyError::Api { status, body });
        }
        Ok(())
    }
}

impl NetworkProxy for CaddyProxy {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.is_healthy_impl().await.map_err(Into::into) })
    }

    fn apply_config<'a>(&'a self, config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_config_impl(config).await.map_err(Into::into) })
    }
}

// ---------------------------------------------------------------------------
// Startup constants
// ---------------------------------------------------------------------------

pub(crate) const CADDY_BLUE: &str = "seedling-caddy-blue";
pub(crate) const CADDY_GREEN: &str = "seedling-caddy-green";
pub(crate) const CADDY_IMAGE: &str = "localhost/seedling-caddy:latest";
pub(crate) const CADDY_DATA_VOLUME: &str = "seedling-caddy-data";
pub(crate) const PROXY_NETWORK: &str = "seedling-proxy";
/// Minimal Caddy JSON config that binds the admin API on all interfaces.
const CADDY_ADMIN_JSON: &str = r#"{"admin":{"listen":":2019"}}"#;

const CADDY_CONTAINERFILE: &str = "\
FROM docker.io/library/caddy:2.11.2-builder AS builder\n\
RUN xcaddy build --with github.com/mholt/caddy-l4\n\
FROM docker.io/library/caddy:2.11.2\n\
COPY --from=builder /usr/bin/caddy /usr/bin/caddy\n";

// ---------------------------------------------------------------------------
// Startup helpers
// ---------------------------------------------------------------------------

/// Returns the /64 infrastructure prefix for the seedling-proxy network.
///
/// The network sits at `fd5e:edXX:XXXX:ff00::/64` within the node's /48,
/// using `0xff` as the subnet discriminant (above the resource-kind range 0–9).
// r[impl infra.proxy.startup]
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
    },
    #[snafu(display("process manager error: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("I/O error writing admin config: {source}"))]
    Io { source: std::io::Error },
    #[snafu(display("Caddy did not become healthy within the timeout"))]
    Timeout,
    #[snafu(display("database error: {source}"))]
    Db { source: rusqlite::Error },
    #[snafu(display("image ID unavailable for {reference}"))]
    ImageId { reference: String },
    #[snafu(display("image build failed: {message}"))]
    Build { message: String },
}

/// Build the custom Caddy image from the embedded Containerfile.
async fn build_caddy_image(data_dir: &std::path::Path) -> Result<(), CaddyStartupError> {
    let containerfile_path = data_dir.join("Containerfile.caddy");
    std::fs::write(&containerfile_path, CADDY_CONTAINERFILE)
        .map_err(|e| CaddyStartupError::Io { source: e })?;

    // podman build needs a context directory; the Containerfile has no
    // local COPY instructions so an empty temp dir suffices.
    let context_dir = data_dir.join("caddy-build-ctx");
    std::fs::create_dir_all(&context_dir).map_err(|e| CaddyStartupError::Io { source: e })?;

    tracing::info!("building custom Caddy image (this may take a moment)");

    let output = tokio::process::Command::new("podman")
        .args([
            "build",
            "-t",
            CADDY_IMAGE,
            "-f",
            &containerfile_path.to_string_lossy(),
            &context_dir.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| CaddyStartupError::Io { source: e })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CaddyStartupError::Build {
            message: format!("podman build exited {}: {}", output.status, stderr.trim()),
        });
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

pub(crate) fn read_cached_proxy_json(
    data_dir: &std::path::Path,
) -> Result<Option<serde_json::Value>, rusqlite::Error> {
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

pub(crate) fn write_cached_proxy_json(
    data_dir: &std::path::Path,
    json: &serde_json::Value,
) -> Result<(), rusqlite::Error> {
    let conn = caddy_db_open(data_dir)?;
    let json_str = serde_json::to_string(json).unwrap_or_default();
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
    data_dir: &std::path::Path,
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
    process
        .start_transient(TransientUnitSpec {
            name: unit_name.clone(),
            description: "seedling Caddy proxy".to_owned(),
            exec_start: vec![
                "podman".to_owned(),
                "run".to_owned(),
                "--rm".to_owned(),
                "--name".to_owned(),
                container_name.to_owned(),
                "--network".to_owned(),
                PROXY_NETWORK.to_owned(),
                "--volume".to_owned(),
                format!("{CADDY_DATA_VOLUME}:/data"),
                "--volume".to_owned(),
                format!("{admin_config_str}:/etc/caddy/admin.json:ro"),
                CADDY_IMAGE.to_owned(),
                "caddy".to_owned(),
                "run".to_owned(),
                "--config".to_owned(),
                "/etc/caddy/admin.json".to_owned(),
            ],
            restart: TransientRestart::Always,
        })
        .await
        .map_err(|e| CaddyStartupError::Process { source: e })
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

/// Poll `container_name` until it is running and Caddy's admin API responds,
/// or until the deadline elapses. Returns `CaddyAddrs` on success.
#[tracing::instrument(skip_all, fields(%container_name))]
async fn poll_until_healthy(
    container_name: &str,
    container: &dyn ContainerRuntime,
    deadline: tokio::time::Instant,
) -> Result<CaddyAddrs, CaddyStartupError> {
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(CaddyStartupError::Timeout);
        }
        if let Ok(Some(state)) = container.inspect(container_name).await
            && matches!(state.status, ContainerStatus::Running)
            && let Some(ip) = state.pod_addr
        {
            let v6 = SocketAddr::new(IpAddr::V6(ip), 2019);
            if CaddyProxy::new(v6).is_healthy().await.unwrap_or(false) {
                let v4 = state
                    .pod_addr_v4
                    .map(|ip4| SocketAddr::new(IpAddr::V4(ip4), 2019));
                return Ok(CaddyAddrs { v6, v4 });
            }
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
/// 4. Reads the active slot from the database (defaults to `seedling-caddy`).
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
    data_dir: &std::path::Path,
) -> Result<CaddyAddrs, CaddyStartupError> {
    // 1. Ensure the proxy network exists.
    let proxy_prefix = proxy_network_prefix(node_prefix);
    if !container
        .network_exists(PROXY_NETWORK)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        let _ = container
            .create_network(PROXY_NETWORK, proxy_prefix, Some(proxy_ipv4_subnet()))
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // 2. Write admin config so Caddy binds the admin API on all interfaces.
    let admin_config_path = data_dir.join("caddy-admin.json");
    std::fs::write(&admin_config_path, CADDY_ADMIN_JSON)
        .map_err(|e| CaddyStartupError::Io { source: e })?;

    // 3. Ensure the data volume exists.
    if !container
        .volume_exists(CADDY_DATA_VOLUME)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        container
            .create_volume(CADDY_DATA_VOLUME)
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // 4. Read active container name from DB (default to blue slot).
    //    Normalize legacy names ("seedling-caddy", "seedling-caddy-next")
    //    to the blue/green scheme so upgrades from older installations
    //    converge cleanly.
    let active = {
        let conn = caddy_db_open(data_dir).map_err(|e| CaddyStartupError::Db { source: e })?;
        let raw = read_active_container(&conn)
            .map_err(|e| CaddyStartupError::Db { source: e })?
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
            let conn = caddy_db_open(data_dir).map_err(|e| CaddyStartupError::Db { source: e })?;
            write_active_container(&conn, CADDY_BLUE)
                .map_err(|e| CaddyStartupError::Db { source: e })?;
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
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        build_caddy_image(data_dir).await?;
    }

    // 8. Get the desired image ID.
    let desired_id = container
        .local_image_id(CADDY_IMAGE)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
        .ok_or_else(|| CaddyStartupError::ImageId {
            reference: CADDY_IMAGE.to_owned(),
        })?;

    // 9. Inspect the active container.
    let active_state = container
        .inspect(&active)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?;

    match active_state {
        Some(state) if matches!(state.status, ContainerStatus::Running) => {
            if state.image_id.as_deref() == Some(&desired_id) {
                // Correct image — check if already healthy.
                if let Some(ip) = state.pod_addr {
                    let addr = SocketAddr::new(IpAddr::V6(ip), 2019);
                    if CaddyProxy::new(addr).is_healthy().await.unwrap_or(false) {
                        let v4 = state
                            .pod_addr_v4
                            .map(|ip4| SocketAddr::new(IpAddr::V4(ip4), 2019));
                        return Ok(CaddyAddrs { v6: addr, v4 });
                    }
                    tracing::warn!(
                        container = %active,
                        addr = %addr,
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
                // Wrong image — perform blue/green upgrade.
                start_slot(other, container, process, data_dir).await?;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                let new_addrs = poll_until_healthy(other, container, deadline).await?;

                // r[impl infra.proxy.upgrade.cache]
                // Apply the cached proxy config to the new container.
                if let Ok(Some(json)) = read_cached_proxy_json(data_dir)
                    && let Err(e) = CaddyProxy::new(new_addrs.v6).apply_raw_json(&json).await
                {
                    tracing::warn!("failed to apply cached proxy config to upgraded Caddy: {e}");
                }

                // Record the new active slot.
                let conn =
                    caddy_db_open(data_dir).map_err(|e| CaddyStartupError::Db { source: e })?;
                write_active_container(&conn, other)
                    .map_err(|e| CaddyStartupError::Db { source: e })?;

                return Ok(new_addrs);
            }
        }
        Some(state) => {
            // Container exists but is not running — force-remove it.
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
    let addrs = poll_until_healthy(&active, container, deadline).await?;

    // r[impl infra.proxy.upgrade.cache]
    // Apply the cached proxy config.
    if let Ok(Some(json)) = read_cached_proxy_json(data_dir)
        && let Err(e) = CaddyProxy::new(addrs.v6).apply_raw_json(&json).await
    {
        tracing::warn!("failed to apply cached proxy config to fresh Caddy: {e}");
    }

    // Record the active slot.
    let conn = caddy_db_open(data_dir).map_err(|e| CaddyStartupError::Db { source: e })?;
    write_active_container(&conn, &active).map_err(|e| CaddyStartupError::Db { source: e })?;

    Ok(addrs)
}

// ---------------------------------------------------------------------------
// ProxyConfig → Caddy JSON
// ---------------------------------------------------------------------------

/// Converts a `ProxyConfig` into the Caddy admin API JSON format sent to
/// `POST /config/`. Caddy applies this atomically with no traffic drop.
///
/// Two HTTP servers are created when both HTTP and HTTPS listeners are
/// present, keeping redirect-only and proxy-only routes clearly separated:
///
/// - `seedling_https`: listens on all HTTPS/QUIC ports, serves proxy routes
///   for TLS-enabled virtual hosts.
/// - `seedling_http`: listens on all plain-HTTP ports, serves redirect routes
///   (for hosts with `tls_acme=true`) and proxy routes (for plain-HTTP hosts).
///
/// TLS certificates are obtained via ACME (Let's Encrypt) for any virtual
/// host with `tls_acme=true`.
pub(crate) fn build_caddy_config(config: &ProxyConfig) -> Value {
    let http_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Http)
        .map(|l| l.port)
        .collect();

    let https_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Https)
        .map(|l| l.port)
        .collect();

    let quic_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Quic)
        .map(|l| l.port)
        .collect();

    let mut servers = serde_json::Map::new();

    // --- HTTPS server ---
    let mut https_listens: Vec<String> = https_ports.iter().map(|p| format!(":{p}")).collect();
    for p in &quic_ports {
        https_listens.push(format!(":{p}/quic"));
    }
    https_listens.dedup();

    if !https_listens.is_empty() {
        let https_routes: Vec<Value> = config
            .virtual_hosts
            .iter()
            .filter(|vh| vh.tls_acme)
            .flat_map(proxy_routes_for_vhost)
            .collect();

        if !https_routes.is_empty() {
            servers.insert(
                "seedling_https".to_string(),
                json!({ "listen": https_listens, "routes": https_routes }),
            );
        }
    }

    // --- HTTP server ---
    let http_listens: Vec<String> = http_ports.iter().map(|p| format!(":{p}")).collect();
    if !http_listens.is_empty() {
        let mut http_routes: Vec<Value> = Vec::new();

        for vh in &config.virtual_hosts {
            if let Some(redirect) = &vh.redirect {
                // Redirect route: HTTP → HTTPS
                http_routes.push(redirect_route(&vh.hostname, redirect.code, &https_ports));
            } else if !vh.tls_acme {
                // Plain HTTP proxy route
                http_routes.extend(proxy_routes_for_vhost(vh));
            }
        }

        if !http_routes.is_empty() {
            servers.insert(
                "seedling_http".to_string(),
                json!({ "listen": http_listens, "routes": http_routes }),
            );
        }
    }

    // --- TLS automation ---
    let tls_subjects: Vec<&str> = config
        .virtual_hosts
        .iter()
        .filter(|vh| vh.tls_acme)
        .map(|vh| vh.hostname.as_str())
        .collect();

    let mut apps = json!({ "http": { "servers": servers } });

    if !tls_subjects.is_empty() {
        apps["tls"] = json!({
            "automation": {
                "policies": [{
                    "subjects": tls_subjects,
                    "issuers": [{ "module": "acme" }],
                }]
            }
        });
    }

    if !config.l4_routes.is_empty() {
        let mut l4_servers = serde_json::Map::new();

        for route in &config.l4_routes {
            let proto_str = match route.proto {
                L4Proto::Tcp => "tcp",
                L4Proto::Udp => "udp",
            };
            let server_name = format!("l4_{proto_str}_{}", route.port);
            let listen = format!("{proto_str}/:{}", route.port);

            let upstreams: Vec<Value> = route
                .upstreams
                .iter()
                .map(|u| json!({ "dial": [u] }))
                .collect();

            l4_servers.insert(
                server_name,
                json!({
                    "listen": [listen],
                    "routes": [{
                        "handle": [{
                            "handler": "proxy",
                            "upstreams": upstreams,
                        }]
                    }]
                }),
            );
        }

        apps["layer4"] = json!({ "servers": l4_servers });
    }

    json!({ "admin": { "listen": ":2019" }, "apps": apps })
}

/// Builds one Caddy route object per `ProxyRoute` within a virtual host.
fn proxy_routes_for_vhost(vh: &VirtualHost) -> Vec<Value> {
    vh.routes
        .iter()
        .map(|route| {
            let match_expr = if route.prefix == "/" {
                json!({ "host": [&vh.hostname] })
            } else {
                json!({ "host": [&vh.hostname], "path": [format!("{}*", route.prefix)] })
            };

            let upstreams: Vec<Value> = route
                .upstreams
                .iter()
                .map(|u| {
                    // Upstream URLs are "http://[fd5e:...]:3000".
                    // Caddy's `dial` field expects "[fd5e:...]:3000" (no scheme).
                    let dial = u.strip_prefix("http://").unwrap_or(u).to_string();
                    json!({ "dial": dial })
                })
                .collect();

            json!({
                "match": [match_expr],
                "handle": [{
                    "handler": "reverse_proxy",
                    "upstreams": upstreams,
                }],
                "terminal": true,
            })
        })
        .collect()
}

/// Builds a Caddy route that issues an HTTP redirect to the HTTPS port.
fn redirect_route(hostname: &str, code: u16, https_ports: &[u16]) -> Value {
    let target_port = https_ports.first().copied().unwrap_or(443);
    let location = if target_port == 443 {
        "https://{http.request.host}{http.request.uri}".to_string()
    } else {
        format!("https://{{http.request.host}}:{target_port}{{http.request.uri}}")
    };

    json!({
        "match": [{ "host": [hostname] }],
        "handle": [{
            "handler": "static_response",
            "status_code": code,
            "headers": { "Location": [location] },
        }],
        "terminal": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::types::{HttpRedirect, ProxyListener, ProxyRoute, VirtualHost};

    fn http_vhost(hostname: &str, upstream: &str) -> VirtualHost {
        VirtualHost {
            hostname: hostname.to_string(),
            tls_acme: false,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                upstreams: vec![format!("http://{upstream}")],
            }],
        }
    }

    fn https_vhost(hostname: &str, upstream: &str) -> VirtualHost {
        VirtualHost {
            hostname: hostname.to_string(),
            tls_acme: true,
            redirect: Some(HttpRedirect {
                from_port: 80,
                code: 308,
            }),
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                upstreams: vec![format!("http://{upstream}")],
            }],
        }
    }

    #[test]
    fn empty_config_produces_empty_servers() {
        let config = ProxyConfig::default();
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(servers.as_object().is_none_or(|m| m.is_empty()));
    }

    #[test]
    fn http_only_vhost_goes_in_http_server() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 80,
                proto: ProxyListenerProto::Http,
            }],
            virtual_hosts: vec![http_vhost("example.com", "[fd5e::1]:3000")],
            l4_routes: vec![],
        };
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(servers["seedling_http"].is_object());
        assert!(servers["seedling_https"].is_null());
    }

    #[test]
    fn https_vhost_goes_in_https_server_redirect_in_http() {
        let config = ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 80,
                    proto: ProxyListenerProto::Http,
                },
            ],
            virtual_hosts: vec![https_vhost("example.com", "[fd5e::1]:3000")],
            l4_routes: vec![],
        };
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(
            servers["seedling_https"].is_object(),
            "missing https server"
        );
        assert!(servers["seedling_http"].is_object(), "missing http server");

        // https server should have proxy routes
        let https_routes = &servers["seedling_https"]["routes"];
        assert!(https_routes.as_array().is_some_and(|r| !r.is_empty()));

        // http server should have redirect route
        let http_routes = &servers["seedling_http"]["routes"];
        let redirect = &http_routes[0];
        assert_eq!(redirect["handle"][0]["handler"], "static_response");
        assert_eq!(redirect["handle"][0]["status_code"], 308);
    }

    #[test]
    fn tls_acme_subjects_appear_in_automation() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            }],
            virtual_hosts: vec![VirtualHost {
                hostname: "secure.example.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                }],
            }],
            l4_routes: vec![],
        };
        let json = build_caddy_config(&config);
        let subjects = &json["apps"]["tls"]["automation"]["policies"][0]["subjects"];
        assert_eq!(subjects[0], "secure.example.com");
    }

    #[test]
    fn dial_strips_http_scheme() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            }],
            virtual_hosts: vec![VirtualHost {
                hostname: "x.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e:ed12:3456:0100::3]:3000".to_string()],
                }],
            }],
            l4_routes: vec![],
        };
        let json = build_caddy_config(&config);
        let dial = &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][0]["upstreams"]
            [0]["dial"];
        assert_eq!(dial, "[fd5e:ed12:3456:0100::3]:3000");
    }

    #[test]
    fn quic_listener_appended_to_https_server() {
        let config = ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Quic,
                },
            ],
            virtual_hosts: vec![VirtualHost {
                hostname: "quic.example.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                }],
            }],
            l4_routes: vec![],
        };
        let json = build_caddy_config(&config);
        let listen = &json["apps"]["http"]["servers"]["seedling_https"]["listen"];
        let listen_strs: Vec<&str> = listen
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(listen_strs.contains(&":443"));
        assert!(listen_strs.contains(&":443/quic"));
    }
}
