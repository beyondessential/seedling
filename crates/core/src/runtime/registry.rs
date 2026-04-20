use std::collections::HashMap;

use parking_lot::Mutex;

use crate::defs::resource::ResourceKind;
use crate::runtime::db::{Db, DbHandle};
use crate::runtime::history;
use crate::runtime::identity::{InstanceVariant, ResourceInstance};

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

/// The result of resolving a scaled deployment group.
pub struct ScaledGroup {
    /// Instances that should be kept (length == the requested count).
    pub keep: Vec<ResourceInstance>,
    /// Pre-existing instances beyond the requested count that should be removed.
    pub excess: Vec<ResourceInstance>,
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

    /// Return exactly `count` scaled instances for the given group, creating
    /// new ones if fewer than `count` exist, and returning any excess.
    /// Existing instances are kept in creation order (oldest first).
    fn ensure_scaled_group(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
    ) -> Result<ScaledGroup, RegistryError>;

    /// Return all existing instances (singleton + scaled) for the group.
    /// Does not create anything. Used during uninstall to find everything
    /// that needs to be torn down.
    fn find_all_instances(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError>;
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
    scaled_cache: Mutex<HashMap<InstanceRegistryKey, Vec<ResourceInstance>>>,
}

impl EphemeralInstanceRegistry {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            scaled_cache: Mutex::new(HashMap::new()),
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

    fn ensure_scaled_group(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
    ) -> Result<ScaledGroup, RegistryError> {
        let key: InstanceRegistryKey = (app.to_owned(), kind, name.map(|s| s.to_owned()));
        let mut scaled_cache = self.scaled_cache.lock();
        let instances = scaled_cache.entry(key.clone()).or_default();

        let count = usize::from(count);
        while instances.len() < count {
            let instance = ResourceInstance::new_scaled(app, kind, name.unwrap_or(""));
            instances.push(instance);
        }

        let keep = instances[..count].to_vec();
        let mut excess = instances[count..].to_vec();

        // Any lingering singleton for the same group is stale and must be
        // torn down (e.g. after a scale definition changes from 1..1 to a
        // range).
        let cache = self.cache.lock();
        if let Some(singleton) = cache.get(&key) {
            excess.push(singleton.clone());
        }

        Ok(ScaledGroup { keep, excess })
    }

    fn find_all_instances(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError> {
        let key: InstanceRegistryKey = (app.to_owned(), kind, name.map(|s| s.to_owned()));
        let mut result = Vec::new();

        let cache = self.cache.lock();
        if let Some(singleton) = cache.get(&key) {
            result.push(singleton.clone());
        }
        drop(cache);

        let scaled_cache = self.scaled_cache.lock();
        if let Some(scaled) = scaled_cache.get(&key) {
            result.extend(scaled.iter().cloned());
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// DbInstanceRegistry
// ---------------------------------------------------------------------------

/// Looks instances up in the SQLite instance registry, creating and persisting
/// new ones when none exist for the requested `(app, kind, name)` group.
pub struct DbInstanceRegistry {
    db: DbHandle,
}

impl DbInstanceRegistry {
    pub fn new(db: DbHandle) -> Self {
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
        let app = app.to_owned();
        let name = name.map(|s| s.to_owned());
        Ok(self
            .db
            .call(move |db| history::get_or_create_singleton(db, &app, kind, name.as_deref()))?)
    }

    fn ensure_scaled_group(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
    ) -> Result<ScaledGroup, RegistryError> {
        let app = app.to_owned();
        let name = name.map(|s| s.to_owned());
        Ok(self.db.call(move |db| -> rusqlite::Result<ScaledGroup> {
            let existing = history::find_instances_for_group(db, &app, kind, name.as_deref())?;

            // Separate singletons (stale after a scale-definition change) from
            // scaled instances that participate in the group.
            let mut singletons = Vec::new();
            let mut scaled = Vec::new();
            for inst in existing {
                match inst.variant {
                    InstanceVariant::Singleton => singletons.push(inst),
                    InstanceVariant::Scaled => scaled.push(inst),
                }
            }

            let count = usize::from(count);
            while scaled.len() < count {
                let instance =
                    ResourceInstance::new_scaled(&app, kind, name.as_deref().unwrap_or(""));
                history::insert_instance(db, &instance)?;
                scaled.push(instance);
            }

            let keep = scaled[..count].to_vec();
            let mut excess = scaled[count..].to_vec();

            // Lingering singletons are always excess — the reconciler will
            // unschedule them so the old container is torn down cleanly.
            excess.extend(singletons);

            Ok(ScaledGroup { keep, excess })
        })?)
    }

    fn find_all_instances(
        &self,
        app: &str,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError> {
        let app = app.to_owned();
        let name = name.map(|s| s.to_owned());
        Ok(self
            .db
            .call(move |db| history::find_instances_for_group(db, &app, kind, name.as_deref()))?)
    }
}
