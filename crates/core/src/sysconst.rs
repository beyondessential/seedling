//! Host-level system facts surfaced to BSL as `const.*` constants.
//!
//! Values populated here back constants defined in `spec: CONST`.
//! Some values (IPv4/IPv6 egress, snapshot support, etc.) can only be
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
    /// The processor architecture, as Rust names it (e.g. `x86_64`).
    pub cpu_architecture: String,
    /// IANA timezone name of the host (e.g. `Pacific/Auckland`); `UTC` while
    /// the host's local timezone is undetermined.
    pub timezone: String,
    /// What an undetermined memory or node name does to an evaluation.
    pub undetermined: Undetermined,
}

/// What an undetermined host constant that has no stand-in value does to an
/// evaluation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Undetermined {
    /// The evaluation fails before the script runs, as the runtime has it.
    #[default]
    FailsEvaluation,
    /// The constant is left out of the scope, and the script fails where it
    /// reads it, as a definition check has it.
    FailsRead,
}

/// The host constants that have no stand-in value while undetermined.
pub const UNDETERMINABLE: [&str; 2] = ["AVAILABLE_MEMORY", "NODE_NAME"];

impl SystemFacts {
    /// Why the host constant `name` has no value, where it is one of
    /// [`UNDETERMINABLE`] and is undetermined.
    pub fn why_undetermined(&self, name: &str) -> Option<String> {
        let why = match name {
            "AVAILABLE_MEMORY" => self.available_memory.as_ref().err(),
            "NODE_NAME" => self.node_name.as_ref().err(),
            _ => None,
        }?;
        Some(format!("{name} cannot be determined: {why}"))
    }
}

impl Default for SystemFacts {
    /// Outside a running daemon the host's identity has no meaning, so the
    /// node name is empty; the compute figure is still the host's. Memory is
    /// read through a platform, which a non-daemon caller does not have, so
    /// it is undetermined.
    fn default() -> Self {
        Self {
            ipv4_egress: false,
            ipv6_egress: false,
            nat64_active: false,
            available_threads: detect_available_threads().ok(),
            available_memory: Err("the host's memory is read only by the daemon".to_owned()),
            has_snapshots: false,
            node_name: Ok(String::new()),
            cpu_architecture: std::env::consts::ARCH.to_owned(),
            timezone: "UTC".to_owned(),
            undetermined: Undetermined::FailsEvaluation,
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
    FACTS.read().clone().unwrap_or_else(unset)
}

/// What a caller that never recorded facts sees.
#[cfg(not(test))]
fn unset() -> SystemFacts {
    SystemFacts::default()
}

/// Core's own tests stand for a host whose memory is known, as the daemon's
/// is, so that the scripts they evaluate can read it.
#[cfg(test)]
fn unset() -> SystemFacts {
    SystemFacts {
        available_memory: Ok(1 << 30),
        ..SystemFacts::default()
    }
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
    let facts = held.get_or_insert_with(unset);
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
    fn outside_the_daemon_memory_is_undetermined() {
        assert!(SystemFacts::default().available_memory.is_err());
    }
}
