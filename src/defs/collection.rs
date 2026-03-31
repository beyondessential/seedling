use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use wildmatch::WildMatch;

use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::action::Action;
use super::app::App;
use super::deployment::Deployment;
use super::ingress::Ingress;
use super::job::Job;
use super::resource::{ResourceId, ResourceKind};
use super::service::{ExternalService, HttpService, Service};
use super::volume::{ExternalVolume, Volume};

// ---------------------------------------------------------------------------
// ResourceBag — source of resource identity and value data
// ---------------------------------------------------------------------------

/// Backing store for a collection. Knows how to enumerate resource IDs and
/// reconstruct Rhai-typed values on demand.
pub trait ResourceBag {
    fn ids(&self) -> Vec<ResourceId>;
    fn fetch(&self, id: &ResourceId) -> Option<Dynamic>;
}

/// App-backed bag: enumerates all static resources and named actions.
pub(crate) struct AppBag(pub App);

impl ResourceBag for AppBag {
    fn ids(&self) -> Vec<ResourceId> {
        let def = self.0.0.lock();
        let resource_ids = def.resources.keys().cloned();
        let action_ids = def.actions.keys().map(|name| ResourceId {
            kind: ResourceKind::Action,
            name: Arc::new(name.clone()),
        });
        resource_ids.chain(action_ids).collect()
    }

    fn fetch(&self, id: &ResourceId) -> Option<Dynamic> {
        let def = self.0.0.lock();
        if id.kind == ResourceKind::Action {
            def.actions.get(id.name.as_str()).map(|_| {
                Dynamic::from(Action {
                    name: (*id.name).clone(),
                })
            })
        } else {
            def.resources.get(id).map(|r| r.to_dynamic())
        }
    }
}

/// Single-item bag: wraps one resource so individual resource values can be
/// used as collections via `col(dep)`, `col(svc)`, etc.
pub(crate) struct ItemBag {
    pub id: ResourceId,
    pub value: Dynamic,
}

impl ResourceBag for ItemBag {
    fn ids(&self) -> Vec<ResourceId> {
        vec![self.id.clone()]
    }

    fn fetch(&self, _id: &ResourceId) -> Option<Dynamic> {
        Some(self.value.clone())
    }
}

// ---------------------------------------------------------------------------
// ResourceHandle — lightweight identity reference into a ResourceBag
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ResourceHandle(pub Rc<dyn ResourceBag>, pub ResourceId);

impl ResourceHandle {
    pub fn kind(&self) -> ResourceKind {
        self.1.kind
    }

    pub fn name(&self) -> &str {
        self.1.name.as_str()
    }

    /// Fetches the Rhai-typed resource value from the backing bag.
    /// Only needed when materialising a value for `one()`.
    pub fn fetch(&self) -> Dynamic {
        self.0.fetch(&self.1).unwrap_or(Dynamic::UNIT)
    }
}

// ---------------------------------------------------------------------------
// Collectable — lazy composition trait
// ---------------------------------------------------------------------------

pub trait Collectable {
    fn resolve(&self) -> Vec<ResourceHandle>;
}

// All resources from one bag (e.g. an entire App).
struct BagCollection(Rc<dyn ResourceBag>);

impl Collectable for BagCollection {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.0
            .ids()
            .into_iter()
            .map(|id| ResourceHandle(Rc::clone(&self.0), id))
            .collect()
    }
}

// Lazy filter — resolved by Selector::matches.
struct Select {
    inner: Collection,
    selector: Selector,
}

impl Collectable for Select {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.inner
            .resolve()
            .into_iter()
            .filter(|h| self.selector.matches(h))
            .collect()
    }
}

// Lazy intersection — resources in left that are also in right.
struct Only {
    left: Collection,
    right: Collection,
}

impl Collectable for Only {
    fn resolve(&self) -> Vec<ResourceHandle> {
        let right_keys: HashSet<(ResourceKind, String)> = self
            .right
            .resolve()
            .into_iter()
            .map(|h| (h.kind(), h.name().to_owned()))
            .collect();
        self.left
            .resolve()
            .into_iter()
            .filter(|h| right_keys.contains(&(h.kind(), h.name().to_owned())))
            .collect()
    }
}

// Lazy difference — resources in left that are not in right.
struct Except {
    left: Collection,
    right: Collection,
}

impl Collectable for Except {
    fn resolve(&self) -> Vec<ResourceHandle> {
        let right_keys: HashSet<(ResourceKind, String)> = self
            .right
            .resolve()
            .into_iter()
            .map(|h| (h.kind(), h.name().to_owned()))
            .collect();
        self.left
            .resolve()
            .into_iter()
            .filter(|h| !right_keys.contains(&(h.kind(), h.name().to_owned())))
            .collect()
    }
}

// Lazy union — flattens an array of collections.
struct Union {
    parts: Vec<Collection>,
}

impl Collectable for Union {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.parts.iter().flat_map(|p| p.resolve()).collect()
    }
}

// Always-empty collection.
struct Empty;

impl Collectable for Empty {
    fn resolve(&self) -> Vec<ResourceHandle> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Selector — parsed filter criterion for select()
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct Selector {
    pub types: Option<Vec<ResourceKind>>,
    pub names: Option<HashSet<String>>,
    pub name_patterns: Option<Vec<String>>,
}

impl Selector {
    pub fn from_map(map: &Map) -> Self {
        // l[impl collection.select.types]
        let types = map.get("types").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.try_cast::<ResourceKind>())
                    .collect()
            })
        });

        // l[impl collection.select.names]
        let names = map.get("names").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.into_string().ok())
                    .collect()
            })
        });

        // l[impl collection.select.name-patterns]
        let name_patterns = map.get("name_patterns").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.into_string().ok())
                    .collect()
            })
        });

        Selector {
            types,
            names,
            name_patterns,
        }
    }

    pub fn matches(&self, handle: &ResourceHandle) -> bool {
        // l[impl collection.select.types]
        if let Some(types) = &self.types
            && !types.contains(&handle.kind())
        {
            return false;
        }

        // l[impl collection.select.names]
        if let Some(names) = &self.names
            && !names.contains(handle.name())
        {
            return false;
        }

        // l[impl collection.select.name-patterns]
        if let Some(patterns) = &self.name_patterns
            && !patterns
                .iter()
                .any(|p| WildMatch::new(p).matches(handle.name()))
        {
            return false;
        }

        true
    }
}

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
            left: self,
            right: col(other),
        }))
    }

    // l[impl collection.except]
    pub fn except(self, other: Dynamic) -> Self {
        Self(Rc::new(Except {
            left: self,
            right: col(other),
        }))
    }

    // l[impl collection.select]
    pub fn select(self, criterion: &Map) -> Self {
        Self(Rc::new(Select {
            inner: self,
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

/// Coerces any Collection-like value into a [`Collection`].
///
/// Handles `Collection`, `App`, all named resource types, and Rhai arrays
/// (which are flattened into a [`Union`]). Anonymous volumes and unknown
/// types yield an empty collection.
// l[impl collection.interface]
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
        // Anonymous volumes have no ResourceId and cannot participate in collections.
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
        let parts = arr.into_iter().map(col).collect();
        return Collection(Rc::new(Union { parts }));
    }

    Collection::empty()
}
