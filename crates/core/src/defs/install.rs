use std::collections::BTreeMap;

use seedling_protocol::names::ParamName;

// spec: ACN#install-action
#[derive(Debug, Clone)]
pub struct InstallDef {
    pub requirements: BTreeMap<ParamName, ParamDef>,
}

// spec: ACN#param-schemas
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub kind: ParamKind,
    pub required: bool,
    pub default_value: Option<String>,
    pub description: Option<String>,
    /// What the script declared for `secret`, or `None` when it said nothing,
    /// in which case the kind decides. An explicit `false` is a declaration, so
    /// it is not the same as unset: it is how a password kind opts out.
    // spec: PARAM#secret-parameters
    pub secret: Option<bool>,
}

impl ParamDef {
    // spec: PARAM#secret-parameters
    pub fn is_secret(&self) -> bool {
        self.secret.unwrap_or(matches!(
            self.kind,
            ParamKind::Password | ParamKind::WeakPassword
        ))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum ParamKind {
    // spec: ACN#param-kinds
    #[default]
    Text,
    // spec: ACN#param-kinds
    Multiline,
    // spec: ACN#param-kinds
    Email,
    // spec: ACN#param-kinds
    Password,
    // spec: ACN#param-kinds
    WeakPassword,
    // spec: ACN#param-kinds
    Random,
    /// A reference to a site volume; only valid in action and shell param
    /// schemas. The runtime resolves the operator-supplied volume reference
    /// to an operation-scoped binding before invoking the closure.
    // spec: ACN#volume-params
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
