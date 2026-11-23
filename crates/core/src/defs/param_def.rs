use seedling_protocol::names::SiteVolumeName;
use strum::VariantArray;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

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

#[derive(Debug, Default, Clone, Copy, PartialEq, VariantArray)]
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
    /// Whether this kind may appear in static param schemas (`app.param`).
    /// `Volume` is rejected there because its bindings are operation-scoped —
    /// the only sensible static equivalent is a declared external_volume
    /// mapping.
    pub fn allowed_static(self) -> bool {
        !matches!(self, Self::Volume)
    }

    /// Parse `value` as a value of this kind, or give the reason it is not
    /// one. This is the whole of the kind's check on a value, both for the
    /// runtime and for the kind's `std` helper.
    // spec: ACN#kind-parsers
    pub fn parse(self, value: &str) -> Result<ParsedValue, String> {
        if self.is_single_line() {
            reject_control_characters(value)?;
        }
        match self {
            Self::Text | Self::Multiline | Self::WeakPassword | Self::Random => {}
            // spec: ACN#kind-parsers
            Self::Email => {
                let Some((local, domain)) = split_email(value) else {
                    return Err("invalid email address".to_owned());
                };
                return Ok(ParsedValue::Email {
                    local: local.to_owned(),
                    domain: domain.to_owned(),
                });
            }
            // spec: ACN#kind-parsers
            Self::Password => {
                if !is_strong_password(value) {
                    return Err("password is too weak".to_owned());
                }
            }
            // spec: ACN#kind-parsers
            Self::Volume => {
                SiteVolumeName::new(value).map_err(|e| e.to_string())?;
            }
        }
        Ok(ParsedValue::Value(value.to_owned()))
    }

    // spec: ACN#kind-parsers
    fn is_single_line(self) -> bool {
        !matches!(self, Self::Multiline)
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

/// What a kind's parser makes of a value it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedValue {
    /// The value itself, for a kind with no parts.
    Value(String),
    Email {
        local: String,
        domain: String,
    },
}

// spec: ACN#kind-parsers
fn reject_control_characters(value: &str) -> Result<(), String> {
    match value.chars().find(|c| {
        matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
    }) {
        Some(c) => Err(format!(
            "contains U+{:04X}, which a single-line value cannot hold",
            u32::from(c)
        )),
        None => Ok(()),
    }
}

/// The local part and domain of an email address, split at the first `@`.
// spec: ACN#kind-parsers
fn split_email(email: &str) -> Option<(&str, &str)> {
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.');
    valid.then_some((local, domain))
}

// spec: ACN#kind-parsers
fn is_strong_password(password: &str) -> bool {
    zxcvbn::zxcvbn(password, &[]).score() >= zxcvbn::Score::Three
}
