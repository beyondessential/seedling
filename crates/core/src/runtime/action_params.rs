//! Shared param-schema validation for action invocations.
//!
//! Both the operator-driven OI invocation path
//! (`oi/handler/actions.rs:invoke_action`) and the script-driven
//! [`Action.call`](crate::defs::action::Action) path apply the same
//! rules to action params: reject reserved keys, apply defaults,
//! enforce required fields, and resolve `kind: "volume"` references
//! against the live site-volume table. The two paths surface errors
//! through different channels (`OiError` vs `EvalAltResult`), so the
//! shared logic returns a tagged [`ParamValidationError`] that each
//! caller maps to its own error type.
//!
//! # Spec
//! - l[impl action.params]
//! - l[impl action.params.volume]
//! - r[impl operation.composition.params]
//! - r[impl operation.volume-param.reserved]

use std::collections::BTreeMap;

use seedling_protocol::names::{ParamName, SiteVolumeName};

use crate::defs::install::{ParamDef, ParamKind};

/// Validation outcome carrying enough structure for the caller to
/// either reject the call or surface a precise error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamValidationError {
    /// A param key uses one of the reserved suffixes (`_volume`,
    /// `_filename`).
    ReservedKey { key: String },
    /// One or more schema fields failed the requirements check
    /// (missing-required / invalid-email / weak-password / ...).
    Requirements { messages: Vec<String> },
    /// A `kind: "volume"` param's value was not a string.
    VolumeNotString { key: String },
    /// A required `kind: "volume"` param was absent.
    VolumeMissing { key: String },
    /// A `kind: "volume"` value did not parse as a valid site-volume
    /// name.
    VolumeInvalidName {
        key: String,
        value: String,
        reason: String,
    },
    /// A `kind: "volume"` value did not resolve to an existing site
    /// volume.
    VolumeNotFound { key: String, value: String },
}

impl std::fmt::Display for ParamValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReservedKey { key } => write!(
                f,
                "param key {key:?} is reserved (keys ending in _volume or _filename are reserved)"
            ),
            Self::Requirements { messages } => write!(f, "{}", messages.join("; ")),
            Self::VolumeNotString { key } => {
                write!(f, "param {key:?}: volume reference must be a string")
            }
            Self::VolumeMissing { key } => {
                write!(f, "param {key:?}: required volume reference is missing")
            }
            Self::VolumeInvalidName { key, value, reason } => write!(
                f,
                "param {key:?}: invalid site volume name {value:?}: {reason}"
            ),
            Self::VolumeNotFound { key, value } => {
                write!(f, "param {key:?}: site volume {value:?} not found")
            }
        }
    }
}

impl std::error::Error for ParamValidationError {}

// l[impl action.params]
// r[impl operation.volume-param.reserved]
/// Reject any param key ending in a reserved suffix.
pub fn reject_reserved_keys(
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), ParamValidationError> {
    for key in params.keys() {
        if key.ends_with("_volume") || key.ends_with("_filename") {
            return Err(ParamValidationError::ReservedKey { key: key.clone() });
        }
    }
    Ok(())
}

/// Run the same field-by-field requirements check the install path
/// uses (defaults, required-field enforcement, kind-specific
/// validation), and write the resulting filled-in values back into
/// `params`.
pub fn apply_schema(
    schema: &BTreeMap<ParamName, ParamDef>,
    params: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), ParamValidationError> {
    if schema.is_empty() {
        return Ok(());
    }

    let submitted: BTreeMap<String, String> = schema
        .keys()
        .filter_map(|k| {
            params
                .get(k.as_str())
                .and_then(|v| v.as_str())
                .map(|s| (k.as_str().to_owned(), s.to_owned()))
        })
        .collect();

    let filled = run_requirements(schema, &submitted)?;
    for (k, v) in filled {
        params.insert(k, serde_json::Value::String(v));
    }
    Ok(())
}

// l[impl action.params.volume]
/// Resolve every `kind: "volume"` param against `lookup`, ensuring
/// each declared volume name is present (when required) and points at
/// a real site volume.
pub fn validate_volume_params<L: VolumeLookup>(
    schema: &BTreeMap<ParamName, ParamDef>,
    params: &serde_json::Map<String, serde_json::Value>,
    lookup: &L,
) -> Result<(), ParamValidationError> {
    let volume_names: Vec<&ParamName> = schema
        .iter()
        .filter_map(|(name, def)| (def.kind == ParamKind::Volume).then_some(name))
        .collect();
    if volume_names.is_empty() {
        return Ok(());
    }

    for name in volume_names {
        let key = name.as_str();
        let val = match params.get(key) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(_) => {
                return Err(ParamValidationError::VolumeNotString {
                    key: key.to_owned(),
                });
            }
            None => {
                let def = schema.get(name).expect("schema entry");
                if def.required {
                    return Err(ParamValidationError::VolumeMissing {
                        key: key.to_owned(),
                    });
                }
                continue;
            }
        };
        let site_name =
            SiteVolumeName::new(&val).map_err(|e| ParamValidationError::VolumeInvalidName {
                key: key.to_owned(),
                value: val.clone(),
                reason: e.to_string(),
            })?;
        if !lookup.site_volume_exists(&site_name) {
            return Err(ParamValidationError::VolumeNotFound {
                key: key.to_owned(),
                value: val,
            });
        }
    }
    Ok(())
}

/// Adapter trait so the validation can run against either the live
/// OI state or a test double without dragging the whole [`OiState`]
/// into the runtime layer.
pub trait VolumeLookup {
    fn site_volume_exists(&self, name: &SiteVolumeName) -> bool;
}

// r[impl operation.composition.params]
/// Run the schema check used by [`apply_schema`]. Extracted so the
/// existing OI install path (which validates a `BTreeMap<String,
/// String>` directly without going through a `serde_json::Map`) can
/// reuse the same field-level logic.
pub fn run_requirements(
    schema: &BTreeMap<ParamName, ParamDef>,
    submitted: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ParamValidationError> {
    let mut filled = submitted.clone();
    let mut errors: Vec<String> = Vec::new();

    for (field, req_def) in schema {
        let field_str = field.as_str();
        let raw = filled.get(field_str).map(|s| s.as_str()).unwrap_or("");

        if raw.is_empty() {
            if let Some(default) = &req_def.default_value {
                filled.insert(field_str.to_owned(), default.clone());
            } else if req_def.required {
                errors.push(format!("{field_str}: required field is missing"));
                continue;
            } else {
                continue;
            }
        }

        let value = filled.get(field_str).map(|s| s.as_str()).unwrap_or("");
        match req_def.kind {
            ParamKind::Email => {
                if !is_valid_email(value) {
                    errors.push(format!("{field_str}: invalid email address"));
                }
            }
            ParamKind::Password => {
                if !is_strong_password(value) {
                    errors.push(format!("{field_str}: password is too weak"));
                }
            }
            ParamKind::Text
            | ParamKind::Multiline
            | ParamKind::WeakPassword
            | ParamKind::Random => {}
            ParamKind::Volume => {}
        }
    }

    if errors.is_empty() {
        Ok(filled)
    } else {
        Err(ParamValidationError::Requirements { messages: errors })
    }
}

// i[impl action.invoke.install.validation]
pub(crate) fn is_valid_email(email: &str) -> bool {
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

// i[impl action.invoke.install.validation]
pub(crate) fn is_strong_password(password: &str) -> bool {
    zxcvbn::zxcvbn(password, &[]).score() >= zxcvbn::Score::Three
}
