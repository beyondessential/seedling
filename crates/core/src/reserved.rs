//! Names Seedling grants itself inside namespaces operators also use.
//!
//! Site ingresses and apps are operator-facing namespaces, and the daemon takes
//! names in both: `tailscale` for the ingress the discovery provider
//! maintains, and the slice component the daemon keeps for its own
//! infrastructure. Both are then recognised later *by name*, and the provider
//! disables whatever row holds the ingress name. Nothing stopped an operator
//! creating an object with either name first.
//!
//! Two halves are needed and neither suffices alone. Reservation at creation
//! stops new collisions but cannot repair one that already exists, and does
//! not protect against a future code path that forgets to ask. So the
//! destructive consumers also match on *recorded ownership*: the Tailscale
//! provider acts only on rows whose source is its own discovery. Reservation
//! makes collisions impossible going forward; ownership checks make them
//! harmless regardless of history.

use seedling_protocol::names::{AppName, SiteIngressName};

/// Site-ingress names the daemon claims.
pub const RESERVED_SITE_INGRESS_NAMES: &[&str] =
    &[crate::runtime::tailscale::TAILSCALE_INGRESS_NAME];

/// App names the daemon claims.
///
/// An app's workloads are placed in a slice named after the app, so an app name
/// that realises the slice component the daemon keeps for its own
/// infrastructure would have its containers share — and reweight — the slice
/// holding the proxy and the resolver.
pub const RESERVED_APP_NAMES: &[&str] = &[crate::system::priority::INFRA_COMPONENT];

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

/// Reject a site-ingress name the daemon claims.
///
/// Creation only — never update or delete. An operator whose ingress predates
/// the reservation must still be able to manage it out of existence.
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

/// Reject an app name the daemon claims.
///
/// Creation only, for the same reason as above: an operator whose app predates
/// the reservation must still be able to manage it out of existence.
// r[impl priority.groups-owned]
// r[impl namespace.reserved]
pub fn check_app_name(name: &AppName) -> Result<(), ReservedName> {
    if RESERVED_APP_NAMES.contains(&name.as_str()) {
        return Err(ReservedName {
            name: name.as_str().to_owned(),
            reason: "this app name would collide with the resource-control group the daemon \
                     keeps for its own infrastructure"
                .to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify priority.groups-owned]
    #[test]
    fn the_infra_slice_app_name_is_reserved() {
        let reserved = AppName::new(crate::system::priority::INFRA_COMPONENT).unwrap();
        assert!(check_app_name(&reserved).is_err());
        let ordinary = AppName::new("infra-tools").unwrap();
        assert!(check_app_name(&ordinary).is_ok());
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
