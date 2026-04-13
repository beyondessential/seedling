use rhai::{EvalAltResult, TypeBuilder};

use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl param.type]
    builder.with_fn(
        "param",
        |this: &mut App, name: &str| -> Result<super::super::param::Param, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let value = this.stored.lock().get(name).cloned();
            this.def.lock().params.insert(name.into());
            Ok(super::super::param::Param {
                name: name.into(),
                value,
                app: this.clone(),
            })
        },
    );
}
