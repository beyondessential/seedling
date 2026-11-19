//! The gate parts of the Operator Interface: running a gate, and what
//! `/apps/show`, `/apps/restart` and `/apps/plan` read of an app's gates.

use std::{collections::BTreeSet, sync::Arc};

use seedling_protocol::{
    error::{ErrorCode, OiError},
    events::OperationTarget,
    names::{AppName, GateName},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    HandlerResult,
    actions::{self, lifecycle},
};
use crate::{
    defs::{
        app::AppDef,
        gate::AfterTarget,
        resource::{ResourceId, ResourceKind},
    },
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase,
        gates::{
            self, AppGates, GateState, GateView, Undetermined, held_resources, read_readiness,
            store, unready_targets,
        },
        stopped::{self, StoppedSet},
    },
};

#[derive(Deserialize)]
pub(crate) struct RunGateParams {
    pub app: AppName,
    pub gate: GateName,
}

/// What an app's gates are, as `/apps/show` reports them.
pub(crate) struct Standing {
    pub gates: AppGates,
    pub view: GateView,
}

/// Read the state of `def`'s gates, and for an installed app what they hold
/// and what waits on them.
///
/// What cannot be read is an error rather than an empty answer: a hold read
/// as no hold, or readiness read as nothing waiting, would tell an operator
/// that a resource is free to start when nobody checked.
// spec: APPS#description
pub(crate) fn read(
    state: &OiState,
    app: &AppName,
    def: &Arc<AppDef>,
    definition_error: Option<String>,
    installed: bool,
    stopped: StoppedSet,
) -> Result<Standing, OiError> {
    let (app, def, cipher) = (app.clone(), Arc::clone(def), Arc::clone(&state.cipher));
    let remembered = state.gate_runtime.remembered(&app);
    let standing = state.db.call(move |db| {
        let gates = gates::evaluate(
            db,
            &app,
            &def,
            &cipher,
            definition_error.as_deref(),
            &remembered,
        );
        let view = if installed {
            gates::view(db, &app, &def, &gates, &stopped, &remembered)
        } else {
            GateView::default()
        };
        Standing { gates, view }
    });
    if let Err(why) = &standing.gates.holds {
        return Err(OiError::internal(format!(
            "could not read what the app's gates hold: {why}"
        )));
    }
    if let Some(why) = &standing.view.readiness_unreadable {
        return Err(OiError::internal(format!(
            "could not read whether the app's workloads are ready: {why}"
        )));
    }
    Ok(standing)
}

fn target_json(target: &AfterTarget) -> Value {
    match target {
        AfterTarget::Gate(name) => json!({ "kind": "gate", "name": name }),
        AfterTarget::Workload(id) => json!({ "kind": id.kind.type_name(), "name": id.name }),
    }
}

/// The `gates` of `/apps/show`, in the order the script declared the gates.
///
/// `running` is the gate whose run is in progress for the app, if any.
// spec: APPS#gates
pub(crate) fn gates_json(
    def: &AppDef,
    standing: &Standing,
    running: Option<&GateName>,
) -> Vec<Value> {
    let holds = standing.gates.holds.as_ref().ok();
    gates::declared_order(def)
        .into_iter()
        .map(|(name, gate)| {
            let status = standing.gates.gates.get(name);
            json!({
                "name": name,
                "state": status
                    .map(|status| status.reported(running == Some(name)))
                    .unwrap_or("undetermined"),
                "last_success_generation": status
                    .and_then(|status| status.record.as_ref())
                    .and_then(|record| record.success_generation),
                "holds_exclusive": holds.is_some_and(|holds| holds.contains_key(name)),
                "next_run_at": status
                    .and_then(gates::GateStatus::next_run_at)
                    .map(|at| at.to_string()),
                "after": gate.after.iter().map(target_json).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// The names of the gates that hold the resource, in ascending order.
// spec: APPS#resources
pub(crate) fn held_by(standing: &Standing, id: &ResourceId) -> Vec<String> {
    standing
        .view
        .held
        .get(id)
        .map(|gates| gates.iter().map(|g| g.as_str().to_owned()).collect())
        .unwrap_or_default()
}

/// The targets a Deployment's or Job's starts wait on, or `None` for a
/// resource of any other kind.
// spec: APPS#resources
pub(crate) fn waiting_on(standing: &Standing, id: &ResourceId) -> Option<Vec<Value>> {
    matches!(id.kind, ResourceKind::Deployment | ResourceKind::Job).then(|| {
        standing
            .view
            .waiting
            .get(id)
            .map(|targets| targets.iter().map(target_json).collect())
            .unwrap_or_default()
    })
}

/// The gates that hold `id` stopped, or why that cannot be told.
// spec: CTL#deployment-restart
pub(crate) fn holders_of(
    state: &OiState,
    app: &AppName,
    def: &AppDef,
    id: &ResourceId,
) -> Result<BTreeSet<GateName>, OiError> {
    let app = app.clone();
    let holds = state
        .db
        .call(move |db| store::load_holds(db, &app))
        .map_err(|e| OiError::internal(format!("could not read what the app's gates hold: {e}")))?;
    Ok(held_resources(def, &holds).remove(id).unwrap_or_default())
}

/// The names of the gates `def` declares that would be unsatisfied under it,
/// evaluated as the runtime would, in ascending order.
///
/// A gate the runtime cannot tell is neither listed nor passed over: where its
/// record or key cannot be read the answer is an error. One that holds a valid
/// success for its key and whose check could not be run is not unsatisfied, and
/// the runtime does not run it.
// spec: PRM#change-planning
pub(crate) fn would_run(
    state: &OiState,
    app: &AppName,
    def: &Arc<AppDef>,
    definition_error: Option<String>,
) -> Result<Vec<String>, OiError> {
    let standing = read(state, app, def, definition_error, false, StoppedSet::new())?;
    let mut names = Vec::new();
    for (name, status) in &standing.gates.gates {
        match &status.state {
            GateState::Unsatisfied => names.push(name.as_str().to_owned()),
            GateState::Satisfied
            | GateState::Undetermined(Undetermined::Check(_) | Undetermined::Unrecorded(_)) => {}
            GateState::Undetermined(Undetermined::Record(why) | Undetermined::Definition(why)) => {
                return Err(OiError::internal(format!(
                    "could not tell whether gate {name} would be unsatisfied: {why}"
                )));
            }
        }
    }
    Ok(names)
}

// spec: CTL#gate-runs
pub(crate) fn run_gate(
    state: &Arc<OiState>,
    params: RunGateParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let (app_name, gate_name) = (&params.app, &params.gate);
    let (def, phase, generation) = {
        let reg = state.registry.read();
        let entry = reg
            .get(app_name.as_str())
            .ok_or_else(|| OiError::not_found(format!("app not found: {app_name}")))?;
        let def = entry.app.def.load_full();
        if !def.gates.contains_key(gate_name) {
            return Err(OiError::not_found(format!(
                "gate not found: {gate_name} in app {app_name}"
            )));
        }
        if !matches!(*entry.phase.lock(), AppPhase::Installed) {
            return Err(OiError::new(
                ErrorCode::NotInstalled,
                format!("app is not installed: {app_name}"),
            ));
        }
        if entry.script_error.is_some() {
            return Err(OiError::script_error(format!(
                "app has a script error: {app_name}"
            )));
        }
        (def, Arc::clone(&entry.phase), entry.current_generation)
    };

    let unready = unready_targets_of(state, app_name, &def, gate_name)?;
    if !unready.is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "gate {gate_name} cannot run while its targets are not ready: {}",
                unready.join(", ")
            ),
        ));
    }

    let admission = lifecycle::admit_and_spawn(
        state,
        app_name,
        OperationTarget::Gate(gate_name.clone()),
        serde_json::Map::new(),
        generation,
        "operator",
        &phase,
        Some(Arc::clone(&ctx.events.actor)),
    );
    let (schedule, operation_id) = match admission {
        lifecycle::Admission::Started(id) => ("accepted", id),
        lifecycle::Admission::Queued(id) => ("queued", id),
        lifecycle::Admission::CannotPersist(e) => return Err(e),
        lifecycle::Admission::Rejected(reason) => {
            return Err(actions::rejection_error(app_name, reason));
        }
    };

    // spec: GRUN#pauses
    // Ended once the run is admitted, so that a refused request leaves the
    // gate paused. A run that starts ends the pause as well.
    let (app, gate) = (app_name.clone(), gate_name.clone());
    if let Err(e) = state.db.call(move |db| store::clear_pause(db, &app, &gate)) {
        tracing::error!(app = %app_name, gate = %gate_name, "cannot end the pause of a gate an operator runs: {e}");
    }
    state.tick_notify.notify_one();

    tracing::info!(app = %app_name, gate = %gate_name, schedule, "run_gate");
    Ok(json!({ "schedule": schedule, "operation_id": operation_id.0 }))
}

/// A description of each target of the gate that is not ready, or an error
/// where that cannot be told.
// spec: CTL#gate-runs
fn unready_targets_of(
    state: &OiState,
    app: &AppName,
    def: &Arc<AppDef>,
    gate: &GateName,
) -> Result<Vec<String>, OiError> {
    let (app, def, cipher, gate) = (
        app.clone(),
        Arc::clone(def),
        Arc::clone(&state.cipher),
        gate.clone(),
    );
    let remembered = state.gate_runtime.remembered(&app);
    state
        .db
        .call(move |db| {
            let gates = gates::evaluate(db, &app, &def, &cipher, None, &remembered);
            let holds = gates
                .holds
                .as_ref()
                .map_err(|why| format!("could not read what the app's gates hold: {why}"))?;
            let held = held_resources(&def, holds).into_keys().collect();
            let stopped = stopped::load_stopped(db, &app)
                .map_err(|e| format!("could not read the app's stopped resources: {e}"))?;
            let readiness = read_readiness(db, &app, &def, &held, &stopped);
            if let Some(why) = readiness.unreadable {
                return Err(format!(
                    "could not read whether the app's workloads are ready: {why}"
                ));
            }
            let Some(gate_def) = def.gates.get(&gate) else {
                return Ok(Vec::new());
            };
            Ok(unready_targets(&gate_def.after, &gates, &readiness)
                .iter()
                .map(|target| match target {
                    AfterTarget::Gate(name) => format!("gate '{name}'"),
                    AfterTarget::Workload(id) => {
                        format!("{} '{}'", id.kind.type_name(), id.name)
                    }
                })
                .collect())
        })
        .map_err(OiError::internal)
}

#[cfg(test)]
mod tests;
