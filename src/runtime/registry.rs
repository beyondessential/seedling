use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;

use crate::defs::resource::ResourceKind;
use crate::runtime::db::Db;
use crate::runtime::history;
use crate::runtime::identity::ResourceInstance;

/// Failure to look up or create an instance in the registry.
#[derive(Debug)]
pub struct RegistryError(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "instance registry error: {}", self.0)
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

impl From<rusqlite::Error> for RegistryError {
    fn from(e: rusqlite::Error) -> Self {
        Self(Box::new(e))
    }
}

/// Provides access to the instance registry during action-closure execution.
///
/// The registry is the authoritative source for which instances exist and
/// what their stable identities are.  `RuntimeInstance` uses it to resolve
/// BSL resource objects (e.g. a `Deployment`) into concrete `ResourceInstance`
/// values before recording them in the action log or querying the world oracle.
pub trait InstanceRegistry: Send + Sync {
    /// Return the singleton instance for `(app, kind, name)`, creating and
    /// persisting a new one if none exists yet.
    fn get_or_create_singleton(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError>;
}

// ---------------------------------------------------------------------------
// EphemeralInstanceRegistry
// ---------------------------------------------------------------------------

type InstanceRegistryKey = (String, ResourceKind, Option<String>);

/// Generates UUIDs on first use and caches them for the lifetime of this
/// registry instance.  Repeated calls for the same `(app, kind, name)` return
/// the same `ResourceInstance`, which is required for barrier replay to work
/// correctly across multiple `run_operation` passes in the same test or
/// runtime session.
///
/// Because `TestWorldOracle` matches on `(kind, name)` rather than the full
/// `ResourceInstance`, the UUIDs never cause spurious mismatches when the
/// oracle is keyed with a separately-created instance.
pub struct EphemeralInstanceRegistry {
    cache: Mutex<HashMap<InstanceRegistryKey, ResourceInstance>>,
}

impl EphemeralInstanceRegistry {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for EphemeralInstanceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InstanceRegistry for EphemeralInstanceRegistry {
    fn get_or_create_singleton(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError> {
        let key: InstanceRegistryKey = (app.to_owned(), kind, name.map(|s| s.to_owned()));
        let mut cache = self.cache.lock();
        if let Some(instance) = cache.get(&key) {
            return Ok(instance.clone());
        }
        let instance = match name {
            Some(n) => ResourceInstance::new_singleton(app, kind, n),
            None => ResourceInstance::new_anonymous(app, kind),
        };
        cache.insert(key, instance.clone());
        Ok(instance)
    }
}

// ---------------------------------------------------------------------------
// DbInstanceRegistry
// ---------------------------------------------------------------------------

/// Looks instances up in the SQLite instance registry, creating and persisting
/// new ones when none exist for the requested `(app, kind, name)` group.
pub struct DbInstanceRegistry {
    db: Arc<Mutex<Db>>,
}

impl DbInstanceRegistry {
    pub fn new(db: Arc<Mutex<Db>>) -> Self {
        Self { db }
    }
}

impl InstanceRegistry for DbInstanceRegistry {
    fn get_or_create_singleton(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError> {
        let db = self.db.lock();
        Ok(history::get_or_create_singleton(&db, app, kind, name)?)
    }
}
