use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use seedling_protocol::names::AppName;

use crate::defs::app::AppDef;
use crate::defs::deployment::Deployment;
use crate::defs::job::Job;
use crate::defs::resource::{Resource, ResourceId, ResourceKind};
use crate::runtime::barrier::{ActionLogEntry, CallKind};
use crate::runtime::db::Db;
use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};
use crate::runtime::lifecycle::LifecycleState;
use crate::runtime::stopped::StoppedSet;
use crate::runtime::{InstanceRegistry, RegistryError};

/// Pre-computed effective scales for all deployments in an app.
///
/// Maps deployment name to `(low, high, effective)` where `effective` is the
/// stored scaling decision clamped to bounds (or the lower bound when no
/// decision exists).
pub type EffectiveScales = HashMap<String, (u16, u16, u16)>;

// spec: RCN#desired-state
#[derive(Debug)]
pub struct DesiredResource {
    pub instance: ResourceInstance,
    pub desired: LifecycleState,
    pub definition: Resource,
    /// The instance has been demoted out of its scaled group: an excess
    /// replica, a singleton left behind by a deployment that has since become
    /// scaled, or a replica an operation stopped. Only a demoted instance may
    /// be retired; every other entry is part of the active desired state
    /// whatever its lifecycle state, including a stopped singleton.
    pub demoted: bool,
}

// spec: RCN#desired-state
#[derive(Debug, Default)]
pub struct DesiredState {
    pub resources: Vec<DesiredResource>,
    /// Recorded instances of an uninstalling app whose resource no definition
    /// declares any more: one an update dropped, or one an operation created.
    /// Each is held `Unscheduled`, and is torn down from its recorded identity
    /// alone, there being no declared configuration to act on.
    // spec: RCN#desired-state
    // spec: TRD#teardown
    pub undeclared: Vec<ResourceInstance>,
}

impl DesiredState {
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty() && self.undeclared.is_empty()
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
    // spec: RT#certificate-warming
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

    /// Drop every resource that is not dynamic.
    // spec: RT#starting-resources
    pub fn retain_dynamic(&mut self) {
        self.resources.retain(|instance, _| !instance.is_declared());
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
                // spec: RT#certificate-warming
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
                // spec: IMG#image-warming
                // WarmImages has no effect on computed desired state. The
                // pin rows written at call time drive pull reconciliation;
                // no resources need to be marked Ready for it.
                CallKind::WarmImages => {}
                // spec: RT#signal-delivery
                // Signal is a transient side-effect; it does not affect the
                // computed desired state.
                CallKind::Signal => {}
                // spec: RT#volume-writes
                // Write is a transient side-effect on a volume's contents; it
                // does not affect any resource's lifecycle, so it does not
                // contribute to the computed desired state.
                CallKind::Write => {}
                // spec: RT#command-execution
                // Exec is a transient side-effect inside a running container;
                // it does not affect the computed desired state.
                CallKind::Exec => {}
                // spec: OPS#action-composition
                // SubAction is a frame marker for a nested Action.call.
                // The actual rt.* calls inside the called closure follow as
                // their own log entries and contribute to desired state via
                // the matching arms above.
                CallKind::SubAction => {}
                // spec: RT#deployment-restart
                // A restart changes no resource's desired state.
                CallKind::Restart => {}
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
// spec: RCN#desired-state
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

/// Compute the desired state for an app that is being uninstalled: every
/// instance recorded for it, each held `Unscheduled`.
///
/// An instance of a resource the definition still declares carries that
/// declaration. Any other is undeclared, and is torn down from its recorded
/// identity.
// spec: RCN#desired-state
// spec: TRD#teardown
pub fn compute_uninstalling(
    app_name: &AppName,
    app_def: &AppDef,
    registry: &dyn InstanceRegistry,
) -> Result<DesiredState, RegistryError> {
    let mut state = DesiredState::default();
    for instance in registry.instances_of_app(app_name)? {
        match lookup_definition(app_def, &instance) {
            Some(definition) => state.resources.push(DesiredResource {
                instance,
                desired: LifecycleState::Unscheduled,
                definition,
                // Uninstall tears the whole app down through its own phase.
                demoted: false,
            }),
            None => state.undeclared.push(instance),
        }
    }
    Ok(state)
}

/// The definition a pod instance nothing declares is stopped with.
///
/// It carries no configuration, so stopping the instance acts on its
/// recorded identity alone: its unit, its container and its pod network.
/// `None` for an instance that is not a pod.
// spec: TRD#teardown
pub fn undeclared_pod(instance: &ResourceInstance) -> Option<Resource> {
    let name = Arc::new(instance.name.clone().unwrap_or_default());
    let anon_id = instance.name.is_none().then_some(instance.id);
    match instance.kind {
        ResourceKind::Deployment => Some(Resource::Deployment(Deployment {
            name,
            def: Default::default(),
            frozen: true,
            anon_id,
        })),
        ResourceKind::Job => Some(Resource::Job(Job {
            name,
            def: Default::default(),
            frozen: true,
            anon_id,
        })),
        _ => None,
    }
}

// spec: RCN#desired-state
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
            // spec: CTL#resource-stop
            // spec: AUTO#scale
            let scale = if is_stopped { 0 } else { effective };
            // Steady-state desired state keeps an instance whatever its
            // containers have done: re-identifying it would lose the restart
            // record and observation history held against its identity.
            let group = registry.ensure_scaled_group(
                app_name,
                id.kind,
                Some(id.name.as_str()),
                scale,
                crate::runtime::registry::InstanceReuse::Any,
            )?;
            for inst in group.keep {
                resources.push(DesiredResource {
                    instance: inst,
                    desired: LifecycleState::Ready,
                    definition: resource.clone(),
                    demoted: false,
                });
            }
            for inst in group.excess {
                resources.push(DesiredResource {
                    instance: inst,
                    desired: LifecycleState::Unscheduled,
                    definition: resource.clone(),
                    demoted: true,
                });
            }
            continue;
        }

        // Non-deployment resources: singleton.
        let inst = registry.get_or_create_singleton(app_name, id.kind, Some(id.name.as_str()))?;
        // spec: CTL#resource-stop
        let desired = if is_stopped {
            LifecycleState::Unscheduled
        } else {
            LifecycleState::Ready
        };
        resources.push(DesiredResource {
            instance: inst,
            desired,
            definition: resource.clone(),
            demoted: false,
        });
    }

    Ok(DesiredState {
        resources,
        undeclared: Vec::new(),
    })
}

// spec: RCN#desired-state
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
                demoted: instance.variant == InstanceVariant::Scaled
                    && desired == LifecycleState::Unscheduled,
            })
        })
        .collect();
    DesiredState {
        resources,
        undeclared: Vec::new(),
    }
}

/// Add what the shell sessions open on an app have placed in the desired
/// state to `state`, beside whatever else it holds.
///
/// A session only ever places dynamic instances, which no operation's
/// progress and no steady state holds, so its entries cannot clash with
/// theirs. Where `hold_stopped`, as for an app being uninstalled or no longer
/// installed, each is held `Unscheduled` whatever the session last put it in.
// spec: RCN#desired-state
pub fn merge_sessions(
    state: &mut DesiredState,
    app_def: &AppDef,
    sessions: &[&OperationProgress],
    hold_stopped: bool,
) {
    let mut held: HashSet<InstanceId> = state
        .resources
        .iter()
        .map(|resource| resource.instance.id)
        .chain(state.undeclared.iter().map(|instance| instance.id))
        .collect();
    for progress in sessions {
        for mut resource in compute_during_operation(app_def, progress).resources {
            if !held.insert(resource.instance.id) {
                continue;
            }
            if hold_stopped {
                resource.desired = LifecycleState::Unscheduled;
            }
            state.resources.push(resource);
        }
    }
}

fn lookup_definition(app_def: &AppDef, instance: &ResourceInstance) -> Option<Resource> {
    let name = Arc::new(instance.name.as_deref().unwrap_or("").to_owned());
    let id = ResourceId {
        kind: instance.kind,
        name,
    };
    app_def.resources.get(&id).cloned()
}

/// What created a dynamic resource, and so what its record's `operation_id`
/// names.
// spec: APP#dynamic-resource-lifetime
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicCreator {
    /// A lifecycle operation; the record holds the operation's id.
    Operation,
    /// A shell session; the record holds the session's id.
    ShellSession,
}

/// A persisted dynamic resource record.
pub struct DynamicResourceRecord {
    pub instance_id: String,
    pub app: AppName,
    /// The id of the operation or shell session that created the resource, as
    /// `creator` says.
    pub operation_id: String,
    pub creator: DynamicCreator,
    pub kind: String,
    pub display_name: String,
    /// BSL-level resource name; `None` for anonymous resources.
    pub resource_name: Option<String>,
    /// Free-form description set via `resource.description()`.
    // spec: BSL#descriptions
    pub description: Option<String>,
}

/// Persist a dynamic resource so it survives restarts.
pub fn insert_dynamic_resource(
    db: &Db,
    instance: &ResourceInstance,
    operation_id: &str,
    creator: DynamicCreator,
    description: Option<&str>,
) -> rusqlite::Result<()> {
    db.conn.execute(
        // spec: INFRA#database
        "INSERT INTO dynamic_resources
             (instance_id, app, operation_id, shell_session, kind, display_name, resource_name,
              description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(instance_id) DO UPDATE SET
             app = excluded.app,
             operation_id = excluded.operation_id,
             shell_session = excluded.shell_session,
             kind = excluded.kind,
             display_name = excluded.display_name,
             resource_name = excluded.resource_name,
             description = excluded.description",
        rusqlite::params![
            instance.id.to_hex(),
            instance.app,
            operation_id,
            creator == DynamicCreator::ShellSession,
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

/// Load all dynamic resource records (e.g., for startup orphan cleanup).
pub fn list_dynamic_resources(db: &Db) -> rusqlite::Result<Vec<DynamicResourceRecord>> {
    let mut stmt = db.conn.prepare(
        "SELECT instance_id, app, operation_id, shell_session, kind, display_name, resource_name,
                description
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
        "SELECT instance_id, app, operation_id, shell_session, kind, display_name, resource_name,
                description
         FROM dynamic_resources WHERE app = ?1 ORDER BY instance_id",
    )?;
    collect_dynamic_rows(stmt.query_map([app], parse_dynamic_row)?)
}

/// Load the dynamic resource records recorded against one operation.
// spec: SUSP#abandoned-replay
pub fn list_dynamic_resources_for_operation(
    db: &Db,
    operation_id: &str,
) -> rusqlite::Result<Vec<DynamicResourceRecord>> {
    let mut stmt = db.conn.prepare(
        "SELECT instance_id, app, operation_id, shell_session, kind, display_name, resource_name,
                description
         FROM dynamic_resources WHERE operation_id = ?1 ORDER BY instance_id",
    )?;
    collect_dynamic_rows(stmt.query_map([operation_id], parse_dynamic_row)?)
}

fn parse_dynamic_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DynamicResourceRecord> {
    Ok(DynamicResourceRecord {
        instance_id: row.get(0)?,
        app: row.get::<_, AppName>(1)?,
        operation_id: row.get(2)?,
        creator: if row.get::<_, bool>(3)? {
            DynamicCreator::ShellSession
        } else {
            DynamicCreator::Operation
        },
        kind: row.get(4)?,
        display_name: row.get(5)?,
        resource_name: row.get(6)?,
        description: row.get(7)?,
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
