//! Names Seedling grants itself inside namespaces operators also use.
//!
//! Site ingresses, external volume mappings and apps are operator-facing
//! namespaces, and the daemon takes names in each: `tailscale` for the ingress
//! the discovery provider maintains, `seedling-op-*` for the volumes bound to a
//! running operation, and the app names it keeps for its own infrastructure
//! and reporting. The ingress name is recognised later *by name*, and its
//! provider disables whatever row holds it. Nothing stopped an operator
//! creating an object with that name first.
//!
//! Two halves are needed and neither suffices alone. Reservation at creation
//! stops new collisions but cannot repair one that already exists, and does
//! not protect against a future code path that forgets to ask. So the
//! destructive consumers also match on *recorded ownership*: the Tailscale
//! provider acts only on rows whose source is its own discovery. Reservation
//! makes collisions impossible going forward; ownership checks make them
//! harmless regardless of history.

use seedling_protocol::names::{AppName, ExternalVolumeName, SiteIngressName};

/// External volume mapping name prefixes the daemon claims.
///
/// Operation-scoped volume bindings are named `seedling-op-<operation>-<key>`
/// and resolved before the mapping table, so a mapping an operator made under
/// one of these names would be shadowed, or would shadow a binding.
pub const RESERVED_EXTERNAL_VOLUME_PREFIXES: &[&str] = &["seedling-op-"];

/// Site-ingress names the daemon claims.
pub const RESERVED_SITE_INGRESS_NAMES: &[&str] =
    &[crate::runtime::tailscale::TAILSCALE_INGRESS_NAME];

/// App names the daemon claims.
///
/// An app's workloads are placed in a slice named after the app, so an app name
/// that realises the slice component the daemon keeps for its own
/// infrastructure would have its containers share — and reweight — the slice
/// holding the proxy and the resolver.
///
/// `seedling` is the app the runtime files faults about its own reporting to
/// other systems against.
pub const RESERVED_APP_NAMES: &[&str] = &[crate::system::priority::INFRA_COMPONENT, "seedling"];

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

/// Reject an external volume mapping name the daemon claims.
///
/// Creation only — never update or delete. An operator must still be able to
/// manage out of existence an object that predates the reservation.
// spec: IDN#reserved-names
pub fn check_external_volume_name(name: &ExternalVolumeName) -> Result<(), ReservedName> {
    for prefix in RESERVED_EXTERNAL_VOLUME_PREFIXES {
        if name.as_str().starts_with(prefix) {
            return Err(ReservedName {
                name: name.as_str().to_owned(),
                reason: format!(
                    "external volume names beginning {prefix:?} belong to the volumes bound \
                     to a running operation"
                ),
            });
        }
    }
    Ok(())
}

/// Reject a site-ingress name the daemon claims.
///
/// Creation only, for the same reason as above.
// spec: IDN#reserved-names
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
// spec: PRIO#resource-control-groups
// spec: IDN#reserved-names
pub fn check_app_name(name: &AppName) -> Result<(), ReservedName> {
    if RESERVED_APP_NAMES.contains(&name.as_str()) {
        return Err(ReservedName {
            name: name.as_str().to_owned(),
            reason: "this app name is one the daemon keeps for its own infrastructure and \
                     reporting"
                .to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: PRIO#resource-control-groups
    #[test]
    fn the_infra_slice_app_name_is_reserved() {
        let reserved = AppName::new(crate::system::priority::INFRA_COMPONENT).unwrap();
        assert!(check_app_name(&reserved).is_err());
        let ordinary = AppName::new("infra-tools").unwrap();
        assert!(check_app_name(&ordinary).is_ok());
    }

    // spec: IDN#reserved-names
    #[test]
    fn operation_scoped_volume_names_are_reserved_for_mappings() {
        let reserved = ExternalVolumeName::new("seedling-op-abcd1234-data").unwrap();
        assert!(check_external_volume_name(&reserved).is_err());
        let ordinary = ExternalVolumeName::new("seedling-data").unwrap();
        assert!(check_external_volume_name(&ordinary).is_ok());
    }

    // spec: IDN#reserved-names
    #[test]
    fn the_reporting_app_name_is_reserved() {
        let reserved = AppName::new("seedling").unwrap();
        assert!(check_app_name(&reserved).is_err());
    }

    // spec: IDN#reserved-names
    #[test]
    fn tailscale_ingress_name_is_reserved() {
        let reserved = SiteIngressName::new("tailscale").unwrap();
        assert!(check_site_ingress_name(&reserved).is_err());
        let ordinary = SiteIngressName::new("tailscale-manual").unwrap();
        assert!(check_site_ingress_name(&ordinary).is_ok());
    }
}
