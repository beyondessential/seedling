use rhai::{CustomType, TypeBuilder};

// r[history.type]
// r[history.var]
#[derive(Debug, Clone)]
pub struct History;

// r[history.was-upgrading]
impl CustomType for History {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("History")
            .with_fn("was_upgrading", |_this: &mut Self| -> bool { false });
    }
}
