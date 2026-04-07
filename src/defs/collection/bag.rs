use std::sync::Arc;

use rhai::Dynamic;

use crate::defs::action::Action;
use crate::defs::app::App;
use crate::defs::resource::{ResourceId, ResourceKind};

pub trait ResourceBag {
    fn ids(&self) -> Vec<ResourceId>;
    fn fetch(&self, id: &ResourceId) -> Option<Dynamic>;
}

pub(crate) struct AppBag(pub App);

impl ResourceBag for AppBag {
    fn ids(&self) -> Vec<ResourceId> {
        let def = self.0.def.lock();
        let resource_ids = def.resources.keys().cloned();
        let action_ids = def.actions.keys().map(|name| ResourceId {
            kind: ResourceKind::Action,
            name: Arc::new(name.clone()),
        });
        resource_ids.chain(action_ids).collect()
    }

    fn fetch(&self, id: &ResourceId) -> Option<Dynamic> {
        let def = self.0.def.lock();
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
