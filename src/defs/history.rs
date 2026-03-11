use rhai::{CustomType, TypeBuilder};

// l[impl history.type]
// l[impl history.var]
#[derive(Debug, Clone)]
pub struct History;

// l[impl history.was-upgrading]
impl CustomType for History {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("History")
            .with_fn("was_upgrading", |_this: &mut Self| -> bool { false });
    }
}
