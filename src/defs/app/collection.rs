use std::rc::Rc;

use rhai::{Dynamic, Map, TypeBuilder};

use super::super::collection::{AppBag, Collection};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl collection.select]
    builder.with_fn("select", |this: &mut App, criterion: Map| -> Collection {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).select(&criterion)
    });

    // l[impl collection.one]
    builder.with_fn("one", |this: &mut App| -> Dynamic {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).one()
    });

    // l[impl collection.only]
    builder.with_fn("only", |this: &mut App, other: Dynamic| -> Collection {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).only(other)
    });

    // l[impl collection.except]
    builder.with_fn("except", |this: &mut App, other: Dynamic| -> Collection {
        Collection::from_bag(Rc::new(AppBag(this.clone()))).except(other)
    });
}
