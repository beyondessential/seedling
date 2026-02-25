use std::collections::BTreeMap;

use super::action::ActionDef;

#[derive(Debug, Clone)]
pub struct InstallDef {
    pub action: ActionDef,
    pub requirements: BTreeMap<String, InstallRequirementDef>,
}

#[derive(Debug, Clone)]
pub struct InstallRequirementDef {
    pub kind: InstallRequirementKind,
    pub required: bool,
    pub default_value: String,
    pub description: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum InstallRequirementKind {
    #[default]
    Text,
    Email,
    Password,
    WeakPassword,
}
