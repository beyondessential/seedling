use crate::defs::resource::ResourceKind;
use serde::{Deserialize, Serialize};

/// A stable identity for one concrete instance of a BSL resource.
// r[impl identity.stable]
// r[impl identity.components]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceInstance {
    pub app: String,
    pub kind: ResourceKind,
    pub name: Option<String>,
    pub ordinal: u32,
}

impl ResourceInstance {
    pub fn named(app: impl Into<String>, kind: ResourceKind, name: impl Into<String>) -> Self {
        Self {
            app: app.into(),
            kind,
            name: Some(name.into()),
            ordinal: 0,
        }
    }

    // r[impl identity.scaled]
    pub fn scaled(
        app: impl Into<String>,
        kind: ResourceKind,
        name: impl Into<String>,
        ordinal: u32,
    ) -> Self {
        Self {
            app: app.into(),
            kind,
            name: Some(name.into()),
            ordinal,
        }
    }

    // r[impl identity.anonymous]
    pub fn anonymous(app: impl Into<String>, kind: ResourceKind) -> Self {
        Self {
            app: app.into(),
            kind,
            name: None,
            ordinal: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;

    // r[verify identity.components]
    #[test]
    fn different_names_not_equal() {
        let a = ResourceInstance::named("app", ResourceKind::Deployment, "a");
        let b = ResourceInstance::named("app", ResourceKind::Deployment, "b");
        assert_ne!(a, b);
    }

    // r[verify identity.stable]
    // r[verify identity.components]
    #[test]
    fn same_components_equal() {
        let a = ResourceInstance::named("app", ResourceKind::Deployment, "web");
        let b = ResourceInstance::named("app", ResourceKind::Deployment, "web");
        assert_eq!(a, b);
    }

    // r[verify identity.scaled]
    #[test]
    fn different_ordinals_not_equal() {
        let a = ResourceInstance::scaled("app", ResourceKind::Deployment, "web", 0);
        let b = ResourceInstance::scaled("app", ResourceKind::Deployment, "web", 1);
        assert_ne!(a, b);
    }

    // r[verify identity.anonymous]
    #[test]
    fn anonymous_has_no_name() {
        let a = ResourceInstance::anonymous("app", ResourceKind::Deployment);
        assert!(a.name.is_none());
    }

    // r[verify identity.stable]
    #[test]
    fn serde_roundtrip() {
        let r = ResourceInstance::named("app", ResourceKind::Deployment, "web");
        let json = serde_json::to_string(&r).unwrap();
        let r2: ResourceInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(r, r2);
    }
}
