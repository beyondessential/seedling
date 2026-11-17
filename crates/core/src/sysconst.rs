//! Host-level system facts surfaced to BSL as `const.*` constants.
//!
//! Values populated here back constants defined in `spec: CONST`.
//! Some values (IPv4/IPv6 egress, BTRFS availability, etc.) can only be
//! determined asynchronously or from runtime daemon state, so they are
//! cached here once at daemon startup and read synchronously from
//! `defs::scope()` on every script execution.

use parking_lot::RwLock;

/// Snapshot of host facts captured at daemon startup.
///
/// Non-daemon callers (tests, `ctl`, `web`) never populate this; they see
/// the [`Default`] value, which models a minimally-capable environment.
#[derive(Debug, Clone)]
pub struct SystemFacts {
    /// Host has a default IPv4 route.
    pub ipv4_egress: bool,
    /// Host has IPv6 egress (default route plus a global-unicast source).
    pub ipv6_egress: bool,
    /// Seedling has activated its own NAT64 translator on this node.
    pub nat64_active: bool,
    /// Compute threads available to the app; `None` while undetermined.
    pub available_threads: Option<i64>,
    /// Memory available to the app in bytes, or why it is undetermined.
    pub available_memory: Result<i64, String>,
    /// The node's volume storage supports copy-on-write snapshots.
    pub has_snapshots: bool,
    /// Human-readable identifier for the node, or why it is undetermined.
    pub node_name: Result<String, String>,
    /// IANA timezone name of the host (e.g. `Pacific/Auckland`); `UTC` while
    /// the host's local timezone is undetermined.
    pub timezone: String,
}

impl Default for SystemFacts {
    /// Outside a running daemon the host's identity has no meaning, so the
    /// node name is empty; the compute and memory figures are still the
    /// host's.
    fn default() -> Self {
        Self {
            ipv4_egress: false,
            ipv6_egress: false,
            nat64_active: false,
            available_threads: detect_available_threads().ok(),
            available_memory: detect_available_memory(),
            has_snapshots: false,
            node_name: Ok(String::new()),
            timezone: "UTC".to_owned(),
        }
    }
}

/// The number of compute threads available to the daemon process.
///
/// `std::thread::available_parallelism` honours cgroup CPU quotas, sched
/// affinity, and platform limits, which matches the spec's framing (the
/// threads available to the app, not the cores on the host).
// spec: CONST#host-constants
pub fn detect_available_threads() -> Result<i64, String> {
    let n = std::thread::available_parallelism()
        .map_err(|e| format!("cannot read the compute threads available: {e}"))?;
    Ok(i64::try_from(n.get()).unwrap_or(i64::MAX))
}

/// Memory the script may plan around: 90% of the host's `MemTotal`,
/// reserving 10% for the daemon itself, the runtime, and the kernel's page
/// cache.
// spec: CONST#host-constants
pub fn detect_available_memory() -> Result<i64, String> {
    let contents = std::fs::read_to_string("/proc/meminfo")
        .map_err(|e| format!("cannot read /proc/meminfo: {e}"))?;
    let total = parse_mem_total_bytes(&contents)?;
    let available = total.saturating_mul(9) / 10;
    if available <= 0 {
        return Err(format!("the host reports {total} bytes of memory"));
    }
    Ok(available)
}

/// `MemTotal` from the contents of `/proc/meminfo`, in bytes.
fn parse_mem_total_bytes(meminfo: &str) -> Result<i64, String> {
    let line = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .ok_or("/proc/meminfo has no MemTotal line")?;
    let mut parts = line.split_whitespace();
    let (Some(num), Some(unit)) = (parts.next(), parts.next()) else {
        return Err(format!("MemTotal {line:?} is not a number and a unit"));
    };
    let value = num
        .parse::<i64>()
        .map_err(|e| format!("MemTotal {num:?} is not a number: {e}"))?;
    let multiplier: i64 = match unit {
        "B" => 1,
        "kB" | "KB" => 1024,
        "MB" => 1024 * 1024,
        "GB" => 1024 * 1024 * 1024,
        other => return Err(format!("MemTotal has an unknown unit {other:?}")),
    };
    Ok(value.saturating_mul(multiplier))
}

/// The host's name.
// spec: CONST#host-constants
pub fn detect_node_name() -> Result<String, String> {
    whoami::devicename().map_err(|e| format!("cannot read the host's name: {e}"))
}

/// The host's IANA timezone name, or why it cannot be determined (e.g. an
/// unusual `/etc/localtime` configuration). An undetermined time zone is not
/// `UTC`: the caller decides what stands for it.
// spec: CONST#determining-host-constants
pub fn detect_timezone() -> Result<String, String> {
    let zone = iana_time_zone::get_timezone()
        .map_err(|e| format!("cannot read the host's time zone: {e}"))?;
    if zone.is_empty() {
        return Err("the host's time zone name is empty".to_owned());
    }
    Ok(zone)
}

static FACTS: RwLock<Option<SystemFacts>> = RwLock::new(None);

/// Record the host facts. First call wins; subsequent calls are ignored.
pub fn set(facts: SystemFacts) {
    let mut held = FACTS.write();
    if held.is_none() {
        *held = Some(facts);
    }
}

/// The recorded facts, or a default snapshot if [`set`] was never called.
pub fn get() -> SystemFacts {
    FACTS.read().clone().unwrap_or_default()
}

/// Host facts determined since the rest were set, each `None` where it
/// has not moved.
#[derive(Debug, Default)]
pub struct Determined {
    pub ipv4_egress: Option<bool>,
    pub ipv6_egress: Option<bool>,
    pub timezone: Option<String>,
    pub available_threads: Option<i64>,
    pub available_memory: Option<i64>,
    pub node_name: Option<String>,
}

/// Record host facts that were undetermined when the rest were set and have
/// since been determined. A fact is only ever moved out of undetermined,
/// never changed once it holds.
// spec: CONST#determining-host-constants
pub fn determined(found: Determined) {
    let mut held = FACTS.write();
    let facts = held.get_or_insert_with(SystemFacts::default);
    if let Some(v4) = found.ipv4_egress {
        facts.ipv4_egress = v4;
    }
    if let Some(v6) = found.ipv6_egress {
        facts.ipv6_egress = v6;
    }
    if let Some(zone) = found.timezone {
        facts.timezone = zone;
    }
    if let Some(threads) = found.available_threads {
        facts.available_threads = Some(threads);
    }
    if let Some(memory) = found.available_memory {
        facts.available_memory = Ok(memory);
    }
    if let Some(name) = found.node_name {
        facts.node_name = Ok(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: CONST#host-constants
    #[test]
    fn mem_total_is_read_in_bytes_and_anything_else_is_undetermined() {
        assert_eq!(
            parse_mem_total_bytes("MemFree: 1 kB\nMemTotal:   2048 kB\n"),
            Ok(2048 * 1024)
        );
        assert!(parse_mem_total_bytes("MemFree: 1 kB\n").is_err());
        assert!(parse_mem_total_bytes("MemTotal: lots kB\n").is_err());
        assert!(parse_mem_total_bytes("MemTotal: 12 parsecs\n").is_err());
    }
}
