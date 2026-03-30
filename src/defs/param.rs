use rhai::{CustomType, FnPtr, TypeBuilder};

// l[impl param.type]
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub value: String,
}

impl CustomType for Param {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Param")
            // l[impl param.value]
            .with_fn("to_string", |this: &mut Self| -> String {
                this.value.clone()
            })
            .with_fn("to_debug", |this: &mut Self| -> String {
                format!("Param({:?}, {:?})", this.name, this.value)
            })
            // l[impl param.on-change]
            .with_fn("on_change", |_this: &mut Self, _closure: FnPtr| -> () {
                todo!()
            });
    }
}
