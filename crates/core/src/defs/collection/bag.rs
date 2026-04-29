use rhai::Dynamic;

use crate::defs::app::App;
use crate::defs::resource::ResourceId;

pub trait ResourceBag {
    fn ids(&self) -> Vec<ResourceId>;
    fn fetch(&self, id: &ResourceId) -> Option<Dynamic>;
}

pub(crate) struct AppBag(pub App);

// l[impl action.type]
// Actions are intentionally absent from the App's resource bag: they are
// invocable handles, not schedulable resources. `app.select(...)` and
// `col(app)` produce only the App's named resources, and `rt.start(app)`
// schedules those without ever entering an action's closure.
impl ResourceBag for AppBag {
    fn ids(&self) -> Vec<ResourceId> {
        self.0.def.load().resources.keys().cloned().collect()
    }

    fn fetch(&self, id: &ResourceId) -> Option<Dynamic> {
        self.0.def.load().resources.get(id).map(|r| r.to_dynamic())
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
