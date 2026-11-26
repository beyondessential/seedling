use std::rc::Rc;

use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::app::App;
use super::deployment::Deployment;
use super::ingress::Ingress;
use super::job::Job;
use super::resource::{ResourceId, ResourceKind, ResourceName};
use super::service::{ExternalService, HttpService, Service};
use super::volume::{ExternalVolume, Volume};
use crate::runtime::identity::InstanceId;

mod bag;
mod collectable;
mod handle;
mod selector;

pub use bag::ResourceBag;
pub(crate) use bag::{AppBag, ItemBag};
pub use collectable::Collectable;
pub use handle::{Member, ResourceHandle};
pub use selector::Selector;

use collectable::{BagCollection, Empty, Except, Only, Select, Union};

// ---------------------------------------------------------------------------
// Collection — the Rhai-facing type
// ---------------------------------------------------------------------------

// spec: COL#collection
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

    // spec: COL#query-methods
    pub fn one(&self) -> Dynamic {
        self.resolve()
            .into_iter()
            .next()
            .map(|h| h.fetch())
            .unwrap_or(Dynamic::UNIT)
    }

    // spec: COL#query-methods
    pub fn only(self, other: Dynamic) -> Self {
        Self(Rc::new(Only {
            left: self,
            right: col(other),
        }))
    }

    // spec: COL#query-methods
    pub fn except(self, other: Dynamic) -> Self {
        Self(Rc::new(Except {
            left: self,
            right: col(other),
        }))
    }

    // spec: COL#selection
    pub fn select(self, criterion: &Map) -> Result<Self, Box<rhai::EvalAltResult>> {
        Ok(Self(Rc::new(Select {
            inner: self,
            selector: Selector::from_map(criterion)?,
        })))
    }
}

impl CustomType for Collection {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Collection")
            // spec: COL#query-methods
            .with_fn("one", |this: &mut Self| -> Dynamic { this.one() })
            // spec: COL#query-methods
            .with_fn("only", |this: &mut Self, other: Dynamic| -> Collection {
                this.clone().only(other)
            })
            // spec: COL#query-methods
            .with_fn("except", |this: &mut Self, other: Dynamic| -> Collection {
                this.clone().except(other)
            })
            // spec: COL#selection
            .with_fn(
                "select",
                |this: &mut Self, criterion: Map| -> Result<Collection, Box<rhai::EvalAltResult>> {
                    this.clone().select(&criterion)
                },
            );
    }
}

// ---------------------------------------------------------------------------
// col() — coerce any value into a Collection
// ---------------------------------------------------------------------------

/// A collection of the one resource `value`, identified as `member`.
fn item(member: Member, value: Dynamic) -> Collection {
    Collection::from_bag(Rc::new(ItemBag { member, value }))
}

/// How a Deployment, Job or Service is identified: by name, or by the
/// identity an anonymous one holds.
fn member(kind: ResourceKind, name: &ResourceName, anon_id: Option<InstanceId>) -> Member {
    match anon_id {
        Some(id) => Member::Anonymous { kind, id },
        None => Member::Named(ResourceId {
            kind,
            name: name.clone(),
        }),
    }
}

// spec: COL#coercion
pub fn col(val: Dynamic) -> Collection {
    if let Some(c) = val.clone().try_cast::<Collection>() {
        return c;
    }

    if let Some(app) = val.clone().try_cast::<App>() {
        return Collection::from_bag(Rc::new(AppBag(app)));
    }

    if let Some(dep) = val.clone().try_cast::<Deployment>() {
        let member = member(ResourceKind::Deployment, &dep.name, dep.anon_id);
        return item(member, Dynamic::from(dep));
    }

    if let Some(svc) = val.clone().try_cast::<Service>() {
        let member = member(ResourceKind::Service, &svc.name, svc.anon_id);
        return item(member, Dynamic::from(svc));
    }

    if let Some(ext) = val.clone().try_cast::<ExternalService>() {
        let member = member(ResourceKind::ExternalService, &ext.name, None);
        return item(member, Dynamic::from(ext));
    }

    // An HttpService is a view of a service, so it is that service.
    if let Some(h) = val.clone().try_cast::<HttpService>() {
        return col(Dynamic::from(h.service));
    }

    if let Some(job) = val.clone().try_cast::<Job>() {
        let member = member(ResourceKind::Job, &job.name, job.anon_id);
        return item(member, Dynamic::from(job));
    }

    if let Some(ingress) = val.clone().try_cast::<Ingress>() {
        let member = member(ResourceKind::Ingress, &ingress.name, None);
        return item(member, Dynamic::from(ingress));
    }

    if let Some(vol) = val.clone().try_cast::<Volume>() {
        let Some(name) = vol.name.clone() else {
            return Collection::empty();
        };
        let member = member(ResourceKind::Volume, &name, None);
        return item(member, Dynamic::from(vol));
    }

    if let Some(vol) = val.clone().try_cast::<ExternalVolume>() {
        let member = member(ResourceKind::ExternalVolume, &vol.name, None);
        return item(member, Dynamic::from(vol));
    }

    if let Some(arr) = val.try_cast::<rhai::Array>() {
        let parts = arr.into_iter().map(col).collect();
        return Collection(Rc::new(Union { parts }));
    }

    Collection::empty()
}
