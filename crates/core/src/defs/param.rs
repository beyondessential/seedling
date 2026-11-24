use rhai::{CustomType, Dynamic, EvalAltResult, FnPtr, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::app::{App, ensure_not_validating};
use super::param_def::{SuppliedValue, take_kind};
use super::take::take_closure;

// spec: PARAM#declaration
#[derive(Debug, Clone)]
pub struct Param {
    pub name: ParamName,
    pub value: Option<String>,
    /// Back-reference to the owning App, which supplies the declared default
    /// and receives schema and validator registrations.
    pub app: App,
}

/// Check each declared default against its parameter's kind, which the script
/// may have declared before or after the default.
// spec: PARAM#kind-checks
pub(super) fn check_defaults(def: &super::app::AppDef) -> Result<(), String> {
    for (name, param) in &def.params {
        param
            .check_default()
            .map_err(|e| format!("the default of parameter '{name}' is rejected: {e}"))?;
    }
    Ok(())
}

impl Param {
    /// The stored value, or the declared default without one.
    // spec: PARAM#values
    fn effective_value(&self) -> Result<String, Box<EvalAltResult>> {
        if let Some(v) = &self.value {
            // spec: SEC#redaction
            if !crate::runtime::barrier::runtime::is_in_action_closure() {
                crate::runtime::seeds::record_param_value(self.name.as_str(), v);
            }
            return Ok(v.clone());
        }
        if let Some(default) = self
            .app
            .def
            .load()
            .params
            .get(&self.name)
            .and_then(|def| def.default_text())
        {
            return Ok(default);
        }
        Err(format!("param '{}' is not set", self.name).into())
    }
}

impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Param");

        // spec: PARAM#values
        builder.with_fn("is_set", |this: &mut Self| -> bool { this.value.is_some() });

        // spec: PARAM#values
        builder.with_fn(
            "value",
            |this: &mut Self| -> Result<String, Box<EvalAltResult>> { this.effective_value() },
        );

        // spec: PARAM#values
        builder.with_fn(
            "parsed",
            |this: &mut Self| -> Result<Dynamic, Box<EvalAltResult>> {
                let value = this.effective_value()?;
                let kind = this
                    .app
                    .def
                    .load()
                    .params
                    .get(&this.name)
                    .map(|def| def.kind.clone())
                    .unwrap_or_default();
                let parsed = kind.parse(&value).map_err(|e| -> Box<EvalAltResult> {
                    format!("param '{}': {e}", this.name).into()
                })?;
                Ok(parsed.into_dynamic())
            },
        );

        // spec: PARAM#validators
        builder.with_fn(
            "validate",
            |this: &mut Self, closure: FnPtr| -> Result<Self, Box<EvalAltResult>> {
                // spec: BSL#arguments
                let closure = take_closure("validate", "fn", closure)?;
                ensure_not_validating()?;
                if crate::runtime::barrier::runtime::is_in_action_closure() {
                    return Err(format!(
                        "validate for parameter '{}' cannot be called from within an action closure",
                        this.name
                    )
                    .into());
                }
                if this.app.def.load().validators.contains(&this.name) {
                    return Err(format!(
                        "a validator is already attached to parameter '{}'",
                        this.name
                    )
                    .into());
                }
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.validators.insert(name_clone.clone());
                    d
                });
                crate::defs::app::capture_validator(this.name.clone(), closure);
                Ok(this.clone())
            },
        );

        // spec: PARAM#schema
        builder.with_fn(
            "kind",
            |this: &mut Self, kind: Dynamic| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("kind")?;
                let kind = take_kind(&format!("kind of parameter '{}'", this.name), kind)?;
                if !kind.allowed_static() {
                    return Err(format!(
                        "param kind '{}' is not allowed on the static parameter '{}'; \
                         use it as an action or shell param instead, or declare an \
                         external_volume for a permanent mapping",
                        kind.as_str(),
                        this.name
                    )
                    .into());
                }
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.kind = kind.clone());
                    d
                });
                Ok(this.clone())
            },
        );

        // spec: PARAM#schema
        builder.with_fn(
            "required",
            |this: &mut Self, required: bool| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("required")?;
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.required = required);
                    d
                });
                Ok(this.clone())
            },
        );

        // spec: PARAM#schema
        builder.with_fn(
            "default_value",
            |this: &mut Self, value: Dynamic| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("default_value")?;
                let value = SuppliedValue::take("default_value", value)?;
                let default = (!value.is_empty()).then_some(value);
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.default_value = default.clone());
                    d
                });
                Ok(this.clone())
            },
        );

        // spec: PARAM#schema
        builder.with_fn(
            "description",
            |this: &mut Self, desc: &str| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("description")?;
                let name_clone = this.name.clone();
                let desc_owned = desc.to_owned();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.description = Some(desc_owned.clone()));
                    d
                });
                Ok(this.clone())
            },
        );

        // spec: PARAM#secret-parameters
        builder.with_fn(
            "secret",
            |this: &mut Self, secret: bool| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("secret")?;
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.secret = Some(secret));
                    d
                });
                Ok(this.clone())
            },
        );
    }
}
