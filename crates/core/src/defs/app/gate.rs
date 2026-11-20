use rhai::{EvalAltResult, TypeBuilder};
use seedling_protocol::names::GateName;

use super::super::gate::{Gate, declare};
use super::{App, ensure_not_in_action_closure, ensure_not_validating};

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: GATE#declaration
    builder.with_fn(
        "gate",
        |this: &mut App, name: &str| -> Result<Gate, Box<EvalAltResult>> {
            let name =
                GateName::new(name).map_err(|e| -> Box<EvalAltResult> { e.to_string().into() })?;
            ensure_not_validating()?;
            ensure_not_in_action_closure("app.gate")?;
            Ok(declare(this, name))
        },
    );
}
