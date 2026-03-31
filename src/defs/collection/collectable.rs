use std::collections::HashSet;
use std::rc::Rc;

use crate::defs::resource::ResourceKind;

use super::Collection;
use super::bag::ResourceBag;
use super::handle::ResourceHandle;
use super::selector::Selector;

pub trait Collectable {
    fn resolve(&self) -> Vec<ResourceHandle>;
}

pub(super) struct BagCollection(pub Rc<dyn ResourceBag>);

impl Collectable for BagCollection {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.0
            .ids()
            .into_iter()
            .map(|id| ResourceHandle(Rc::clone(&self.0), id))
            .collect()
    }
}

pub(super) struct Select {
    pub inner: Collection,
    pub selector: Selector,
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

pub(super) struct Only {
    pub left: Collection,
    pub right: Collection,
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

pub(super) struct Except {
    pub left: Collection,
    pub right: Collection,
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

pub(super) struct Union {
    pub parts: Vec<Collection>,
}

impl Collectable for Union {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.parts.iter().flat_map(|p| p.resolve()).collect()
    }
}

pub(super) struct Empty;

impl Collectable for Empty {
    fn resolve(&self) -> Vec<ResourceHandle> {
        vec![]
    }
}
