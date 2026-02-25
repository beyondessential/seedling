use std::collections::BTreeMap;

use super::action::ActionDef;

#[derive(Debug, Clone)]
#[expect(dead_code, reason = "not yet used")]
pub struct InstallDef {
    pub action: ActionDef,
    pub requirements: BTreeMap<String, InstallRequirementDef>,
}

#[derive(Debug, Clone)]
#[expect(dead_code, reason = "not yet used")]
pub struct InstallRequirementDef {
    pub kind: InstallRequirementKind,
    pub required: bool,
    pub default_value: String,
    pub description: String,
}

#[derive(Debug, Default, Clone, Copy)]
#[expect(dead_code, reason = "not yet used")]
pub enum InstallRequirementKind {
    #[default]
    Text,
    Email,
    Password,
    WeakPassword,
}
