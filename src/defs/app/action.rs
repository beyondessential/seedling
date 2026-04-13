use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};

use super::super::action::{Action, ActionDef};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.type]
    // l[impl action.option-description]
    builder
        .with_fn(
            "on_action",
            |this: &mut App, name: &str, closure: FnPtr| -> Result<Action, Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                this.def.lock().actions.insert(
                    name.into(),
                    ActionDef {
                        name: name.into(),
                        description: None,
                    },
                );
                super::capture_action(name.into(), closure);
                Ok(Action { name: name.into() })
            },
        )
        .with_fn(
            "on_action",
            |this: &mut App,
             name: &str,
             closure: FnPtr,
             options: Map|
             -> Result<Action, Box<EvalAltResult>> {
                super::super::validate_name(name)?;
                let desc = super::extract_description(&options);
                this.def.lock().actions.insert(
                    name.into(),
                    ActionDef {
                        name: name.into(),
                        description: desc,
                    },
                );
                super::capture_action(name.into(), closure);
                Ok(Action { name: name.into() })
            },
        );

    // l[impl action.start]
    builder
        .with_fn("on_start", |this: &mut App, closure: FnPtr| -> Action {
            this.def.lock().actions.insert(
                "start".into(),
                ActionDef {
                    name: "start".into(),
                    description: None,
                },
            );
            super::capture_action("start".into(), closure);
            Action {
                name: "start".into(),
            }
        })
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr, options: Map| -> Action {
                let desc = super::extract_description(&options);
                this.def.lock().actions.insert(
                    "start".into(),
                    ActionDef {
                        name: "start".into(),
                        description: desc,
                    },
                );
                super::capture_action("start".into(), closure);
                Action {
                    name: "start".into(),
                }
            },
        );
}
