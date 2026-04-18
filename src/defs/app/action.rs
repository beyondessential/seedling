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
                let app_name = this.def.lock().name.clone();
                this.def.lock().actions.insert(
                    name.into(),
                    ActionDef {
                        name: name.into(),
                        description: None,
                        schedules: Vec::new(),
                    },
                );
                super::capture_action(name.into(), closure);
                Ok(Action::new(name.into(), app_name))
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
                let app_name = this.def.lock().name.clone();
                let desc = super::extract_description(&options);
                this.def.lock().actions.insert(
                    name.into(),
                    ActionDef {
                        name: name.into(),
                        description: desc,
                        schedules: Vec::new(),
                    },
                );
                super::capture_action(name.into(), closure);
                Ok(Action::new(name.into(), app_name))
            },
        );

    // l[impl action.start]
    builder
        .with_fn("on_start", |this: &mut App, closure: FnPtr| -> Action {
            let app_name = this.def.lock().name.clone();
            this.def.lock().actions.insert(
                "start".into(),
                ActionDef {
                    name: "start".into(),
                    description: None,
                    schedules: Vec::new(),
                },
            );
            super::capture_action("start".into(), closure);
            Action::new("start".into(), app_name)
        })
        .with_fn(
            "on_start",
            |this: &mut App, closure: FnPtr, options: Map| -> Action {
                let app_name = this.def.lock().name.clone();
                let desc = super::extract_description(&options);
                this.def.lock().actions.insert(
                    "start".into(),
                    ActionDef {
                        name: "start".into(),
                        description: desc,
                        schedules: Vec::new(),
                    },
                );
                super::capture_action("start".into(), closure);
                Action::new("start".into(), app_name)
            },
        );
}
