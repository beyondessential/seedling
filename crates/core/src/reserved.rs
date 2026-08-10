//! Names Seedling grants itself inside namespaces operators also use.
//!
//! Site volumes and site ingresses are operator-facing namespaces, and the
//! daemon takes names in both: `backup-snap-*` for the transient snapshots a
//! backup run creates, and `tailscale` for the ingress the discovery provider
//! maintains. Both are then recognised later *by name*, and both have a
//! destructive consumer — startup deletes everything under the snapshot
//! prefix, and the provider disables whatever row holds the ingress name.
//! Nothing stopped an operator creating an object with either name first.
//!
//! Two halves are needed and neither suffices alone. Reservation at creation
//! stops new collisions but cannot repair one that already exists, and does
//! not protect against a future code path that forgets to ask. So the
//! destructive consumers also match on *recorded ownership*: the startup
//! sweep skips names present in `site_volumes`, and the Tailscale provider
//! acts only on rows whose source is its own discovery. Reservation makes
//! collisions impossible going forward; ownership checks make them harmless
//! regardless of history.
//!
//! The constants lived apart before — `backup_execution.rs` and
//! `tailscale.rs` knew nothing about each other or about the creation
//! handlers — which is exactly how the gap opened.

use seedling_protocol::names::{SiteIngressName, SiteVolumeName};

/// Site-volume name prefixes the daemon claims.
///
/// Startup deletes every site volume under these, so an operator volume that
/// happened to match was destroyed on the next daemon start.
pub const RESERVED_SITE_VOLUME_PREFIXES: &[&str] =
    &[crate::runtime::backup_execution::SNAPSHOT_NAME_PREFIX];

/// Site-ingress names the daemon claims.
pub const RESERVED_SITE_INGRESS_NAMES: &[&str] =
    &[crate::runtime::tailscale::TAILSCALE_INGRESS_NAME];

/// A name that belongs to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedName {
    pub name: String,
    pub reason: String,
}

impl std::fmt::Display for ReservedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is reserved: {}", self.name, self.reason)
    }
}

/// Reject a site-volume name the daemon claims.
///
/// Creation only — never update or delete. An operator with a legacy
/// `backup-snap-*` volume must still be able to manage it out of existence.
// r[impl namespace.reserved]
pub fn check_site_volume_name(name: &SiteVolumeName) -> Result<(), ReservedName> {
    for prefix in RESERVED_SITE_VOLUME_PREFIXES {
        if name.as_str().starts_with(prefix) {
            return Err(ReservedName {
                name: name.as_str().to_owned(),
                reason: format!(
                    "site volume names beginning {prefix:?} are used by backup runs and are \
                     deleted at daemon startup"
                ),
            });
        }
    }
    Ok(())
}

/// Reject a site-ingress name the daemon claims.
///
/// Creation only, for the same reason as above.
// r[impl namespace.reserved]
pub fn check_site_ingress_name(name: &SiteIngressName) -> Result<(), ReservedName> {
    if RESERVED_SITE_INGRESS_NAMES.contains(&name.as_str()) {
        return Err(ReservedName {
            name: name.as_str().to_owned(),
            reason: "this site ingress name is maintained by a discovery provider".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify namespace.reserved]
    #[test]
    fn snapshot_prefix_is_reserved() {
        let reserved = SiteVolumeName::new("backup-snap-archive").unwrap();
        assert!(check_site_volume_name(&reserved).is_err());
        let ordinary = SiteVolumeName::new("archive").unwrap();
        assert!(check_site_volume_name(&ordinary).is_ok());
        // A name that merely contains the prefix is fine — only the claim on
        // the front of the namespace is the daemon's.
        let contains = SiteVolumeName::new("my-backup-snap-thing").unwrap();
        assert!(check_site_volume_name(&contains).is_ok());
    }

    // r[verify namespace.reserved]
    #[test]
    fn tailscale_ingress_name_is_reserved() {
        let reserved = SiteIngressName::new("tailscale").unwrap();
        assert!(check_site_ingress_name(&reserved).is_err());
        let ordinary = SiteIngressName::new("tailscale-manual").unwrap();
        assert!(check_site_ingress_name(&ordinary).is_ok());
    }
}
