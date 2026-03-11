use rhai::{CustomType, FnPtr, TypeBuilder};

// r[action.type]
#[derive(Debug, Clone)]
pub struct ActionDef {
    pub name: String,
    pub closure: FnPtr,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
}

impl CustomType for Action {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Action");
    }
}

// r[action.shell]
#[derive(Debug, Clone)]
pub struct ShellDef {
    pub name: String,
    pub closure: FnPtr,
    pub description: Option<String>,
}
