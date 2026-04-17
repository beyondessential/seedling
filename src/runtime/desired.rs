use std::collections::HashMap;
use std::sync::Arc;

use crate::defs::app::AppDef;
use crate::defs::resource::{Resource, ResourceId, ResourceKind};
use crate::runtime::barrier::{ActionLogEntry, CallKind};
use crate::runtime::db::Db;
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;
use crate::runtime::{InstanceRegistry, RegistryError};

/// Pre-computed effective scales for all deployments in an app.
///
/// Maps deployment name to `(low, high, effective)` where `effective` is the
/// stored scaling decision clamped to bounds (or the lower bound when no
/// decision exists).
pub type EffectiveScales = HashMap<String, (u16, u16, u16)>;

// r[impl desired-state.definition]
#[derive(Debug)]
pub struct DesiredResource {
    pub instance: ResourceInstance,
    pub desired: LifecycleState,
    pub definition: Resource,
}

// r[impl desired-state.definition]
#[derive(Debug, Default)]
pub struct DesiredState {
    pub resources: Vec<DesiredResource>,
}

impl DesiredState {
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

/// Records the resources an in-progress lifecycle operation has placed into
/// the desired state so far, as directed by `rt.start()`, `rt.stop()`, and
/// `rt.reconcile()` calls in the action closure.
#[derive(Debug, Default, Clone)]
pub struct OperationProgress {
    resources: HashMap<ResourceInstance, LifecycleState>,
    /// Definitions of dynamic resources started during the current operation pass.
    /// These are resources created anonymously inside an action closure that are
    /// not present in the static AppDef. Repopulated on each pass of run_operation.
    pub dynamic_defs: HashMap<ResourceInstance, Resource>,
}

impl OperationProgress {
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
            dynamic_defs: HashMap::new(),
        }
    }

    /// Mark a resource as explicitly started (desired state: `Ready`).
    pub fn started(&mut self, resource: ResourceInstance) {
        self.resources.insert(resource, LifecycleState::Ready);
    }

    /// Mark a dynamic (anonymous) resource as started and record its definition.
    pub fn started_dynamic(&mut self, instance: ResourceInstance, def: Resource) {
        self.resources
            .insert(instance.clone(), LifecycleState::Ready);
        self.dynamic_defs.insert(instance, def);
    }

    /// Mark a resource as explicitly stopped (desired state: `Unscheduled`).
    pub fn stopped(&mut self, resource: ResourceInstance) {
        self.resources.insert(resource, LifecycleState::Unscheduled);
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Build from a slice of action log entries.
    ///
    /// `Start` entries map to desired state `Ready`.
    /// `Stop` entries map to desired state `Unscheduled`.
    /// `Query` entries are ignored; they do not affect the desired state.
    ///
    /// Later entries for the same resource override earlier ones.
    pub fn from_log(entries: &[ActionLogEntry]) -> Self {
        let mut this = Self {
            resources: HashMap::new(),
            dynamic_defs: HashMap::new(),
        };
        for entry in entries {
            match entry.call_kind {
                CallKind::Start => {
                    for r in &entry.resources {
                        this.started(r.clone());
                    }
                }
                CallKind::Stop => {
                    for r in &entry.resources {
                        this.stopped(r.clone());
                    }
                }
                CallKind::Query => {}
            }
        }
        this
    }
}

/// Compute the desired state for an application.
///
/// When `operation_progress` is `None` the full AppDef contributes all
/// resources at desired state `Ready` (steady-state maintenance).
///
/// When an operation is in progress, only resources the action closure has
/// explicitly placed into the desired state are included.
// r[impl desired-state.definition]
pub fn compute(
    app_name: &str,
    app_def: &AppDef,
    operation_progress: Option<&OperationProgress>,
    registry: &dyn InstanceRegistry,
    effective_scales: &EffectiveScales,
) -> Result<DesiredState, RegistryError> {
    match operation_progress {
        None => compute_steady(app_name, app_def, registry, effective_scales),
        Some(progress) => Ok(compute_during_operation(app_def, progress)),
    }
}

/// Compute the desired state for an app that is being uninstalled.
/// All resources are desired at `Unscheduled`.
pub fn compute_uninstalling(
    app_name: &str,
    app_def: &AppDef,
    registry: &dyn InstanceRegistry,
) -> Result<DesiredState, RegistryError> {
    let mut resources = Vec::new();
    for (id, resource) in &app_def.resources {
        // During uninstall, find every existing instance and mark it Unscheduled.
        let all = registry.find_all_instances(app_name, id.kind, Some(id.name.as_str()))?;
        if all.is_empty() {
            // No instances at all — nothing to tear down for this resource.
            continue;
        }
        for inst in all {
            resources.push(DesiredResource {
                instance: inst,
                desired: LifecycleState::Unscheduled,
                definition: resource.clone(),
            });
        }
    }
    Ok(DesiredState { resources })
}

// r[impl desired-state.steady]
// r[impl autonomous.scale]
fn compute_steady(
    app_name: &str,
    app_def: &AppDef,
    registry: &dyn InstanceRegistry,
    effective_scales: &EffectiveScales,
) -> Result<DesiredState, RegistryError> {
    let mut resources = Vec::new();

    for (id, resource) in &app_def.resources {
        if id.kind == ResourceKind::Deployment
            && let Some(&(_low, _high, effective)) = effective_scales.get(id.name.as_str())
        {
            let group = registry.ensure_scaled_group(
                app_name,
                id.kind,
                Some(id.name.as_str()),
                effective,
            )?;
            for inst in group.keep {
                resources.push(DesiredResource {
                    instance: inst,
                    desired: LifecycleState::Ready,
                    definition: resource.clone(),
                });
            }
            for inst in group.excess {
                resources.push(DesiredResource {
                    instance: inst,
                    desired: LifecycleState::Unscheduled,
                    definition: resource.clone(),
                });
            }
            continue;
        }

        // Non-deployment resources: singleton.
        let inst = registry.get_or_create_singleton(app_name, id.kind, Some(id.name.as_str()))?;
        resources.push(DesiredResource {
            instance: inst,
            desired: LifecycleState::Ready,
            definition: resource.clone(),
        });
    }

    Ok(DesiredState { resources })
}

// r[impl desired-state.during-operation]
fn compute_during_operation(app_def: &AppDef, progress: &OperationProgress) -> DesiredState {
    let resources = progress
        .resources
        .iter()
        .filter_map(|(instance, &desired)| {
            let definition = lookup_definition(app_def, instance)
                .or_else(|| progress.dynamic_defs.get(instance).cloned())?;
            Some(DesiredResource {
                instance: instance.clone(),
                desired,
                definition,
            })
        })
        .collect();
    DesiredState { resources }
}

fn lookup_definition(app_def: &AppDef, instance: &ResourceInstance) -> Option<Resource> {
    let name = Arc::new(instance.name.as_deref().unwrap_or("").to_owned());
    let id = ResourceId {
        kind: instance.kind,
        name,
    };
    app_def.resources.get(&id).cloned()
}

/// A persisted dynamic resource record.
pub struct DynamicResourceRecord {
    pub instance_id: String,
    pub app: String,
    pub operation_id: String,
    pub kind: String,
    pub display_name: String,
}

/// Persist a dynamic resource so it survives restarts.
pub fn insert_dynamic_resource(
    db: &Db,
    instance: &ResourceInstance,
    operation_id: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO dynamic_resources (instance_id, app, operation_id, kind, display_name)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            instance.id.to_hex(),
            instance.app,
            operation_id,
            format!("{:?}", instance.kind),
            instance.display_name,
        ],
    )?;
    Ok(())
}

/// Remove a dynamic resource record after cleanup.
pub fn delete_dynamic_resource(db: &Db, instance_id: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM dynamic_resources WHERE instance_id = ?1",
        [instance_id],
    )?;
    Ok(())
}

/// Remove all dynamic resources for an operation.
pub fn delete_dynamic_resources_for_operation(db: &Db, operation_id: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM dynamic_resources WHERE operation_id = ?1",
        [operation_id],
    )?;
    Ok(())
}

/// Load all dynamic resource records (e.g., for startup orphan cleanup).
pub fn list_dynamic_resources(db: &Db) -> rusqlite::Result<Vec<DynamicResourceRecord>> {
    let mut stmt = db.conn.prepare(
        "SELECT instance_id, app, operation_id, kind, display_name
         FROM dynamic_resources ORDER BY app, instance_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DynamicResourceRecord {
            instance_id: row.get(0)?,
            app: row.get(1)?,
            operation_id: row.get(2)?,
            kind: row.get(3)?,
            display_name: row.get(4)?,
        })
    })?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

#[cfg(test)]
mod tests;
