use std::collections::HashMap;

use parking_lot::Mutex;
use seedling_protocol::names::AppName;

use crate::defs::resource::ResourceKind;
use crate::runtime::db::DbHandle;
use crate::runtime::gc::UNSCHEDULED_OBS_KINDS;
use crate::runtime::history;
use crate::runtime::identity::{InstanceVariant, ResourceInstance};
use crate::runtime::lifecycle::LifecycleState;

#[cfg(test)]
mod tests;

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

impl RegistryError {
    /// Build a `RegistryError` from a free-form message. Useful when
    /// surfacing script-level usage errors (e.g. an unsupported value
    /// passed to `rt.start`) through the same channel as registry
    /// failures, since the call sites already map both to the same
    /// `EvalAltResult` string.
    pub fn message(msg: impl Into<String>) -> Self {
        Self(Box::<dyn std::error::Error + Send + Sync>::from(msg.into()))
    }
}

// spec: GC#instances-started-by-actions
/// Whether a group may take an instance that has already been torn down back
/// into its keep set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceReuse {
    /// Reuse any instance recorded for the group.
    ///
    /// What the reconciler resolves steady-state desired state with. An
    /// instance whose container has exited is still that replica, and
    /// re-identifying it every tick would lose the restart record and the
    /// observation history kept against its identity.
    Any,
    /// Leave instances whose observations say they have been torn down out
    /// of the keep set, allocating fresh ones in their place.
    ///
    /// What an operation resolves its groups with. Those are precisely the
    /// instances the reconciler's retirement sweep deletes, so an operation
    /// that does not hold any of them cannot have its keep set deleted out
    /// from under it.
    Live,
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
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError>;

    /// Return exactly `count` scaled instances for the given group, creating
    /// new ones if fewer than `count` exist, and returning any excess.
    /// Existing instances are kept in creation order (oldest first), subject
    /// to `reuse`.
    fn ensure_scaled_group(
        &self,
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
        reuse: InstanceReuse,
    ) -> Result<ScaledGroup, RegistryError>;

    /// Return all existing instances (singleton + scaled) for the group.
    /// Does not create anything. Used during uninstall to find everything
    /// that needs to be torn down.
    fn find_all_instances(
        &self,
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError>;
}

// ---------------------------------------------------------------------------
// EphemeralInstanceRegistry
// ---------------------------------------------------------------------------

type InstanceRegistryKey = (AppName, ResourceKind, Option<String>);

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
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError> {
        let key: InstanceRegistryKey = (app.clone(), kind, name.map(|s| s.to_owned()));
        let mut cache = self.cache.lock();
        if let Some(instance) = cache.get(&key) {
            return Ok(instance.clone());
        }
        let instance = match name {
            Some(n) => ResourceInstance::new_singleton(app.clone(), kind, n),
            None => ResourceInstance::new_anonymous(app.clone(), kind),
        };
        cache.insert(key, instance.clone());
        Ok(instance)
    }

    /// `reuse` has no bearing here: this registry keeps no observations, so
    /// it has no record of an instance having been torn down.
    fn ensure_scaled_group(
        &self,
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
        _reuse: InstanceReuse,
    ) -> Result<ScaledGroup, RegistryError> {
        let key: InstanceRegistryKey = (app.clone(), kind, name.map(|s| s.to_owned()));
        let mut scaled_cache = self.scaled_cache.lock();
        let instances = scaled_cache.entry(key.clone()).or_default();

        let count = usize::from(count);
        while instances.len() < count {
            let instance = ResourceInstance::new_scaled(app.clone(), kind, name.unwrap_or(""));
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
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError> {
        let key: InstanceRegistryKey = (app.clone(), kind, name.map(|s| s.to_owned()));
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
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<ResourceInstance, RegistryError> {
        let app = app.clone();
        let name = name.map(|s| s.to_owned());
        Ok(self
            .db
            .call(move |db| history::get_or_create_singleton(db, &app, kind, name.as_deref()))?)
    }

    // spec: AUTO#health-check-replacement
    fn ensure_scaled_group(
        &self,
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
        count: u16,
        reuse: InstanceReuse,
    ) -> Result<ScaledGroup, RegistryError> {
        let app = app.clone();
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

            // Read the group's observations in one query: the keep order
            // is drawn from them, and so is whether each instance has
            // already been torn down. Asking per instance put a prepared
            // statement and a round trip between every replica, on each
            // reconcile tick and each pass of every action that names the
            // deployment.
            //
            // A failed read is an error rather than an empty history, which
            // would say both "not ready" and, worse, "not torn down" —
            // handing the keep set an instance on the strength of a query
            // that did not answer.
            let observations = history::query_observations_for(db, &scaled)?;
            let mut observed: Vec<(ResourceInstance, LifecycleState, bool)> =
                Vec::with_capacity(scaled.len());
            for inst in scaled {
                let obs = observations.get(&inst.id).map_or(&[][..], Vec::as_slice);
                let state = crate::runtime::barrier::oracle::derive_lifecycle_state(&inst, obs);
                // Latest wins, because that is what the retirement sweep
                // this mirrors does: it selects on the most recent
                // observation being a terminal kind. Asking whether a
                // terminal observation appears anywhere in the history
                // instead would mark an instance torn down for the rest of
                // its life the first time its container was removed, so a
                // replica that had since been brought back would be dropped
                // from the keep set while it was still running — and a
                // `rt.stop` would leave it serving, which is the split
                // AUTO#scale exists to prevent.
                //
                // The observations are ordered by recorded_at then id, so
                // the last is the most recent.
                let latest_is_terminal = obs
                    .last()
                    .is_some_and(|o| UNSCHEDULED_OBS_KINDS.contains(&o.obs_kind.as_str()));
                let torn_down = state == LifecycleState::Unscheduled || latest_is_terminal;
                observed.push((inst, state, torn_down));
            }

            // spec: GC#instances-started-by-actions
            // An instance that has been torn down is one the retirement
            // sweep deletes, so a caller that must not have its keep set
            // deleted under it does not take one. Set them aside before the
            // count is filled, so fresh instances are minted in their place
            // rather than the group coming up short.
            let mut retired = Vec::new();
            if reuse == InstanceReuse::Live {
                let (live, torn_down): (Vec<_>, Vec<_>) =
                    observed.into_iter().partition(|(_, _, torn)| !torn);
                retired = torn_down.into_iter().map(|(i, _, _)| i).collect();
                observed = live;
            }

            let count = usize::from(count);
            while observed.len() < count {
                let instance =
                    ResourceInstance::new_scaled(app.clone(), kind, name.as_deref().unwrap_or(""));
                history::insert_instance(db, &instance)?;
                observed.push((instance, LifecycleState::Pending, false));
            }

            // Sort the scaled instances by (currently-Ready desc, original
            // order asc) so that the keep set prefers healthy instances. Ties
            // (e.g. all healthy or all not-yet-healthy) fall back to creation
            // order, which is the existing behaviour.
            //
            // The original order produced by `find_instances_for_group` is
            // ASC by `created_at`, so a stable sort_by_key preserves that as
            // the secondary criterion when the primary is tied.
            //
            // Stable sort: false (not ready) > true (ready) in usize, so
            // negate by sorting on `!is_ready`.
            observed.sort_by_key(|(_, state, _)| *state != LifecycleState::Ready);
            let scaled: Vec<ResourceInstance> = observed.into_iter().map(|(i, _, _)| i).collect();

            let keep = scaled[..count].to_vec();
            let mut excess = scaled[count..].to_vec();
            // The instances left out of the keep set are still the group's
            // to tear down.
            excess.extend(retired);

            // Lingering singletons are always excess — the reconciler will
            // unschedule them so the old container is torn down cleanly.
            excess.extend(singletons);

            Ok(ScaledGroup { keep, excess })
        })?)
    }

    fn find_all_instances(
        &self,
        app: &AppName,
        kind: ResourceKind,
        name: Option<&str>,
    ) -> Result<Vec<ResourceInstance>, RegistryError> {
        let app = app.clone();
        let name = name.map(|s| s.to_owned());
        Ok(self
            .db
            .call(move |db| history::find_instances_for_group(db, &app, kind, name.as_deref()))?)
    }
}
