use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};

use super::super::action::ShellDef;
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.shell]
    builder
        .with_fn(
            "on_shell",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<(), Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                this.def.lock().shells.insert(
                    name.into(),
                    ShellDef {
                        name: name.into(),
                        description: None,
                    },
                );
                super::capture_shell(name.into(), closure);
                Ok(())
            },
        )
        .with_fn(
            "on_shell",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<(), Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let desc = super::extract_description(&options);
                this.def.lock().shells.insert(
                    name.into(),
                    ShellDef {
                        name: name.into(),
                        description: desc,
                    },
                );
                super::capture_shell(name.into(), closure);
                Ok(())
            },
        );
}
