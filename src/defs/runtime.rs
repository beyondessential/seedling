use rhai::{CustomType, Dynamic, Map, TypeBuilder};

// l[impl rt.var]
// l[impl rt.type]
// l[impl rt.constructor]
// l[impl rt.lifecyle]
#[derive(Debug, Clone)]
pub struct RuntimeInstance;

// l[impl rt.start]
// l[impl rt.stop]
// l[impl rt.query]
// l[impl rt.reconcile]
// l[impl rt.methods]
impl CustomType for RuntimeInstance {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("RuntimeInstance")
            .with_fn(
                "start",
                |_this: &mut Self, _resources: Dynamic| -> Started { Started },
            )
            .with_fn("stop", |_this: &mut Self, _resources: Dynamic| {})
            .with_fn(
                "stop",
                |_this: &mut Self, _resources: Dynamic, _deadline: i64| {},
            )
            .with_fn(
                "query",
                |_this: &mut Self, _resources: Dynamic| -> Started { Started },
            )
            .with_fn(
                "reconcile",
                |_this: &mut Self, _old: Dynamic, _new: Dynamic| -> Started { Started },
            );
    }
}

// l[impl rt.started.type]
// l[impl rt.started.state-methods]
#[derive(Debug, Clone)]
pub struct Started;

impl CustomType for Started {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Started")
            .with_fn("scheduled", |this: &mut Self| this.clone())
            .with_fn("scheduled", |this: &mut Self, _deadline: i64| this.clone())
            .with_fn("running", |this: &mut Self| this.clone())
            .with_fn("running", |this: &mut Self, _deadline: i64| this.clone())
            .with_fn("ready", |this: &mut Self| this.clone())
            .with_fn("ready", |this: &mut Self, _deadline: i64| this.clone())
            // l[impl rt.started.terminated]
            .with_fn("terminated", |_this: &mut Self| -> Termination {
                Termination
            })
            .with_fn(
                "terminated",
                |_this: &mut Self, _deadline: i64| -> Termination { Termination },
            )
            // l[impl rt.started.type]: Collection methods on Started return Started
            .with_fn("one", |this: &mut Self| this.clone())
            .with_fn("only", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("except", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("select", |this: &mut Self, _criterion: Map| this.clone());
    }
}

// l[impl rt.termination.type]
// l[impl rt.termination.ensure-success]
#[derive(Debug, Clone)]
pub struct Termination;

impl CustomType for Termination {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Termination")
            .with_fn("ensure_success", |_this: &mut Self| {});
    }
}

// l[impl action.shell.attach]
pub fn register_shell_attach(engine: &mut rhai::Engine) {
    engine.register_fn("__bsl_shell_attach_impl", |_job: Dynamic| {});
}

pub fn shell_attach_fn_ptr() -> rhai::FnPtr {
    rhai::FnPtr::new("__bsl_shell_attach_impl").expect("valid function name")
}
