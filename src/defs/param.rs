use rhai::{CustomType, EvalAltResult, FnPtr, TypeBuilder};

use super::app::App;

// l[impl param.type]
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub value: String,
    /// Back-reference to the owning App so that `on_change` can register the
    /// handler in `AppDef.param_changes` without needing a separate method on App.
    pub app: App,
}

impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Param")
            // l[impl param.value]
            .with_fn("to_string", |this: &mut Self| -> String {
                this.value.clone()
            })
            .with_fn("to_debug", |this: &mut Self| -> String {
                format!("Param({:?}, {:?})", this.name, this.value)
            })
            // l[impl param.on-change]
            .with_fn(
                "on_change",
                |this: &mut Self, closure: FnPtr| -> Result<(), Box<EvalAltResult>> {
                    if crate::runtime::barrier::runtime::is_in_action_closure() {
                        return Err(format!(
                            "on_change for parameter '{}' cannot be called from within an action closure",
                            this.name
                        )
                        .into());
                    }
                    let mut def = this.app.0.lock();
                    if def.param_changes.contains_key(&this.name) {
                        return Err(format!(
                            "on_change already registered for parameter '{}'",
                            this.name
                        )
                        .into());
                    }
                    def.param_changes.insert(this.name.clone(), closure);
                    Ok(())
                },
            );
    }
}
