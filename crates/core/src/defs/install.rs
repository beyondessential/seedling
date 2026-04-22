use std::collections::BTreeMap;

use seedling_protocol::names::ParamName;

// l[impl action.install]
#[derive(Debug, Clone)]
pub struct InstallDef {
    pub requirements: BTreeMap<ParamName, ParamDef>,
}

// l[impl action.install.requirements]
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub kind: ParamKind,
    pub required: bool,
    pub default_value: Option<String>,
    pub description: Option<String>,
    // l[impl param.schema.secret]
    pub secret: bool,
}

impl ParamDef {
    // l[impl param.schema.secret-from-kind]
    pub fn is_secret(&self) -> bool {
        self.secret || matches!(self.kind, ParamKind::Password | ParamKind::WeakPassword)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum ParamKind {
    // l[impl action.install.requirements.kind-text]
    #[default]
    Text,
    // l[impl action.install.requirements.kind-email]
    Email,
    // l[impl action.install.requirements.kind-password]
    Password,
    // l[impl action.install.requirements.kind-weak-password]
    WeakPassword,
}

impl std::str::FromStr for ParamKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "email" => Ok(Self::Email),
            "password" => Ok(Self::Password),
            "weak-password" => Ok(Self::WeakPassword),
            _ => Err(()),
        }
    }
}
