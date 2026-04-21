use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::defs::resource::ResourceKind;

/// A stable, opaque handle to a single resource instance.
///
/// Internally a randomly-generated 128-bit value; `Copy` because it is just
/// 16 bytes and never needs heap allocation.
// r[impl identity.stable]
// r[impl identity.components]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstanceId(pub Uuid);

impl InstanceId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    /// First 8 lowercase hex characters — used as the human-readable suffix
    /// in the display name of scaled instances.
    pub fn display_suffix(&self) -> String {
        format!("{:08x}", (self.0.as_u128() >> 96) as u32)
    }

    /// 32-character lowercase hex — the canonical storage key in SQLite.
    pub fn to_hex(self) -> String {
        format!("{:032x}", self.0.as_u128())
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        u128::from_str_radix(s, 16)
            .ok()
            .map(|n| Self(Uuid::from_u128(n)))
    }
}

/// Whether this instance is the sole instance of its resource, or one of many
/// concurrent instances of the same scaled resource.
// r[impl identity.scaled]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstanceVariant {
    /// At most one instance of this resource exists at any time.
    Singleton,
    /// One of potentially many concurrent instances of the same resource.
    Scaled,
}

/// The stable, complete identity of one concrete resource instance.
///
/// The `id` field is the true primary key; all other fields are queryable
/// metadata.  `display_name` is derived at creation time and stored in the
/// instance registry so it never changes even if the derivation logic does.
// r[impl identity.stable]
// r[impl identity.components]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceInstance {
    pub id: InstanceId,
    pub app: String,
    pub kind: ResourceKind,
    /// The BSL-level resource name.  `None` for anonymous resources.
    pub name: Option<String>,
    pub variant: InstanceVariant,
    /// The human-readable name used in external systems (podman, interfaces,
    /// DNS).  Derived from the identity at creation time and stored stably.
    pub display_name: String,
}

impl ResourceInstance {
    // r[impl identity.components]
    // r[impl identity.job]
    pub fn new_singleton(
        app: impl Into<String>,
        kind: ResourceKind,
        name: impl Into<String>,
    ) -> Self {
        let app = app.into();
        let name = name.into();
        // Jobs use a fixed all-zero instance ID so that their identity is
        // fully deterministic without persisting state.  Their display name
        // includes the ID suffix (always "00000000") to match the format used
        // by operation-derived and shell instances of the same Job definition,
        // and to avoid clashing with Deployment display names.
        //
        // Deployments keep the flat "{app}-{name}" form because they are
        // managed as singletons whose display name must be stable across
        // upgrades.  All other resource kinds include the kind slug.
        let (id, display_name) = match kind {
            ResourceKind::Job => {
                let id = InstanceId(uuid::Uuid::nil());
                let dn = format!("{}-{}-{}", app, name, id.display_suffix());
                (id, dn)
            }
            ResourceKind::Deployment => {
                let id = InstanceId::generate();
                (id, format!("{}-{}", app, name))
            }
            _ => {
                let id = InstanceId::generate();
                (id, format!("{}-{}-{}", app, kind_slug(kind), name))
            }
        };
        Self {
            id,
            app,
            kind,
            name: Some(name),
            variant: InstanceVariant::Singleton,
            display_name,
        }
    }

    // r[impl identity.scaled]
    pub fn new_scaled(app: impl Into<String>, kind: ResourceKind, name: impl Into<String>) -> Self {
        let id = InstanceId::generate();
        let app = app.into();
        let name = name.into();
        let display_name = format!("{}-{}-{}", app, name, id.display_suffix());
        Self {
            id,
            app,
            kind,
            name: Some(name),
            variant: InstanceVariant::Scaled,
            display_name,
        }
    }

    // r[impl identity.anonymous]
    pub fn new_anonymous(app: impl Into<String>, kind: ResourceKind) -> Self {
        let id = InstanceId::generate();
        let app = app.into();
        let display_name = format!("{}-{}", app, kind_slug(kind));
        Self {
            id,
            app,
            kind,
            name: None,
            variant: InstanceVariant::Singleton,
            display_name,
        }
    }
}

/// The canonical on-disk / podman name of a named app volume.
///
/// All consumers must address a named volume through this newtype so the
/// exact string format has a single point of truth. Direct construction
/// is deliberately unavailable; use [`VolumeName::for_app`] when you
/// know the app and BSL volume name, or [`VolumeName::of_instance`] when
/// you already hold the `ResourceInstance` the registry stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumeName(String);

impl VolumeName {
    /// The canonical form: `<app>-volume-<name>`, matching the
    /// `display_name` that `ResourceInstance::new_singleton` produces for a
    /// `ResourceKind::Volume`.
    pub fn for_app(app: &str, name: &str) -> Self {
        Self(format!(
            "{}-{}-{}",
            app,
            kind_slug(ResourceKind::Volume),
            name
        ))
    }

    /// The canonical form of the given Volume resource instance. The caller
    /// is responsible for ensuring `instance.kind == ResourceKind::Volume`.
    pub fn of_instance(instance: &ResourceInstance) -> Self {
        debug_assert_eq!(instance.kind, ResourceKind::Volume);
        Self(instance.display_name.clone())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VolumeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn kind_slug(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Parameter => "parameter",
        ResourceKind::Service => "service",
        ResourceKind::HttpService => "http-service",
        ResourceKind::Ingress => "ingress",
        ResourceKind::Deployment => "deployment",
        ResourceKind::Job => "job",
        ResourceKind::Volume => "volume",
        ResourceKind::ExternalVolume => "ext-volume",
        ResourceKind::Action => "action",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;

    // r[verify identity.stable]
    #[test]
    fn clone_equals_original() {
        let a = ResourceInstance::new_singleton("app", ResourceKind::Deployment, "web");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.id, b.id);
    }

    // r[verify identity.components]
    #[test]
    fn separately_constructed_instances_differ() {
        let a = ResourceInstance::new_singleton("app", ResourceKind::Deployment, "web");
        let b = ResourceInstance::new_singleton("app", ResourceKind::Deployment, "web");
        assert_ne!(a.id, b.id);
        assert_ne!(a, b);
    }

    // r[verify identity.scaled]
    #[test]
    fn scaled_display_name_includes_suffix() {
        let a = ResourceInstance::new_scaled("myapp", ResourceKind::Deployment, "web");
        assert!(
            a.display_name.starts_with("myapp-web-"),
            "display_name was: {}",
            a.display_name
        );
        // suffix is 8 hex chars
        let suffix = a.display_name.strip_prefix("myapp-web-").unwrap();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // r[verify identity.scaled]
    #[test]
    fn two_scaled_instances_have_different_ids_and_display_names() {
        let a = ResourceInstance::new_scaled("myapp", ResourceKind::Deployment, "web");
        let b = ResourceInstance::new_scaled("myapp", ResourceKind::Deployment, "web");
        assert_ne!(a.id, b.id);
        assert_ne!(a.display_name, b.display_name);
    }

    // r[verify identity.components]
    #[test]
    fn singleton_display_name_has_no_suffix() {
        let a = ResourceInstance::new_singleton("myapp", ResourceKind::Deployment, "web");
        assert_eq!(a.display_name, "myapp-web");
    }

    // r[verify identity.anonymous]
    #[test]
    fn anonymous_has_no_name() {
        let a = ResourceInstance::new_anonymous("myapp", ResourceKind::Deployment);
        assert!(a.name.is_none());
        assert_eq!(a.display_name, "myapp-deployment");
    }

    // r[verify identity.stable]
    #[test]
    fn serde_roundtrip_preserves_id() {
        let r = ResourceInstance::new_singleton("app", ResourceKind::Deployment, "web");
        let json = serde_json::to_string(&r).unwrap();
        let r2: ResourceInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(r, r2);
        assert_eq!(r.id, r2.id);
    }

    #[test]
    fn instance_id_hex_roundtrip() {
        let id = InstanceId::generate();
        let hex = id.to_hex();
        assert_eq!(hex.len(), 32);
        let id2 = InstanceId::from_hex(&hex).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn display_suffix_is_8_chars() {
        let id = InstanceId::generate();
        let suffix = id.display_suffix();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn instance_id_is_copy() {
        let id = InstanceId::generate();
        let id2 = id; // copy
        assert_eq!(id, id2);
    }
}
