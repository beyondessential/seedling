use std::{
    net::{Ipv6Addr, SocketAddr},
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use ipnet::Ipv6Net;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use snafu::{ResultExt, Snafu};

use crate::system::priority;
use crate::system::{
    ContainerRuntime, ProcessManager,
    types::{
        ContainerState, ContainerStatus, SPEC_HASH_LABEL, TransientRestart, TransientUnitSpec,
    },
};

use super::{resolver_addr, resolver_ipv4_subnet, resolver_network_prefix};

pub(crate) const RESOLVER_BLUE: &str = "seedling-resolver-blue";
pub(crate) const RESOLVER_GREEN: &str = "seedling-resolver-green";
pub(crate) const RESOLVER_IMAGE: &str = "docker.io/coredns/coredns:1.12.1";
pub const RESOLVER_NETWORK: &str = "seedling-resolver";

/// Number of consecutive inline-health-check failures the runtime
/// tolerates before deciding the container is genuinely sick and
/// stop+restarting it. With a 2-second per-check timeout and a 5-second
/// reconciler tick, a real outage trips after ≤15s while a transient
/// host-load blip (where coredns is alive but the HTTP probe missed its
/// deadline) doesn't bounce the container.
pub(crate) const HEALTH_CHECK_FAIL_THRESHOLD: u32 = 3;

#[derive(Debug)]
pub struct ResolverAddrs {
    pub v6: Ipv6Addr,
}

#[derive(Debug, Snafu)]
pub enum ResolverStartupError {
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
    #[snafu(display("I/O error: {source}"))]
    Io {
        source: std::io::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("health check timed out"))]
    Timeout { backtrace: snafu::Backtrace },
    #[snafu(display("database error: {source}"))]
    Db {
        source: rusqlite::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image {reference} not found after pull"))]
    ImageId {
        reference: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image pull failed: {message}"))]
    Pull {
        message: String,
        backtrace: snafu::Backtrace,
    },
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

fn resolver_db_open(data_dir: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    let conn = rusqlite::Connection::open(data_dir.join("seedling.db"))?;
    // spec: INFRA#database
    // Another connection holds the write lock for moments at a time; waiting
    // for it is not a failure.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS resolver_state (
            singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
            active_container TEXT NOT NULL
        )",
    )?;
    Ok(conn)
}

fn read_active_container(conn: &rusqlite::Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT active_container FROM resolver_state WHERE singleton = 1",
        [],
        |r| r.get(0),
    )
    .optional()
}

fn write_active_container(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO resolver_state (singleton, active_container)
         VALUES (1, ?1)
         ON CONFLICT(singleton) DO UPDATE SET active_container = excluded.active_container",
        rusqlite::params![name],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Slot helpers
// ---------------------------------------------------------------------------

fn other_slot(active: &str) -> &'static str {
    if active == RESOLVER_BLUE {
        RESOLVER_GREEN
    } else {
        RESOLVER_BLUE
    }
}

fn slot_unit(container: &str) -> String {
    format!("{container}.service")
}

/// Fingerprint of what a resolver container is started with: its Corefile and
/// the address it listens at. The image is compared separately, by id.
fn config_hash(corefile: &str, resolver_ip: &Ipv6Addr) -> String {
    let mut hasher = Sha256::new();
    hasher.update(corefile.as_bytes());
    hasher.update([0]);
    hasher.update(resolver_ip.octets());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// How a running resolver container compares with what is wanted.
#[derive(Debug, PartialEq, Eq)]
enum Running {
    UpToDate,
    StaleImage,
    StaleConfig,
}

fn assess_running(state: &ContainerState, desired_image_id: &str, desired_config: &str) -> Running {
    if state.image_id.as_deref() != Some(desired_image_id) {
        Running::StaleImage
    } else if state.spec_hash.as_deref() != Some(desired_config) {
        Running::StaleConfig
    } else {
        Running::UpToDate
    }
}

// ---------------------------------------------------------------------------
// Container lifecycle
// ---------------------------------------------------------------------------

#[tracing::instrument(skip_all, fields(%container_name))]
async fn start_slot(
    container_name: &str,
    process: &dyn ProcessManager,
    data_dir: &Path,
    resolver_ip: &Ipv6Addr,
    config_hash: &str,
) -> Result<(), ResolverStartupError> {
    let unit_name = slot_unit(container_name);

    if process
        .unit_state(&unit_name)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        let _ = process.reset_failed_unit(&unit_name).await;
        let _ = process.stop_unit(&unit_name).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            if process
                .unit_state(&unit_name)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    let corefile_path = data_dir.join("Corefile");
    let corefile_str = corefile_path.to_string_lossy().into_owned();

    process
        .start_transient(TransientUnitSpec {
            name: unit_name,
            description: "seedling CoreDNS resolver".to_owned(),
            exec_start: vec![
                "podman".to_owned(),
                "run".to_owned(),
                "--rm".to_owned(),
                "--cap-drop=ALL".to_owned(),
                "--cap-add=NET_BIND_SERVICE".to_owned(),
                "--security-opt".to_owned(),
                "no-new-privileges".to_owned(),
                "--read-only".to_owned(),
                "--pids-limit".to_owned(),
                "32".to_owned(),
                "--name".to_owned(),
                container_name.to_owned(),
                "--network".to_owned(),
                RESOLVER_NETWORK.to_owned(),
                "--ip6".to_owned(),
                resolver_ip.to_string(),
                "--label".to_owned(),
                format!("{SPEC_HASH_LABEL}={config_hash}"),
                "--volume".to_owned(),
                format!("{corefile_str}:/Corefile:ro"),
                RESOLVER_IMAGE.to_owned(),
                "-conf".to_owned(),
                "/Corefile".to_owned(),
            ],
            restart: TransientRestart::Always,
            log_extra_fields: vec![("SEEDLING_INFRA".to_owned(), "resolver".to_owned())],
            kill_signal: None,
            timeout_stop_secs: None,
            // spec: PRIO#kill-order
            // Infrastructure ranks above every app workload: a host under
            // memory pressure must not lose the components that route to
            // whatever survives.
            slice: Some(priority::INFRA_SLICE.to_owned()),
            oom_score_adjust: Some(priority::INFRA_OOM_SCORE_ADJUST),
            restart_sec: Some(5),
            start_limit_interval_sec: Some(600),
            start_limit_burst: Some(10),
        })
        .await
        .context(ProcessSnafu)
}

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

// ---------------------------------------------------------------------------
// Health checking
// ---------------------------------------------------------------------------

#[tracing::instrument(skip_all, fields(%container_name))]
async fn poll_until_healthy(
    container_name: &str,
    container: &dyn ContainerRuntime,
    deadline: tokio::time::Instant,
    resolver_ip: &Ipv6Addr,
) -> Result<ResolverAddrs, ResolverStartupError> {
    let health_url = format!("http://[{}]:8080/health", resolver_ip);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    loop {
        if tokio::time::Instant::now() >= deadline {
            return TimeoutSnafu.fail();
        }
        if let Ok(Some(state)) = container.inspect(container_name).await
            && matches!(state.status, ContainerStatus::Running)
            && client
                .get(&health_url)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
        {
            return Ok(ResolverAddrs { v6: *resolver_ip });
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[tracing::instrument(skip_all)]
pub async fn teardown_resolver(container: &dyn ContainerRuntime, process: &dyn ProcessManager) {
    for slot in [RESOLVER_BLUE, RESOLVER_GREEN] {
        if container.inspect(slot).await.ok().flatten().is_some() {
            tracing::info!(container = slot, "tearing down resolver slot");
            stop_slot(slot, process, container).await;
        }
    }

    if container
        .network_exists(RESOLVER_NETWORK)
        .await
        .unwrap_or(false)
    {
        let _ = container.remove_network(RESOLVER_NETWORK).await;
    }
}

/// `inline_health_fail_count` is a caller-owned counter of consecutive
/// failed inline health checks. Cleared on success; the function only
/// stop+restarts the container after the count crosses
/// [`HEALTH_CHECK_FAIL_THRESHOLD`], so a single missed 2-second probe
/// doesn't churn the resolver when the container is alive but the host
/// is briefly busy.
#[expect(
    clippy::too_many_arguments,
    reason = "infra subsystem startup needs the full set of runtime handles"
)]
#[tracing::instrument(skip_all, level = "debug")]
pub async fn ensure_resolver_running(
    container: &dyn ContainerRuntime,
    process: &dyn ProcessManager,
    node_prefix: &Ipv6Net,
    data_dir: &Path,
    upstreams: &[SocketAddr],
    nat64_active: bool,
    force_dns64_translation: bool,
    inline_health_fail_count: &AtomicU32,
) -> Result<ResolverAddrs, ResolverStartupError> {
    let resolver_prefix = resolver_network_prefix(node_prefix);
    let resolver_ip = resolver_addr(node_prefix);

    // 1. Ensure resolver network exists (dual-stack so CoreDNS can forward
    //    to IPv4 upstream DNS servers).
    if !container
        .network_exists(RESOLVER_NETWORK)
        .await
        .context(ContainerSnafu)?
    {
        let _ = container
            .create_network(
                RESOLVER_NETWORK,
                resolver_prefix,
                Some(resolver_ipv4_subnet()),
            )
            .await
            .context(ContainerSnafu)?;
    }

    // 2. Write the Corefile.
    let corefile_path = data_dir.join("Corefile");
    let corefile_content =
        super::config::generate_corefile(upstreams, nat64_active, force_dns64_translation);
    std::fs::write(&corefile_path, &corefile_content).context(IoSnafu)?;
    let desired_config = config_hash(&corefile_content, &resolver_ip);

    // 3. Read active container from DB (default blue).
    let mut active = {
        let conn = resolver_db_open(data_dir).context(DbSnafu)?;
        read_active_container(&conn)
            .context(DbSnafu)?
            .unwrap_or_else(|| RESOLVER_BLUE.to_owned())
    };

    let other = other_slot(&active);

    // 4. Clean up stale other-slot.
    if container.inspect(other).await.ok().flatten().is_some() {
        stop_slot(other, process, container).await;
    }

    // 5. Ensure image is present.
    if !container
        .image_exists(RESOLVER_IMAGE)
        .await
        .context(ContainerSnafu)?
    {
        container
            .pull_image(RESOLVER_IMAGE)
            .await
            .context(ContainerSnafu)?;
    }

    // 6. Get desired image ID.
    let desired_id = container
        .local_image_id(RESOLVER_IMAGE)
        .await
        .context(ContainerSnafu)?
        .ok_or_else(|| {
            ImageIdSnafu {
                reference: RESOLVER_IMAGE.to_owned(),
            }
            .build()
        })?;

    // 7. Inspect active container.
    let active_state = container.inspect(&active).await.context(ContainerSnafu)?;

    match active_state {
        Some(state) if matches!(state.status, ContainerStatus::Running) => {
            let assessment = assess_running(&state, &desired_id, &desired_config);
            if assessment == Running::UpToDate {
                let health_url = format!("http://[{}]:8080/health", resolver_ip);
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap_or_default();
                let healthy = client
                    .get(&health_url)
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success());
                if healthy {
                    inline_health_fail_count.store(0, Ordering::Relaxed);
                    return Ok(ResolverAddrs { v6: resolver_ip });
                }
                let fails = inline_health_fail_count.fetch_add(1, Ordering::Relaxed) + 1;
                if fails < HEALTH_CHECK_FAIL_THRESHOLD {
                    // Container is `Running`; the 2-second probe is
                    // tight enough that one miss is almost always a
                    // host-load blip rather than a real outage. Trust
                    // the running container, return Ok with the cached
                    // address, and try again next tick. systemd's
                    // Restart=Always will catch a genuine container
                    // crash independently.
                    tracing::warn!(
                        container = %active,
                        consecutive_failures = fails,
                        threshold = HEALTH_CHECK_FAIL_THRESHOLD,
                        "resolver inline health check failed; deferring restart pending more failures"
                    );
                    return Ok(ResolverAddrs { v6: resolver_ip });
                }
                tracing::warn!(
                    container = %active,
                    consecutive_failures = fails,
                    "resolver inline health check failed for {HEALTH_CHECK_FAIL_THRESHOLD} consecutive ticks; restarting"
                );
                inline_health_fail_count.store(0, Ordering::Relaxed);
                stop_slot(&active, process, container).await;
            } else {
                // spec: DNS#resolver-replacement
                tracing::info!(
                    container = %active,
                    ?assessment,
                    running_image = ?state.image_id,
                    desired_image = %desired_id,
                    "resolver is not what is wanted; replacing it"
                );
                // The resolver address is held by the running container, and a
                // second container cannot take it while that one has it, so the
                // replacement starts only once the running one is gone. The
                // slot to start in is recorded before anything is started, so
                // that an interruption from here on leaves a database that
                // names a slot the next tick will start fresh.
                stop_slot(&active, process, container).await;
                let conn = resolver_db_open(data_dir).context(DbSnafu)?;
                write_active_container(&conn, other).context(DbSnafu)?;
                inline_health_fail_count.store(0, Ordering::Relaxed);
                active = other.to_owned();
            }
        }
        Some(state) => {
            tracing::warn!(
                container = %active,
                status = ?state.status,
                "resolver container exists but is not running; removing and restarting"
            );
            let _ = container.remove_container(&active, true).await;
        }
        None => {
            tracing::info!(container = %active, "resolver container not found; starting fresh");
        }
    }

    // 8. Fresh start.
    start_slot(&active, process, data_dir, &resolver_ip, &desired_config).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let addrs = poll_until_healthy(&active, container, deadline, &resolver_ip).await?;
    let conn = resolver_db_open(data_dir).context(DbSnafu)?;
    write_active_container(&conn, &active).context(DbSnafu)?;
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::types::ContainerHealth;

    fn running(image_id: Option<&str>, spec_hash: Option<&str>) -> ContainerState {
        ContainerState {
            status: ContainerStatus::Running,
            health: ContainerHealth::None,
            pid: None,
            exit_code: None,
            started_at: None,
            finished_at: None,
            pod_addr: None,
            pod_addr_v4: None,
            image_id: image_id.map(str::to_owned),
            spec_hash: spec_hash.map(str::to_owned),
        }
    }

    // spec: DNS#resolver-replacement
    #[test]
    fn config_hash_follows_corefile_and_address() {
        let a: Ipv6Addr = "fd5e:a5:bac1:fd00::53".parse().unwrap();
        let b: Ipv6Addr = "fd5e:a5:bac1:fd00::35".parse().unwrap();
        let base = config_hash("forward . 1.1.1.1\n", &a);
        assert_eq!(base, config_hash("forward . 1.1.1.1\n", &a));
        assert_ne!(base, config_hash("forward . 8.8.8.8\n", &a));
        assert_ne!(base, config_hash("forward . 1.1.1.1\n", &b));
    }

    // spec: DNS#resolver-replacement
    #[test]
    fn running_resolver_with_changed_config_is_stale() {
        let state = running(Some("img"), Some("old"));
        assert_eq!(assess_running(&state, "img", "new"), Running::StaleConfig);
    }

    // spec: DNS#resolver-replacement
    #[test]
    fn running_resolver_without_a_config_label_is_stale() {
        let state = running(Some("img"), None);
        assert_eq!(assess_running(&state, "img", "new"), Running::StaleConfig);
    }

    // spec: DNS#resolver-replacement
    #[test]
    fn running_resolver_on_another_image_is_stale() {
        let state = running(Some("old-img"), Some("cfg"));
        assert_eq!(assess_running(&state, "img", "cfg"), Running::StaleImage);
        let state = running(None, Some("cfg"));
        assert_eq!(assess_running(&state, "img", "cfg"), Running::StaleImage);
    }

    #[test]
    fn running_resolver_matching_image_and_config_is_up_to_date() {
        let state = running(Some("img"), Some("cfg"));
        assert_eq!(assess_running(&state, "img", "cfg"), Running::UpToDate);
    }

    #[test]
    fn other_slot_alternates() {
        assert_eq!(other_slot(RESOLVER_BLUE), RESOLVER_GREEN);
        assert_eq!(other_slot(RESOLVER_GREEN), RESOLVER_BLUE);
    }
}

#[cfg(test)]
mod db_tests {
    // spec: INFRA#database
    #[test]
    fn the_resolvers_database_connection_waits_for_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let conn = super::resolver_db_open(dir.path()).unwrap();
        let ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert!(ms > 0);
    }
}
