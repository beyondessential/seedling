use std::{collections::BTreeMap, sync::Arc};

use seedling_protocol::{
    error::{ErrorCode, OiError},
    events::ParamChangeEntry,
    names::{AppName, ParamName},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::HandlerResult;
use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::{
        AppPhase, generations,
        scheduler::{AppHold, RejectReason, ScheduleResult},
    },
};

/// The parameters a request changes, in ascending order of name: a string
/// sets the parameter and `None` unsets it.
///
/// Deserialised from the request's own text, since a JSON object read into a
/// [`serde_json::Value`] first would keep only the last of two members of one
/// name.
// spec: PRM#parameter-changes
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParamChanges(BTreeMap<ParamName, Option<String>>);

impl ParamChanges {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ParamName, Option<&str>)> {
        self.0.iter().map(|(name, value)| (name, value.as_deref()))
    }

    /// The values an app would hold, given what it holds now, if every member
    /// of the request were applied.
    pub(crate) fn applied_to(
        &self,
        current: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut proposed = current.clone();
        for (name, value) in self.iter() {
            match value {
                Some(v) => proposed.insert(name.as_str().to_owned(), v.to_owned()),
                None => proposed.remove(name.as_str()),
            };
        }
        proposed
    }

    /// The members that move a stored value: a member giving a parameter's
    /// stored value, or unsetting one with no stored value, changes nothing.
    pub(crate) fn edits(&self, current: &BTreeMap<String, String>) -> Vec<ParamEdit> {
        self.iter()
            .filter_map(|(name, value)| {
                let previous = current.get(name.as_str());
                (previous.map(String::as_str) != value).then(|| ParamEdit {
                    name: name.clone(),
                    previous: previous.cloned(),
                    new_value: value.map(str::to_owned),
                })
            })
            .collect()
    }
}

impl<'de> Deserialize<'de> for ParamChanges {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{Error, MapAccess, Visitor};

        struct Changes;

        impl<'de> Visitor<'de> for Changes {
            type Value = ParamChanges;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object naming each parameter to change, with a string or null")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut changes = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let name = ParamName::new(&key).map_err(A::Error::custom)?;
                    let value = match map.next_value::<Value>()? {
                        Value::String(s) => Some(s),
                        Value::Null => None,
                        _ => {
                            return Err(A::Error::custom(format!(
                                "parameter '{key}' must be a string, or null to unset it"
                            )));
                        }
                    };
                    if changes.insert(name, value).is_some() {
                        return Err(A::Error::custom(format!(
                            "parameter '{key}' is named more than once"
                        )));
                    }
                }
                if changes.is_empty() {
                    return Err(A::Error::custom("params must name at least one parameter"));
                }
                Ok(ParamChanges(changes))
            }
        }

        deserializer.deserialize_map(Changes)
    }
}

#[derive(Deserialize)]
pub(crate) struct SetParamsParams {
    pub app: AppName,
    pub params: ParamChanges,
}

/// A parameter whose stored value a change moves.
pub(crate) struct ParamEdit {
    pub name: ParamName,
    pub previous: Option<String>,
    /// `None` when the change unsets the parameter.
    pub new_value: Option<String>,
}

/// An edit once its value is stored, with where each of its two values was
/// held.
pub(crate) struct StoredEdit {
    edit: ParamEdit,
    is_secret: bool,
    previous_is_secret: bool,
}

impl StoredEdit {
    pub(crate) fn name(&self) -> &ParamName {
        &self.edit.name
    }

    pub(crate) fn record(&self) -> generations::ParamChange<'_> {
        generations::ParamChange {
            name: &self.edit.name,
            previous: self.edit.previous.as_deref(),
            new_value: self.edit.new_value.as_deref(),
            is_secret: self.is_secret,
            previous_is_secret: self.previous_is_secret,
        }
    }

    // spec: SEC#generation-history
    // spec: SEC#redaction
    /// The entry an event carries for this edit: neither value if either
    /// was, or is, held in secret storage, since the entry carries one
    /// redaction flag for both.
    pub(crate) fn event_entry(&self) -> ParamChangeEntry {
        if self.is_secret || self.previous_is_secret {
            ParamChangeEntry::redacted(self.edit.name.clone())
        } else {
            ParamChangeEntry::new(
                self.edit.name.clone(),
                self.edit.previous.clone(),
                self.edit.new_value.clone(),
            )
        }
    }
}

/// Store each edit's value, in the caller's transaction, and read whether the
/// value it replaces was held in secret storage before the write moves it.
// spec: SEC#generation-history
// spec: GEN#generation-numbers
pub(crate) fn store_edits(
    db: &crate::runtime::db::Db,
    cipher: &crate::runtime::secrets::Cipher,
    app: &AppName,
    edits: Vec<ParamEdit>,
    is_secret: impl Fn(&ParamName) -> bool,
) -> rusqlite::Result<Vec<StoredEdit>> {
    edits
        .into_iter()
        .map(|edit| {
            let previous_is_secret =
                crate::runtime::apps::secret_params::is_stored_secret(db, app, &edit.name)?;
            let is_secret = is_secret(&edit.name);
            crate::runtime::apps::store_param_value(
                db,
                cipher,
                app,
                &edit.name,
                edit.new_value.as_deref(),
                is_secret,
            )?;
            Ok(StoredEdit {
                edit,
                is_secret,
                previous_is_secret,
            })
        })
        .collect()
}

pub(crate) fn load_current_values(
    state: &OiState,
    app: &AppName,
) -> Result<BTreeMap<String, String>, OiError> {
    let app_owned = app.clone();
    let cipher = Arc::clone(&state.cipher);
    state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &app_owned))
        .map_err(|e| OiError::internal(format!("db error: {e}")))
}

fn param_is_secret(state: &OiState, app: &AppName, name: &ParamName) -> bool {
    state
        .registry
        .read()
        .get(app.as_str())
        .and_then(|e| e.app.def.load().params.get(name).map(|d| d.is_secret()))
        .unwrap_or(false)
}

// spec: OPS#changes-to-an-app
/// Hold `app` for a parameter or definition change, or refuse the change.
///
/// Changes to an app apply one at a time. An operation running or queued for
/// the app does not refuse a change: it goes on against its own generation.
pub(crate) fn hold_for_change(state: &OiState, app: &AppName) -> Result<AppHold, OiError> {
    AppHold::take(&state.scheduler, app)
        .map_err(|blocker| super::apps::app_blocked(app.as_str(), "change", &blocker))
}

fn reload_and_persist_apperror(
    state: &OiState,
    app: &AppName,
    new_generation: generations::Generation,
) -> Result<(), OiError> {
    use crate::oi::handler::apps::{extract_persist_fields, persist_app_fields, sync_fault_state};

    let app_owned = app.clone();
    let cipher = Arc::clone(&state.cipher);
    let loaded_params = state
        .db
        .call(move |db| crate::runtime::apps::load_all_params_for_app(db, &cipher, &app_owned))
        .map_err(|e| OiError::internal(format!("db error: {e}")))?;
    // The outcome needs no gating here: this re-evaluates the app's existing
    // definition under new param values and derives nothing from the result
    // but the fault state synced below, which is filed on either outcome.
    let _ = crate::runtime::apps::reload(
        &state.registry,
        &state.db,
        &state.cipher,
        app,
        &loaded_params,
        &state.script_limits,
    );
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(app.as_str()) {
            sync_fault_state(&state.db, entry);
        }
    }
    // spec: FWD#forward-lifetime
    // The re-evaluation may have dropped a service or port a forward reaches.
    crate::oi::forwards::end_undeclared_forwards(state, app.as_str());
    {
        let mut reg = state.registry.write();
        if let Some(entry) = reg.get_mut(app.as_str()) {
            entry.current_generation = new_generation;
        }
    }
    {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
        let (app_name, generation_n, installed, uninstalling, installing) =
            extract_persist_fields(entry);
        state
            .db
            .call(move |db| {
                persist_app_fields(
                    db,
                    &app_name,
                    generation_n,
                    installed,
                    uninstalling,
                    installing,
                )
            })
            .map_err(|e| OiError::internal(format!("db persist: {e}")))?;
    }
    Ok(())
}

/// Validate the values a parameter change would leave the app with, before
/// anything is persisted.
///
/// The validators are those of the current definition evaluated with the
/// proposed values; when that evaluation fails, those of the app's most
/// recent successful evaluation, which the registry holds as its running
/// `App` together with the bundle and values it came from.
// spec: PRM#validation
fn validate_proposed(
    state: &OiState,
    app: &AppName,
    proposed: &BTreeMap<String, String>,
) -> Result<(), OiError> {
    let (bundle, last_good) = {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
        (
            Arc::clone(&entry.bundle),
            (
                Arc::clone(&entry.app.bundle),
                entry.app.stored.lock().clone(),
            ),
        )
    };
    let limits = &state.script_limits;
    let (_, result) =
        crate::runtime::apps::evaluate_validated(app, &bundle, proposed, proposed, limits);
    let rejections = match result {
        Ok(r) => r,
        Err(_) => {
            let (last_bundle, last_values) = last_good;
            let (_, fallback) = crate::runtime::apps::evaluate_validated(
                app,
                &last_bundle,
                &last_values,
                proposed,
                limits,
            );
            // With no evaluation that succeeds there are no validators to
            // run; the value is stored and the failure surfaces as
            // `script_error`, as it would without validation.
            fallback.unwrap_or_default()
        }
    };
    if rejections.is_empty() {
        Ok(())
    } else {
        Err(super::apps::validation_error(&rejections))
    }
}

/// Admit the `on_change` handler for a committed change in place of the
/// hold [`hold_for_change`] took before it, or report that none runs.
///
/// A temporary shim until change handlers are removed: a change schedules at
/// most one handler, that of the first changed parameter, in ascending order
/// of name, that registers one. A handler is not scheduled while another
/// operation runs or is queued for the app.
pub(crate) fn schedule_on_change(
    state: &Arc<OiState>,
    hold: AppHold,
    app: &AppName,
    changed: &[&ParamName],
    generation: generations::Generation,
    actor: &Arc<seedling_protocol::actor::Actor>,
) -> Result<(), OiError> {
    let (handler_param, is_installed, tick_notify, phase_handle) = {
        let reg = state.registry.read();
        let entry = reg.get(app.as_str()).expect("confirmed registered");
        let def = entry.app.def.load();
        let handler_param = changed
            .iter()
            .find(|name| def.param_changes.contains(**name))
            .map(|name| (*name).clone());
        // spec: TRD#uninstall-admission
        // An app being torn down admits no handler. The hold keeps an
        // uninstall from starting until the handler is admitted, so the
        // phase read here is the one admission will see.
        let installed = matches!(*entry.phase.lock(), AppPhase::Installed);
        let notify = Arc::clone(&entry.tick_notify);
        (handler_param, installed, notify, Arc::clone(&entry.phase))
    };

    let Some(param_name) = handler_param.filter(|_| is_installed) else {
        return Ok(());
    };
    let source_generation = generation.saturating_sub(1);
    // The on_change handler is dispatched by using the param name itself as an
    // action identifier. Param names follow the same bsl.name rules.
    let param_action = seedling_protocol::names::ActionName::new_unchecked(param_name.as_str());
    let (result, op_id) = hold.admit(
        &param_action,
        source_generation,
        generation,
        "param_change",
        &phase_handle,
        Some(Arc::clone(actor)),
    );
    match result {
        // A queued handler runs when the other app's operation ends, and is
        // the operation this change triggered all the same.
        ScheduleResult::Accepted | ScheduleResult::Queued => {
            let refused_after_commit = |why: &dyn std::fmt::Display| {
                OiError::internal(format!(
                    "the change to {app} was applied as generation {generation}, but its \
                     on_change handler could not be scheduled: {why}"
                ))
            };
            let op_id = op_id.ok_or_else(|| refused_after_commit(&"it holds no operation"))?;
            // Attach the operation to the generation history entry so the
            // outcome can be recorded against it later.
            {
                let app_owned = app.clone();
                let op = op_id.0.clone();
                state.db.call(move |db| {
                    if let Err(e) = generations::attach_operation(db, &app_owned, generation, &op) {
                        tracing::warn!(app = %app_owned, generation, "failed to attach op to generation: {e}");
                    }
                });
            }
            // spec: OPS#initiating-events
            // spec: OPS#concurrency
            if matches!(result, ScheduleResult::Accepted) {
                super::actions::lifecycle::spawn_accepted_operation(
                    Arc::clone(state),
                    app.clone(),
                    param_action,
                    op_id,
                    super::actions::lifecycle::OperationParams::Requested(serde_json::Map::new()),
                    source_generation,
                    generation,
                    "param_change".to_owned(),
                    Some(Arc::clone(actor)),
                );
            } else {
                super::actions::lifecycle::persist_queued(state, &op_id)
                    .map_err(|e| refused_after_commit(&e.message))?;
            }
            tick_notify.notify_one();
            Ok(())
        }
        ScheduleResult::Rejected(
            RejectReason::SameAppOperationInProgress | RejectReason::SameAppAlreadyQueued,
        ) => {
            tracing::warn!(app = %app, param = %param_name, generation, "on_change handler not scheduled: another operation is running or queued for the app");
            Ok(())
        }
        // The hold ruled out every other refusal, so this is a broken
        // invariant. The change is committed either way, and the error says
        // so rather than reading as a change that did not happen.
        ScheduleResult::Rejected(reason) => {
            tracing::error!(app = %app, param = %param_name, generation, ?reason, "on_change handler refused after the change was committed");
            Err(OiError::new(
                ErrorCode::Internal,
                format!(
                    "the change to {app} was applied as generation {generation}, but its on_change \
                     handler was refused: {reason:?}"
                ),
            ))
        }
    }
}

// spec: PRM#parameter-changes
// spec: PRM#parameter-storage
pub(crate) fn set_params(
    state: &Arc<OiState>,
    params: SetParamsParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let app = &params.app;

    {
        let reg = state.registry.read();
        if !reg.is_registered(app.as_str()) {
            return Err(OiError::not_found(format!("app not found: {app}")));
        }
    }

    let hold = hold_for_change(state, app)?;

    let current = load_current_values(state, app)?;
    let edits = params.params.edits(&current);

    let previous_generation = {
        let reg = state.registry.read();
        reg.get(app.as_str())
            .map(|e| e.current_generation)
            .unwrap_or(0)
    };
    if edits.is_empty() {
        return Ok(json!({ "generation": previous_generation }));
    }

    validate_proposed(state, app, &params.params.applied_to(&current))?;

    let secret: BTreeMap<ParamName, bool> = edits
        .iter()
        .map(|e| (e.name.clone(), param_is_secret(state, app, &e.name)))
        .collect();
    let app_owned = app.clone();
    let cipher = Arc::clone(&state.cipher);

    let (generation, stored) = state
        .db
        .call(move |db| -> rusqlite::Result<_> {
            // spec: GEN#generation-numbers — the stored values commit with their bump.
            db.atomically(|db| {
                let stored = store_edits(db, &cipher, &app_owned, edits, |name| secret[name])?;
                let generation = {
                    let records: Vec<_> = stored.iter().map(StoredEdit::record).collect();
                    generations::bump_param_change(db, &app_owned, &records, &cipher)?
                };
                Ok((generation, stored))
            })
        })
        .map_err(|e| OiError::internal(format!("db error: {e}")))?;

    reload_and_persist_apperror(state, app, generation)?;

    // spec: IMG#pin-reconciliation
    super::images::reconcile_pins_post_update(state, app);

    let changed: Vec<&ParamName> = stored.iter().map(StoredEdit::name).collect();
    schedule_on_change(state, hold, app, &changed, generation, &ctx.events.actor)?;

    // spec: PRM#parameter-changes
    ctx.events.params_changed(
        app,
        stored.iter().map(StoredEdit::event_entry).collect(),
        generation,
        previous_generation,
    );

    tracing::info!(app = %app, params = changed.len(), generation, "set_params");
    Ok(json!({ "generation": generation }))
}

#[cfg(test)]
mod tests;
