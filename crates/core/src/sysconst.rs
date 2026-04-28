//! Host-level system facts surfaced to BSL as `const.*` constants.
//!
//! Values populated here back constants defined in `docs/spec/language.md`.
//! Some values (IPv4/IPv6 egress, BTRFS availability, etc.) can only be
//! determined asynchronously or from runtime daemon state, so they are
//! cached here once at daemon startup and read synchronously from
//! `defs::scope()` on every script execution.

use std::sync::OnceLock;

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
    /// IANA timezone name of the host (e.g. `Pacific/Auckland`); defaults to
    /// `UTC` when the host's local timezone cannot be determined.
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

/// Best-effort detection of the host's IANA timezone name. Returns `UTC`
/// when detection fails (e.g. unusual `/etc/localtime` configuration).
pub fn detect_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned())
}

static FACTS: OnceLock<SystemFacts> = OnceLock::new();

/// Record the host facts. First call wins; subsequent calls are ignored.
pub fn set(facts: SystemFacts) {
    let _ = FACTS.set(facts);
}

/// Borrow the recorded facts, or a default snapshot if [`set`] was never
/// called.
pub fn get() -> &'static SystemFacts {
    static DEFAULT: OnceLock<SystemFacts> = OnceLock::new();
    FACTS
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(SystemFacts::default))
}
