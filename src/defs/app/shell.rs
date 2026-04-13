use rhai::{FnPtr, Map, TypeBuilder};

use super::super::action::ShellDef;
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.shell]
    builder
        .with_fn("on_shell", |this: &mut App, name: &str, closure: FnPtr| {
            this.def.lock().shells.insert(
                name.into(),
                ShellDef {
                    name: name.into(),
                    description: None,
                },
            );
            super::capture_shell(name.into(), closure);
        })
        .with_fn(
            "on_shell",
            |this: &mut App, name: &str, closure: FnPtr, options: Map| {
                let desc = super::extract_description(&options);
                this.def.lock().shells.insert(
                    name.into(),
                    ShellDef {
                        name: name.into(),
                        description: desc,
                    },
                );
                super::capture_shell(name.into(), closure);
            },
        );
}
