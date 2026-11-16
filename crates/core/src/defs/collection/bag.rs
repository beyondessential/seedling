use rhai::Dynamic;

use crate::defs::app::App;

use super::handle::Member;

pub trait ResourceBag {
    fn members(&self) -> Vec<Member>;
    fn fetch(&self, member: &Member) -> Option<Dynamic>;
}

pub(crate) struct AppBag(pub App);

// spec: ACN#action-declaration
// Actions are intentionally absent from the App's resource bag: they are
// invocable handles, not schedulable resources. `app.select(...)` and
// `col(app)` produce only the App's named resources, and `rt.start(app)`
// schedules those without ever entering an action's closure.
impl ResourceBag for AppBag {
    fn members(&self) -> Vec<Member> {
        self.0
            .def
            .load()
            .resources
            .keys()
            .cloned()
            .map(Member::Named)
            .collect()
    }

    fn fetch(&self, member: &Member) -> Option<Dynamic> {
        let Member::Named(id) = member else {
            return None;
        };
        self.0.def.load().resources.get(id).map(|r| r.to_dynamic())
    }
}

pub(crate) struct ItemBag {
    pub member: Member,
    pub value: Dynamic,
}

impl ResourceBag for ItemBag {
    fn members(&self) -> Vec<Member> {
        vec![self.member.clone()]
    }

    fn fetch(&self, _member: &Member) -> Option<Dynamic> {
        Some(self.value.clone())
    }
}
