use rhai::{CustomType, EvalAltResult, FnPtr, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::app::{App, ensure_not_validating};
use super::param_def::ParamKind;
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

impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Param");

        // spec: PARAM#values
        builder.with_fn("is_set", |this: &mut Self| -> bool { this.value.is_some() });

        // spec: PARAM#values
        builder.with_fn(
            "value",
            |this: &mut Self| -> Result<String, Box<EvalAltResult>> {
                if let Some(v) = &this.value {
                    // spec: SEC#redaction
                    if !crate::runtime::barrier::runtime::is_in_action_closure() {
                        crate::runtime::seeds::record_param_value(this.name.as_str(), v);
                    }
                    return Ok(v.clone());
                }
                if let Some(default) = this
                    .app
                    .def
                    .load()
                    .params
                    .get(&this.name)
                    .and_then(|def| def.default_value.clone())
                {
                    return Ok(default);
                }
                Err(format!("param '{}' is not set", this.name).into())
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
            |this: &mut Self, kind_str: &str| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("kind")?;
                let kind = kind_str
                    .parse::<ParamKind>()
                    .map_err(|_| -> Box<EvalAltResult> {
                        format!(
                            "unknown param kind '{}' for parameter '{}'",
                            kind_str, this.name
                        )
                        .into()
                    })?;
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
                        .and_modify(|def| def.kind = kind);
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
            |this: &mut Self, value: &str| -> Result<Self, Box<EvalAltResult>> {
                ensure_not_validating()?;
                crate::defs::app::ensure_not_in_action_closure("default_value")?;
                let name_clone = this.name.clone();
                let value_owned = value.to_owned();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.params
                        .entry(name_clone.clone())
                        .and_modify(|def| def.default_value = Some(value_owned.clone()));
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
