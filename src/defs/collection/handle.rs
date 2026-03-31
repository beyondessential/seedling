use std::rc::Rc;

use rhai::Dynamic;

use crate::defs::resource::{ResourceId, ResourceKind};

use super::bag::ResourceBag;

#[derive(Clone)]
pub struct ResourceHandle(pub Rc<dyn ResourceBag>, pub ResourceId);

impl ResourceHandle {
    pub fn kind(&self) -> ResourceKind {
        self.1.kind
    }

    pub fn name(&self) -> &str {
        self.1.name.as_str()
    }

    pub fn fetch(&self) -> Dynamic {
        self.0.fetch(&self.1).unwrap_or(Dynamic::UNIT)
    }
}
