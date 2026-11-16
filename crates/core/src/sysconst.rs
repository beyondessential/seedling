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
    /// The node's volume storage supports copy-on-write snapshots.
    pub has_snapshots: bool,
    /// Human-readable identifier for the node (empty if not known).
    pub node_name: String,
    /// IANA timezone name of the host (e.g. `Pacific/Auckland`); `UTC` while
    /// the host's local timezone is undetermined.
    pub timezone: String,
}

impl Default for SystemFacts {
    fn default() -> Self {
        Self {
            ipv4_egress: false,
            ipv6_egress: false,
            nat64_active: false,
            has_snapshots: false,
            node_name: String::new(),
            timezone: "UTC".to_owned(),
        }
    }
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

/// Record host facts that were undetermined when the rest were set and have
/// since been determined. A fact is only ever moved out of undetermined,
/// never changed once it holds.
// spec: CONST#determining-host-constants
pub fn determined(ipv4_egress: Option<bool>, ipv6_egress: Option<bool>, timezone: Option<String>) {
    let mut held = FACTS.write();
    let facts = held.get_or_insert_with(SystemFacts::default);
    if let Some(v4) = ipv4_egress {
        facts.ipv4_egress = v4;
    }
    if let Some(v6) = ipv6_egress {
        facts.ipv6_egress = v6;
    }
    if let Some(zone) = timezone {
        facts.timezone = zone;
    }
}
