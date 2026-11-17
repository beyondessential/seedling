use std::rc::Rc;

use rhai::Dynamic;

use crate::defs::resource::{ResourceId, ResourceKind};
use crate::runtime::identity::InstanceId;

use super::bag::ResourceBag;

/// What makes one resource the same as another within a collection.
///
/// Named resources are the same when they share a type and name. An
/// anonymous resource is the same only as itself, by the identity it holds
/// for its lifetime, which no named resource's identity can equal.
// spec: COL#membership
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Member {
    Named(ResourceId),
    Anonymous { kind: ResourceKind, id: InstanceId },
}

impl Member {
    pub fn kind(&self) -> ResourceKind {
        match self {
            Self::Named(id) => id.kind,
            Self::Anonymous { kind, .. } => *kind,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(id) => Some(id.name.as_str()),
            Self::Anonymous { .. } => None,
        }
    }
}

#[derive(Clone)]
pub struct ResourceHandle(pub Rc<dyn ResourceBag>, pub Member);

impl ResourceHandle {
    pub fn kind(&self) -> ResourceKind {
        self.1.kind()
    }

    pub fn name(&self) -> Option<&str> {
        self.1.name()
    }

    pub fn member(&self) -> &Member {
        &self.1
    }

    pub fn fetch(&self) -> Dynamic {
        self.0.fetch(&self.1).unwrap_or(Dynamic::UNIT)
    }
}
