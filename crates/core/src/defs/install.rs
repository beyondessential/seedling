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
    // l[impl action.install.requirements.kind-multiline]
    Multiline,
    // l[impl action.install.requirements.kind-email]
    Email,
    // l[impl action.install.requirements.kind-password]
    Password,
    // l[impl action.install.requirements.kind-weak-password]
    WeakPassword,
    // l[impl action.install.requirements.kind-random]
    Random,
    /// A reference to a site volume; only valid in action and shell param
    /// schemas. The runtime resolves the operator-supplied volume reference
    /// to an operation-scoped binding before invoking the closure.
    // l[impl action.params.volume]
    Volume,
}

impl ParamKind {
    /// Whether this kind may appear in static param schemas (`app.param`,
    /// `app.on_install`'s requirements). `Volume` is rejected there because
    /// its bindings are operation-scoped — the only sensible static
    /// equivalent is a declared external_volume mapping.
    pub fn allowed_static(self) -> bool {
        !matches!(self, Self::Volume)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Multiline => "multiline",
            Self::Email => "email",
            Self::Password => "password",
            Self::WeakPassword => "weak-password",
            Self::Random => "random",
            Self::Volume => "volume",
        }
    }
}

impl std::str::FromStr for ParamKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            "multiline" => Ok(Self::Multiline),
            "email" => Ok(Self::Email),
            "password" => Ok(Self::Password),
            "weak-password" => Ok(Self::WeakPassword),
            "random" => Ok(Self::Random),
            "volume" => Ok(Self::Volume),
            _ => Err(()),
        }
    }
}
