use rhai::{CustomType, EvalAltResult, FnPtr, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::app::App;
use super::install::ParamKind;

// l[impl param.type]
#[derive(Debug, Clone)]
pub struct Param {
    pub name: ParamName,
    pub value: Option<String>,
    /// Back-reference to the owning App so that `on_change` can register the
    /// handler in `AppDef.param_changes` without needing a separate method on App.
    pub app: App,
}

impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Param");

        // l[impl param.is-set]
        builder.with_fn("is_set", |this: &mut Self| -> bool { this.value.is_some() });

        // l[impl param.value]
        builder.with_fn(
            "value",
            |this: &mut Self| -> Result<String, Box<EvalAltResult>> {
                this.value
                    .clone()
                    .ok_or_else(|| format!("param '{}' is not set", this.name).into())
            },
        );

        // l[impl param.on-change]
        // l[impl param.on-change.constraints]
        builder.with_fn(
            "on_change",
            |this: &mut Self, closure: FnPtr| -> Result<(), Box<EvalAltResult>> {
                if crate::runtime::barrier::runtime::is_in_action_closure() {
                    return Err(format!(
                        "on_change for parameter '{}' cannot be called from within an action closure",
                        this.name
                    )
                    .into());
                }
                {
                    let def = this.app.def.load();
                    if def.param_changes.contains(&this.name) {
                        return Err(format!(
                            "on_change already registered for parameter '{}'",
                            this.name
                        )
                        .into());
                    }
                }
                let name_clone = this.name.clone();
                this.app.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.param_changes.insert(name_clone.clone());
                    d
                });
                crate::defs::app::capture_param_change(this.name.clone(), closure);
                Ok(())
            },
        );

        // l[impl param.schema.kind]
        builder.with_fn(
            "kind",
            |this: &mut Self, kind_str: &str| -> Result<Self, Box<EvalAltResult>> {
                let kind = kind_str
                    .parse::<ParamKind>()
                    .map_err(|_| -> Box<EvalAltResult> {
                        format!(
                            "unknown param kind '{}' for parameter '{}'",
                            kind_str, this.name
                        )
                        .into()
                    })?;
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

        // l[impl param.schema.required]
        builder.with_fn("required", |this: &mut Self, required: bool| -> Self {
            let name_clone = this.name.clone();
            this.app.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_clone.clone())
                    .and_modify(|def| def.required = required);
                d
            });
            this.clone()
        });

        // l[impl param.schema.default-value]
        builder.with_fn("default_value", |this: &mut Self, value: &str| -> Self {
            let name_clone = this.name.clone();
            let value_owned = value.to_owned();
            this.app.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_clone.clone())
                    .and_modify(|def| def.default_value = Some(value_owned.clone()));
                d
            });
            this.clone()
        });

        // l[impl param.schema.description]
        builder.with_fn("description", |this: &mut Self, desc: &str| -> Self {
            let name_clone = this.name.clone();
            let desc_owned = desc.to_owned();
            this.app.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_clone.clone())
                    .and_modify(|def| def.description = Some(desc_owned.clone()));
                d
            });
            this.clone()
        });

        // l[impl param.schema.secret]
        builder.with_fn("secret", |this: &mut Self, secret: bool| -> Self {
            let name_clone = this.name.clone();
            this.app.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_clone.clone())
                    .and_modify(|def| def.secret = secret);
                d
            });
            this.clone()
        });
    }
}
