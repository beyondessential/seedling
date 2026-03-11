use rhai::{CustomType, Dynamic, TypeBuilder};

// l[impl collection.interface]
#[derive(Debug, Clone)]
pub struct Collection;

// l[impl collection.one]
// l[impl collection.only]
// l[impl collection.except]
// l[impl collection.select]
// l[impl collection.select.types]
// l[impl collection.select.names]
// l[impl collection.select.name-patterns]
impl CustomType for Collection {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Collection")
            .with_fn("one", |_this: &mut Self| -> Dynamic { Dynamic::UNIT })
            .with_fn("only", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("except", |this: &mut Self, _other: Dynamic| this.clone())
            .with_fn("select", |this: &mut Self, _criterion: rhai::Map| {
                this.clone()
            });
    }
}
