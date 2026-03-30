use rhai::{CustomType, Dynamic, TypeBuilder};

// l[impl collection.interface]
#[derive(Debug, Clone)]
pub struct Collection;

impl CustomType for Collection {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Collection")
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
