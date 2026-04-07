use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::collection::{Collection, col};

#[derive(Debug, Clone)]
pub struct ActionDef {
    pub name: String,
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
            .with_fn("one", |this: &mut Self| -> Dynamic {
                col(Dynamic::from(this.clone())).one()
            })
            // l[impl collection.only]
            .with_fn("only", |this: &mut Self, other: Dynamic| -> Collection {
                col(Dynamic::from(this.clone())).only(other)
            })
            // l[impl collection.except]
            .with_fn("except", |this: &mut Self, other: Dynamic| -> Collection {
                col(Dynamic::from(this.clone())).except(other)
            })
            // l[impl collection.select]
            .with_fn("select", |this: &mut Self, criterion: Map| -> Collection {
                col(Dynamic::from(this.clone())).select(&criterion)
            });
    }
}

#[derive(Debug, Clone)]
pub struct ShellDef {
    pub name: String,
    pub description: Option<String>,
}
