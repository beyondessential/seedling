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
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.shells.insert(
                        name_owned.clone(),
                        ShellDef {
                            name: name_owned.clone(),
                            description: None,
                        },
                    );
                    d
                });
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
                let name_owned: String = name.into();
                this.def.rcu(|d| {
                    let mut d = (**d).clone();
                    d.shells.insert(
                        name_owned.clone(),
                        ShellDef {
                            name: name_owned.clone(),
                            description: desc.clone(),
                        },
                    );
                    d
                });
                super::capture_shell(name.into(), closure);
                Ok(())
            },
        );
}
