use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use seedling_protocol::names::AppName;

use crate::defs::app::AppDef;
use crate::defs::resource::{Resource, ResourceId, ResourceKind};
use crate::runtime::barrier::{ActionLogEntry, CallKind};
use crate::runtime::db::Db;
use crate::runtime::identity::ResourceInstance;
use crate::runtime::lifecycle::LifecycleState;
use crate::runtime::stopped::StoppedSet;
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
/// the desired state so far, as directed by `rt.start()` and `rt.stop()`
/// calls in the action closure.
#[derive(Debug, Default, Clone)]
pub struct OperationProgress {
    resources: HashMap<ResourceInstance, LifecycleState>,
    /// Definitions of dynamic resources started during the current operation pass.
    /// These are resources created anonymously inside an action closure that are
    /// not present in the static AppDef. Repopulated on each pass of run_operation.
    pub dynamic_defs: HashMap<ResourceInstance, Resource>,
    /// Hostnames whose certs should be pre-warmed (via `rt.warm_certs`) without
    /// routing traffic to them. Read by the proxy reconciler to emit
    /// `tls.certificates.automate` entries; not part of the standard desired
    /// state. Stored by ingress resource name.
    // r[impl actuate.ingress.warm-certs]
    pub warm_cert_hostnames: BTreeSet<String>,
}

impl OperationProgress {
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
            dynamic_defs: HashMap::new(),
            warm_cert_hostnames: BTreeSet::new(),
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
            warm_cert_hostnames: BTreeSet::new(),
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
                // r[impl actuate.ingress.warm-certs]
                // WarmCerts records intent in a separate map; it does not put
                // resources into the standard desired state, so the reconciler
                // will not actuate them as fully Ready (which would route
                // traffic). The Caddy reconciler reads warm_cert_hostnames
                // separately to push cert-only proxy entries.
                CallKind::WarmCerts => {
                    for r in &entry.resources {
                        if let Some(name) = r.name.as_deref() {
                            this.warm_cert_hostnames.insert(name.to_owned());
                        }
                    }
                }
                // r[impl actuate.image.warm]
                // WarmImages has no effect on computed desired state. The
                // pin rows written at call time drive pull reconciliation;
                // no resources need to be marked Ready for it.
                CallKind::WarmImages => {}
                // l[impl rt.signal]
                // Signal is a transient side-effect; it does not affect the
                // computed desired state.
                CallKind::Signal => {}
                // l[impl rt.write]
                // Write is a transient side-effect on a volume's contents; it
                // does not affect any resource's lifecycle, so it does not
                // contribute to the computed desired state.
                CallKind::Write => {}
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
    app_name: &AppName,
    app_def: &AppDef,
    operation_progress: Option<&OperationProgress>,
    registry: &dyn InstanceRegistry,
    effective_scales: &EffectiveScales,
    stopped: &StoppedSet,
) -> Result<DesiredState, RegistryError> {
    match operation_progress {
        None => compute_steady(app_name, app_def, registry, effective_scales, stopped),
        Some(progress) => Ok(compute_during_operation(app_def, progress)),
    }
}

/// Compute the desired state for an app that is being uninstalled.
/// All resources are desired at `Unscheduled`.
pub fn compute_uninstalling(
    app_name: &AppName,
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
fn compute_steady(
    app_name: &AppName,
    app_def: &AppDef,
    registry: &dyn InstanceRegistry,
    effective_scales: &EffectiveScales,
    stopped: &StoppedSet,
) -> Result<DesiredState, RegistryError> {
    let mut resources = Vec::new();

    for (id, resource) in &app_def.resources {
        let is_stopped = stopped.contains(&(id.kind, id.name.as_str().to_owned()));

        if id.kind == ResourceKind::Deployment
            && let Some(&(_low, _high, effective)) = effective_scales.get(id.name.as_str())
        {
            // i[impl resource.stop]
            // r[impl autonomous.scale]
            let scale = if is_stopped { 0 } else { effective };
            let group =
                registry.ensure_scaled_group(app_name, id.kind, Some(id.name.as_str()), scale)?;
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
        // i[impl resource.stop]
        let desired = if is_stopped {
            LifecycleState::Unscheduled
        } else {
            LifecycleState::Ready
        };
        resources.push(DesiredResource {
            instance: inst,
            desired,
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
    pub app: AppName,
    pub operation_id: String,
    pub kind: String,
    pub display_name: String,
    /// BSL-level resource name; `None` for anonymous resources.
    pub resource_name: Option<String>,
    /// Free-form description set via `resource.description()`.
    // l[impl bsl.resource.description]
    pub description: Option<String>,
}

/// Persist a dynamic resource so it survives restarts.
pub fn insert_dynamic_resource(
    db: &Db,
    instance: &ResourceInstance,
    operation_id: &str,
    description: Option<&str>,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT OR REPLACE INTO dynamic_resources
             (instance_id, app, operation_id, kind, display_name, resource_name, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            instance.id.to_hex(),
            instance.app,
            operation_id,
            format!("{:?}", instance.kind),
            instance.display_name,
            instance.name,
            description,
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
        "SELECT instance_id, app, operation_id, kind, display_name, resource_name, description
         FROM dynamic_resources ORDER BY app, instance_id",
    )?;
    collect_dynamic_rows(stmt.query_map([], parse_dynamic_row)?)
}

/// Load dynamic resource records for a single app.
pub fn list_dynamic_resources_for_app(
    db: &Db,
    app: &AppName,
) -> rusqlite::Result<Vec<DynamicResourceRecord>> {
    let mut stmt = db.conn.prepare(
        "SELECT instance_id, app, operation_id, kind, display_name, resource_name, description
         FROM dynamic_resources WHERE app = ?1 ORDER BY instance_id",
    )?;
    collect_dynamic_rows(stmt.query_map([app], parse_dynamic_row)?)
}

fn parse_dynamic_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DynamicResourceRecord> {
    Ok(DynamicResourceRecord {
        instance_id: row.get(0)?,
        app: row.get::<_, AppName>(1)?,
        operation_id: row.get(2)?,
        kind: row.get(3)?,
        display_name: row.get(4)?,
        resource_name: row.get(5)?,
        description: row.get(6)?,
    })
}

fn collect_dynamic_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<DynamicResourceRecord>,
    >,
) -> rusqlite::Result<Vec<DynamicResourceRecord>> {
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

#[cfg(test)]
mod tests;
