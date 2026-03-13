use rhai::{CustomType, FnPtr, TypeBuilder};

// l[impl param.type]
// l[impl param.value]
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub value: String,
}

// l[impl param.on-change]
impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Param")
            .with_fn("to_string", |this: &mut Self| -> String {
                this.value.clone()
            })
            .with_fn("to_debug", |this: &mut Self| -> String {
                format!("Param({:?}, {:?})", this.name, this.value)
            })
            .with_fn("on_change", |_this: &mut Self, _closure: FnPtr| {
                // stub: in the real runtime, this would register the closure
            });
    }
}
