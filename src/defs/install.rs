use std::collections::BTreeMap;

use rhai::FnPtr;

// r[action.install]
#[derive(Debug, Clone)]
pub struct InstallDef {
    pub closure: FnPtr,
    pub requirements: BTreeMap<String, InstallRequirementDef>,
}

// r[action.install.requirements]
#[derive(Debug, Clone)]
pub struct InstallRequirementDef {
    pub kind: InstallRequirementKind,
    pub required: bool,
    pub default_value: Option<String>,
    pub description: Option<String>,
}

// r[action.install.requirements.kind-text]
// r[action.install.requirements.kind-email]
// r[action.install.requirements.kind-password]
// r[action.install.requirements.kind-weak-password]
#[derive(Debug, Default, Clone, Copy)]
pub enum InstallRequirementKind {
    #[default]
    Text,
    Email,
    Password,
    WeakPassword,
}

impl InstallRequirementKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "email" => Some(Self::Email),
            "password" => Some(Self::Password),
            "weak-password" => Some(Self::WeakPassword),
            _ => None,
        }
    }
}
