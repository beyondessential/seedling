use rhai::{CustomType, Dynamic, TypeBuilder};

// r[rt.type]
// r[rt.constructor]
#[derive(Debug, Clone)]
pub struct RuntimeInstance;

// r[rt.start]
// r[rt.stop]
// r[rt.query]
// r[rt.reconcile]
// r[rt.methods]
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

// r[rt.started.type]
// r[rt.started.state-methods]
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
            .with_fn("terminated", |_this: &mut Self| -> Termination {
                Termination
            })
            .with_fn(
                "terminated",
                |_this: &mut Self, _deadline: i64| -> Termination { Termination },
            )
            // Collection interface stubs
            .with_fn("one", |this: &mut Self| -> Dynamic {
                Dynamic::from(this.clone())
            })
            .with_fn("only", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("except", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("select", |this: &mut Self, _criterion: rhai::Map| {
                this.clone()
            });
    }
}

// r[rt.termination.type]
// r[rt.termination.ensure-success]
#[derive(Debug, Clone)]
pub struct Termination;

impl CustomType for Termination {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Termination")
            .with_fn("ensure_success", |_this: &mut Self| {});
    }
}
