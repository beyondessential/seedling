#[cfg(test)]
use parking_lot::Mutex;
#[cfg(test)]
use std::collections::HashMap;

use crate::runtime::{LifecycleState, ResourceInstance};

pub trait WorldStateOracle: Send + Sync {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState;
}

#[cfg(test)]
pub struct TestWorldOracle {
    states: Mutex<HashMap<ResourceInstance, LifecycleState>>,
}

#[cfg(test)]
impl TestWorldOracle {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn set(&self, resource: ResourceInstance, state: LifecycleState) {
        self.states.lock().insert(resource, state);
    }
}

#[cfg(test)]
impl Default for TestWorldOracle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl WorldStateOracle for TestWorldOracle {
    fn lifecycle_state(&self, resource: &ResourceInstance) -> LifecycleState {
        let states = self.states.lock();

        // Try exact match first.
        if let Some(&s) = states.get(resource) {
            return s;
        }

        // Fallback: match by kind + name + ordinal, ignoring the app field.
        // This allows tests that key the oracle with a named app (e.g. "test-app")
        // to match resources extracted by the runtime with an empty app name.
        for (k, &v) in states.iter() {
            if k.kind == resource.kind && k.name == resource.name && k.ordinal == resource.ordinal {
                return v;
            }
        }

        LifecycleState::Pending
    }
}
