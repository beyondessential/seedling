use rhai::{EvalAltResult, TypeBuilder};

use super::super::install::{ParamDef, ParamKind};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl param.type]
    // l[impl param.schema]
    builder.with_fn(
        "param",
        |this: &mut App, name: &str| -> Result<super::super::param::Param, Box<EvalAltResult>> {
            super::super::validate_name(name)?;
            let value = this.stored.lock().get(name).cloned();
            let name_owned: String = name.into();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_owned.clone())
                    .or_insert_with(|| ParamDef {
                        kind: ParamKind::Text,
                        required: false,
                        default_value: None,
                        description: None,
                    });
                d
            });
            Ok(super::super::param::Param {
                name: name.into(),
                value,
                app: this.clone(),
            })
        },
    );
}
