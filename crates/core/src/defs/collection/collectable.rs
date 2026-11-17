use std::collections::HashSet;
use std::rc::Rc;

use super::Collection;
use super::bag::ResourceBag;
use super::handle::{Member, ResourceHandle};
use super::selector::Selector;

pub trait Collectable {
    fn resolve(&self) -> Vec<ResourceHandle>;
}

pub(super) struct BagCollection(pub Rc<dyn ResourceBag>);

impl Collectable for BagCollection {
    fn resolve(&self) -> Vec<ResourceHandle> {
        self.0
            .members()
            .into_iter()
            .map(|member| ResourceHandle(Rc::clone(&self.0), member))
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
        let right_keys: HashSet<Member> = self
            .right
            .resolve()
            .into_iter()
            .map(|h| h.member().clone())
            .collect();
        self.left
            .resolve()
            .into_iter()
            .filter(|h| right_keys.contains(h.member()))
            .collect()
    }
}

pub(super) struct Except {
    pub left: Collection,
    pub right: Collection,
}

impl Collectable for Except {
    fn resolve(&self) -> Vec<ResourceHandle> {
        let right_keys: HashSet<Member> = self
            .right
            .resolve()
            .into_iter()
            .map(|h| h.member().clone())
            .collect();
        self.left
            .resolve()
            .into_iter()
            .filter(|h| !right_keys.contains(h.member()))
            .collect()
    }
}

pub(super) struct Union {
    pub parts: Vec<Collection>,
}

impl Collectable for Union {
    fn resolve(&self) -> Vec<ResourceHandle> {
        let mut seen = HashSet::new();
        self.parts
            .iter()
            .flat_map(|p| p.resolve())
            .filter(|h| seen.insert(h.member().clone()))
            .collect()
    }
}

pub(super) struct Empty;

impl Collectable for Empty {
    fn resolve(&self) -> Vec<ResourceHandle> {
        vec![]
    }
}
