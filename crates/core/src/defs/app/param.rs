use rhai::{EvalAltResult, TypeBuilder};
use seedling_protocol::names::ParamName;

use super::super::install::{ParamDef, ParamKind};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl param.type]
    // l[impl param.schema]
    builder.with_fn(
        "param",
        |this: &mut App, name: &str| -> Result<super::super::param::Param, Box<EvalAltResult>> {
            let param_name =
                ParamName::new(name).map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
            let value = this.stored.lock().get(name).cloned();
            let name_for_insert = param_name.clone();
            this.def.rcu(|d| {
                let mut d = (**d).clone();
                d.params
                    .entry(name_for_insert.clone())
                    .or_insert_with(|| ParamDef {
                        kind: ParamKind::Text,
                        required: false,
                        default_value: None,
                        description: None,
                        secret: false,
                    });
                d
            });
            Ok(super::super::param::Param {
                name: param_name,
                value,
                app: this.clone(),
            })
        },
    );
}
