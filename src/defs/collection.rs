use std::rc::Rc;
use std::sync::Arc;

use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::action::Action;
use super::app::App;
use super::deployment::Deployment;
use super::ingress::Ingress;
use super::job::Job;
use super::resource::{ResourceId, ResourceKind};
use super::service::{ExternalService, HttpService, Service};
use super::volume::{ExternalVolume, Volume};

mod bag;
mod collectable;
mod handle;
mod selector;

pub use bag::ResourceBag;
pub(crate) use bag::{AppBag, ItemBag};
pub use collectable::Collectable;
pub use handle::ResourceHandle;
pub use selector::Selector;

use collectable::{BagCollection, Empty, Except, Only, Select, Union};

// ---------------------------------------------------------------------------
// Collection — the Rhai-facing type
// ---------------------------------------------------------------------------

// l[impl collection.interface]
#[derive(Clone)]
pub struct Collection(pub(crate) Rc<dyn Collectable>);

impl Collection {
    pub fn empty() -> Self {
        Self(Rc::new(Empty))
    }

    pub fn from_bag(bag: Rc<dyn ResourceBag>) -> Self {
        Self(Rc::new(BagCollection(bag)))
    }

    pub fn resolve(&self) -> Vec<ResourceHandle> {
        self.0.resolve()
    }

    // l[impl collection.one]
    pub fn one(&self) -> Dynamic {
        self.resolve()
            .into_iter()
            .next()
            .map(|h| h.fetch())
            .unwrap_or(Dynamic::UNIT)
    }

    // l[impl collection.only]
    pub fn only(self, other: Dynamic) -> Self {
        Self(Rc::new(Only {
            left: self.0,
            right: col(other).0,
        }))
    }

    // l[impl collection.except]
    pub fn except(self, other: Dynamic) -> Self {
        Self(Rc::new(Except {
            left: self.0,
            right: col(other).0,
        }))
    }

    // l[impl collection.select]
    pub fn select(self, criterion: &Map) -> Self {
        Self(Rc::new(Select {
            inner: self.0,
            selector: Selector::from_map(criterion),
        }))
    }
}

impl CustomType for Collection {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Collection")
            // l[impl collection.one]
            .with_fn("one", |this: &mut Self| -> Dynamic { this.one() })
            // l[impl collection.only]
            .with_fn("only", |this: &mut Self, other: Dynamic| -> Collection {
                this.clone().only(other)
            })
            // l[impl collection.except]
            .with_fn("except", |this: &mut Self, other: Dynamic| -> Collection {
                this.clone().except(other)
            })
            // l[impl collection.select]
            .with_fn("select", |this: &mut Self, criterion: Map| -> Collection {
                this.clone().select(&criterion)
            });
    }
}

// ---------------------------------------------------------------------------
// col() — coerce any value into a Collection
// ---------------------------------------------------------------------------

// l[impl collection.col]
pub fn col(val: Dynamic) -> Collection {
    if let Some(c) = val.clone().try_cast::<Collection>() {
        return c;
    }

    if let Some(app) = val.clone().try_cast::<App>() {
        return Collection::from_bag(Rc::new(AppBag(app)));
    }

    if let Some(dep) = val.clone().try_cast::<Deployment>() {
        let id = ResourceId {
            kind: ResourceKind::Deployment,
            name: dep.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(dep),
        }));
    }

    if let Some(svc) = val.clone().try_cast::<Service>() {
        let id = ResourceId {
            kind: ResourceKind::Service,
            name: svc.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(svc),
        }));
    }

    if let Some(h) = val.clone().try_cast::<HttpService>() {
        let id = ResourceId {
            kind: ResourceKind::HttpService,
            name: h.service.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(h),
        }));
    }

    if let Some(job) = val.clone().try_cast::<Job>() {
        let id = ResourceId {
            kind: ResourceKind::Job,
            name: job.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(job),
        }));
    }

    if let Some(action) = val.clone().try_cast::<Action>() {
        let id = ResourceId {
            kind: ResourceKind::Action,
            name: Arc::new(action.name.clone()),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(action),
        }));
    }

    if let Some(ingress) = val.clone().try_cast::<Ingress>() {
        let id = ResourceId {
            kind: ResourceKind::Ingress,
            name: ingress.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(ingress),
        }));
    }

    if let Some(vol) = val.clone().try_cast::<Volume>() {
        if let Some(name) = vol.name.clone() {
            let id = ResourceId {
                kind: ResourceKind::Volume,
                name,
            };
            return Collection::from_bag(Rc::new(ItemBag {
                id,
                value: Dynamic::from(vol),
            }));
        }
        return Collection::empty();
    }

    if let Some(ext) = val.clone().try_cast::<ExternalService>() {
        let id = ResourceId {
            kind: ResourceKind::ExternalService,
            name: ext.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(ext),
        }));
    }

    if let Some(vol) = val.clone().try_cast::<ExternalVolume>() {
        let id = ResourceId {
            kind: ResourceKind::ExternalVolume,
            name: vol.name.clone(),
        };
        return Collection::from_bag(Rc::new(ItemBag {
            id,
            value: Dynamic::from(vol),
        }));
    }

    if let Some(arr) = val.try_cast::<rhai::Array>() {
        let parts = arr.into_iter().map(|v| col(v).0).collect();
        return Collection(Rc::new(Union { parts }));
    }

    Collection::empty()
}
