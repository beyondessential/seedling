use rhai::{CustomType, Dynamic, FnPtr, TypeBuilder};

#[derive(Debug, Clone)]
pub struct ActionDef {
    pub name: String,
    pub closure: FnPtr,
    pub description: Option<String>,
}

// l[impl action.type]
#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
}

impl CustomType for Action {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Action")
            // l[impl collection.one]
            .with_fn("one", |_this: &mut Self| -> Dynamic { todo!() })
            // l[impl collection.only]
            .with_fn("only", |_this: &mut Self, _other: Dynamic| -> Dynamic {
                todo!()
            })
            // l[impl collection.except]
            .with_fn("except", |_this: &mut Self, _other: Dynamic| -> Dynamic {
                todo!()
            })
            // l[impl collection.select]
            .with_fn(
                "select",
                |_this: &mut Self, _criterion: rhai::Map| -> Dynamic { todo!() },
            );
    }
}

#[derive(Debug, Clone)]
pub struct ShellDef {
    pub name: String,
    pub closure: FnPtr,
    pub description: Option<String>,
}
