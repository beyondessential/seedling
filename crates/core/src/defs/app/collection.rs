use std::rc::Rc;

use rhai::{Dynamic, Map, TypeBuilder};

use super::super::collection::{AppBag, Collection};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // spec: COL#selection
    builder.with_fn(
        "select",
        |this: &mut App, criterion: Map| -> Result<Collection, Box<rhai::EvalAltResult>> {
            Collection::from_bag(Rc::new(AppBag(this.clone()))).select(&criterion)
        },
    );

    // spec: COL#query-methods
    builder.with_fn("one", |this: &mut App| -> Dynamic {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).one()
    });

    // spec: COL#query-methods
    builder.with_fn("only", |this: &mut App, other: Dynamic| -> Collection {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).only(other)
    });

    // spec: COL#query-methods
    builder.with_fn("except", |this: &mut App, other: Dynamic| -> Collection {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).except(other)
    });
}
