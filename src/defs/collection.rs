use std::collections::HashSet;
use std::sync::Arc;

use rhai::{CustomType, Dynamic, Map, TypeBuilder};

use super::app::App;
use super::resource::{ResourceId, ResourceKind};

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
        // Scaffolding — filled in next commit.
        vec![]
    }

    fn fetch(&self, _id: &ResourceId) -> Option<Dynamic> {
        // Scaffolding — filled in next commit.
        None
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
pub struct ResourceHandle(pub Arc<dyn ResourceBag>, pub ResourceId);

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
struct BagCollection(Arc<dyn ResourceBag>);

impl Collectable for BagCollection {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.0
            .ids()
            .into_iter()
            .map(|id| ResourceHandle(Arc::clone(&self.0), id))
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
        todo!("Select::resolve")
    }
}

// Lazy intersection — resources in left that are also in right.
struct Only {
    left: Collection,
    right: Collection,
}

impl Collectable for Only {
    fn resolve(&self) -> Vec<ResourceHandle> {
        todo!("Only::resolve")
    }
}

// Lazy difference — resources in left that are not in right.
struct Except {
    left: Collection,
    right: Collection,
}

impl Collectable for Except {
    fn resolve(&self) -> Vec<ResourceHandle> {
        todo!("Except::resolve")
    }
}

// Lazy union — flattens an array of collections.
struct Union {
    parts: Vec<Collection>,
}

impl Collectable for Union {
    fn resolve(&self) -> Vec<ResourceHandle> {
        todo!("Union::resolve")
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
        // Scaffolding — will parse `types`, `names`, `name_patterns` keys.
        let _ = map;
        Selector::default()
    }

    pub fn matches(&self, handle: &ResourceHandle) -> bool {
        todo!(
            "Selector::matches — kind={:?} name={}",
            handle.kind(),
            handle.name()
        )
    }
}

// ---------------------------------------------------------------------------
// Collection — the Rhai-facing type
// ---------------------------------------------------------------------------

// l[impl collection.interface]
#[derive(Clone)]
pub struct Collection(pub(crate) Arc<dyn Collectable>);

impl Collection {
    pub fn empty() -> Self {
        Self(Arc::new(Empty))
    }

    pub fn from_bag(bag: Arc<dyn ResourceBag>) -> Self {
        Self(Arc::new(BagCollection(bag)))
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
        Self(Arc::new(Only {
            left: self,
            right: col(other),
        }))
    }

    // l[impl collection.except]
    pub fn except(self, other: Dynamic) -> Self {
        Self(Arc::new(Except {
            left: self,
            right: col(other),
        }))
    }

    // l[impl collection.select]
    pub fn select(self, criterion: &Map) -> Self {
        Self(Arc::new(Select {
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

/// Coerces any Collection-like value into a `Collection`.
///
/// - `Collection` is returned as-is.
/// - `App` is wrapped in an [`AppBag`].
/// - Individual resource types (Deployment, Service, …) are wrapped in an
///   [`ItemBag`] — handled in the implementation commit.
/// - A Rhai array is collapsed into a [`Union`] — handled in the
///   implementation commit.
/// - Everything else yields an empty collection.
// l[impl collection.interface]
pub fn col(val: Dynamic) -> Collection {
    if let Some(c) = val.clone().try_cast::<Collection>() {
        return c;
    }

    if let Some(app) = val.try_cast::<App>() {
        return Collection::from_bag(Arc::new(AppBag(app)));
    }

    // TODO: Deployment, Service, Job, Action, Ingress, Volume,
    //       ExternalService, ExternalVolume, Array — next commit.
    Collection::empty()
}

// ---------------------------------------------------------------------------
// Glob matching (used by Selector::matches)
// ---------------------------------------------------------------------------

/// Returns true if `text` matches `pattern`, where `*` matches any sequence
/// of characters and `?` matches exactly one character.
fn glob_matches(_pattern: &str, _text: &str) -> bool {
    todo!("glob_matches")
}
