//! One run of a gate, and the checks that keep a satisfied gate honest.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::RwLock;
use seedling_protocol::{
    events::OperationEventCtx,
    names::{AppName, GateName},
};
use tokio::sync::Notify;

use super::{
    GateStatus, gates_behind,
    store::{self, FailedRuns, GateRecord},
};
use crate::{
    defs::{
        app::{App, AppDef},
        gate::GateDef,
        resource::{Resource, ResourceId, ResourceKind},
    },
    oi::{
        handler::actions::lifecycle::{
            BoundParams, GateInput, OperationEnd, Persisted, app_at_generation, language_for,
            run_operation_loop,
        },
        state::OiState,
    },
    runtime::{
        apps::CheckId,
        barrier::{
            CancelToken, OperationId,
            oracle::{container_run_succeeded, derive_lifecycle_state},
        },
        db::{Db, DbHandle},
        desired::{DynamicCreator, OperationProgress, insert_dynamic_resource},
        dynamic_cleanup::cleanup_dynamic_resources,
        history::{
            CurrentOperation, clear_current_operation, find_instances_for_group, load_action_log,
            query_autonomous_operations, query_observations, save_current_operation,
        },
        identity::ResourceInstance,
        lifecycle::LifecycleState,
        probe::probe_gate_body,
        registry::DbInstanceRegistry,
        secrets::KeyDigest,
    },
};

/// How long a check's Job has to start before the check is taken as one that
/// could not be run.
const START_WAIT: Duration = Duration::from_secs(300);

/// How long a check waits between looks at its Job.
const CHECK_POLL: Duration = Duration::from_millis(500);

/// How long a run waits between looks at the world while it prepares.
const PREPARE_POLL: Duration = Duration::from_millis(500);

/// How long a run waits for its exclusive selection to stop, or for a read
/// that tells it so, before it fails, so that it ends and is retried with
/// back-off rather than holding the app's operation for as long as an
/// instance is wedged or the reads fail.
const SELECTION_WAIT: Duration = Duration::from_secs(600);

/// Whether a run that began waiting for its exclusive selection at `began`
/// has waited for long enough to give up.
// spec: GRUN#preparation
// spec: GRUN#retries
fn selection_waited_too_long(began: Instant, now: Instant) -> bool {
    now.saturating_duration_since(began) >= SELECTION_WAIT
}

/// What a gate run needs of the operation that dispatched it.
pub struct GateRunInput {
    pub app: App,
    pub gate: GateName,
    pub op_ctx: OperationEventCtx,
    pub active_progress: Arc<RwLock<Option<OperationProgress>>>,
    pub tick_notify: Arc<Notify>,
    pub cancel_token: Arc<CancelToken>,
    pub db_path: PathBuf,
}

/// How a run ended, with what recording its outcome needs.
enum RunEnd {
    Succeeded {
        key: KeyDigest,
        generation: u64,
        checked: bool,
    },
    Failed {
        how: String,
        key: Option<KeyDigest>,
    },
    Cancelled {
        key: Option<KeyDigest>,
    },
}

/// Why preparation did not finish.
enum Stopped {
    Failed(String),
    Cancelled,
}

impl From<String> for Stopped {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

/// Run a gate: prepare, run its body, check it, and record how that went.
///
/// A run succeeds when its body returns and its check, if it declares one,
/// then passes. Any other end leaves the gate unsatisfied, with the failure or
/// the cancel on its record for the retry to read.
// spec: GRUN#preparation
// spec: GRUN#outcome
// spec: GRUN#retries
// spec: GRUN#pauses
pub async fn run_gate(state: &Arc<OiState>, input: GateRunInput) {
    let GateRunInput {
        app,
        gate,
        op_ctx,
        active_progress,
        tick_notify,
        cancel_token,
        db_path,
    } = input;

    // spec: GRUN#checks
    let _claim = state.gate_runtime.claim_for_run(&op_ctx.app, &gate).await;
    let run = Run {
        state,
        app_name: op_ctx.app.clone(),
        gate,
        op_ctx,
        active_progress,
        tick_notify,
        cancel: cancel_token,
        db_path,
    };
    let end = run.execute(app).await;
    run.finish(end).await;
}

struct Run<'a> {
    state: &'a Arc<OiState>,
    app_name: AppName,
    gate: GateName,
    op_ctx: OperationEventCtx,
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    tick_notify: Arc<Notify>,
    cancel: Arc<CancelToken>,
    db_path: PathBuf,
}

impl Run<'_> {
    async fn execute(&self, current: App) -> RunEnd {
        let state = self.state;
        let (name, generation) = (&self.app_name, self.op_ctx.operation_generation);

        // spec: GRUN#retries
        // A run that fails before it has the key of its own generation is
        // still recorded, against the gate's current key: with nothing on
        // record the gate would be due again at once, whatever had gone wrong.
        let current_key = current
            .def
            .load()
            .gates
            .get(&self.gate)
            .and_then(|gate| gate.key.digest(&state.cipher).ok());

        // spec: OPS#initiating-events
        // The run is against the definition of its generation, however many
        // have been committed since, and on every replay.
        let at_generation = match app_at_generation(
            &state.db,
            name,
            &current,
            generation,
            &state.script_limits,
            &state.cipher,
        ) {
            Ok(rebuilt) => rebuilt,
            Err(e) => {
                return RunEnd::Failed {
                    how: format!("generation {generation} cannot be rebuilt: {e}"),
                    key: current_key,
                };
            }
        };
        let app = at_generation.as_ref().unwrap_or(&current);
        let def = app.def.load_full();
        let Some(gate_def) = def.gates.get(&self.gate).cloned() else {
            return RunEnd::Failed {
                how: format!("the gate is not declared at generation {generation}"),
                key: current_key,
            };
        };
        let key = match gate_def.key.digest(&state.cipher) {
            Ok(key) => key,
            Err(e) => {
                return RunEnd::Failed {
                    how: format!("the gate's key cannot be digested: {e}"),
                    key: current_key,
                };
            }
        };
        let failed = |how: String| RunEnd::Failed {
            how,
            key: Some(key),
        };

        // spec: OPS#persistence
        // Nothing is done before the operation is on record: a restart could
        // not otherwise replay it.
        let record = CurrentOperation {
            operation_id: OperationId(self.op_ctx.operation_id.clone()),
            app: name.clone(),
            target: self.op_ctx.target.clone(),
            generation,
        };
        let cipher = Arc::clone(&state.cipher);
        let persisted = state.db.call(move |db| {
            save_current_operation(
                db,
                &cipher,
                &record,
                &serde_json::Map::new(),
                &HashMap::new(),
            )
        });
        if let Err(e) = persisted {
            return failed(format!("the operation could not be persisted: {e}"));
        }

        // A body that has begun has already been prepared for, and what it
        // did is on its log: preparing again would pull and wait for no
        // reason, and could fail a run whose work is half done.
        let resumed = {
            let id = OperationId(self.op_ctx.operation_id.clone());
            match state.db.call(move |db| load_action_log(db, &id)) {
                Ok(entries) => !entries.is_empty(),
                Err(e) => return failed(format!("the operation's log cannot be read: {e}")),
            }
        };

        let last = match self.last(&current).await {
            Ok(last) => last,
            Err(why) => return failed(why),
        };

        if !resumed {
            if self.cancel.is_cancelled() {
                return RunEnd::Cancelled { key: Some(key) };
            }
            if let Err(stopped) = self.prepare(app, &def, &gate_def, last.as_ref()).await {
                return match stopped {
                    Stopped::Failed(why) => failed(why),
                    Stopped::Cancelled => RunEnd::Cancelled { key: Some(key) },
                };
            }
        }

        if gate_def.has_body {
            match self.body(&current, last).await {
                OperationEnd::Completed => {}
                OperationEnd::Failed(why) | OperationEnd::NotRun(why) => return failed(why),
                OperationEnd::Cancelled => return RunEnd::Cancelled { key: Some(key) },
            }
        }

        let mut checked = false;
        if let Some(check) = &gate_def.check {
            if self.cancel.is_cancelled() {
                return RunEnd::Cancelled { key: Some(key) };
            }
            let Some(job) = def.resources.get(&check.job).cloned() else {
                return failed("the gate's check is not a Job of the app".to_owned());
            };
            match run_check(state, name, &self.gate, job, Some(&self.cancel)).await {
                CheckOutcome::Passed => checked = true,
                CheckOutcome::Failed(why) => {
                    // The check ran to an outcome, which ends whatever kept
                    // an earlier one from being run.
                    let (app, gate) = (name.clone(), self.gate.clone());
                    state.db.call(move |db| {
                        if let Err(e) = store::clear_check_unavailable(db, &app, &gate) {
                            tracing::error!(%app, %gate, "cannot record that the check ran to an outcome: {e}");
                        }
                    });
                    return failed(format!("its check failed: {why}"));
                }
                CheckOutcome::Unavailable(why) => {
                    let (app, gate, why_for_record) =
                        (name.clone(), self.gate.clone(), why.clone());
                    state.db.call(move |db| {
                        if let Err(e) = store::record_check_unavailable(
                            db,
                            &app,
                            &gate,
                            jiff::Timestamp::now(),
                            &why_for_record,
                        ) {
                            tracing::error!(%app, %gate, "cannot record that the check could not be run: {e}");
                        }
                    });
                    return failed(format!("its check could not be run: {why}"));
                }
                CheckOutcome::Cancelled => return RunEnd::Cancelled { key: Some(key) },
            }
        }

        RunEnd::Succeeded {
            key,
            generation,
            checked,
        }
    }

    /// The app as it was at the gate's last success, or `None` for a gate
    /// that has none.
    // spec: GATE#body
    async fn last(&self, current: &App) -> Result<Option<App>, String> {
        last_success(
            &self.state.db,
            &self.app_name,
            &self.gate,
            current,
            &self.state.script_limits,
            &self.state.cipher,
        )
    }

    /// Everything a run does before its body: invalidate the last success,
    /// pull what it could need, hold the exclusive selection and stop it.
    // spec: GRUN#preparation
    async fn prepare(
        &self,
        app: &App,
        def: &AppDef,
        gate_def: &GateDef,
        last: Option<&App>,
    ) -> Result<(), Stopped> {
        let state = self.state;
        let (name, gate) = (self.app_name.clone(), self.gate.clone());

        let (n, g) = (name.clone(), gate.clone());
        state
            .db
            .call(move |db| store::begin_run(db, &n, &g))
            .map_err(|e| format!("the gate's last success could not be invalidated: {e}"))?;

        let images = self.images(app, def, gate_def, last).await?;
        for image in images {
            self.ensure_image(&image).await?;
        }

        if !gate_def.exclusive.is_empty() {
            let (n, g, held) = (name.clone(), gate.clone(), gate_def.exclusive.clone());
            state
                .db
                .call(move |db| store::set_hold(db, &n, &g, &held))
                .map_err(|e| {
                    format!("the hold on the exclusive selection could not be recorded: {e}")
                })?;
            self.tick_notify.notify_one();
            self.wait_for_selection(&gate_def.exclusive).await?;
        }
        Ok(())
    }

    /// The images the body, the check and the dependants of the gate might
    /// need, as the run's generation gives them.
    // spec: GRUN#preparation
    async fn images(
        &self,
        app: &App,
        def: &AppDef,
        gate_def: &GateDef,
        last: Option<&App>,
    ) -> Result<BTreeSet<String>, Stopped> {
        let mut images = BTreeSet::new();
        if gate_def.has_body {
            let (app, last) = (app.clone(), last.cloned());
            let (gate, db, limits) = (
                self.gate.clone(),
                self.state.db.clone(),
                self.state.script_limits.clone(),
            );
            let probed = tokio::task::spawn_blocking(move || {
                let (engine, _scope, ast) = language_for(&app, &limits)?;
                let registry: Arc<dyn crate::runtime::InstanceRegistry> =
                    Arc::new(DbInstanceRegistry::new(db));
                probe_gate_body(&engine, &ast, &app, registry, &gate, last.as_ref())
            })
            .await
            .map_err(|e| format!("the probe of the gate's body failed: {e}"))?
            .map_err(|why| format!("the gate's body cannot be probed for its images: {why}"))?;
            images.extend(probed.images);
        }
        if let Some(check) = &gate_def.check
            && let Some(image) = def.resources.get(&check.job).and_then(image_of)
        {
            images.insert(image);
        }
        for (id, resource) in &def.resources {
            if gates_behind(def, id).contains(&self.gate)
                && let Some(image) = image_of(resource)
            {
                images.insert(image);
            }
        }
        Ok(images)
    }

    /// Pull `image` if the host does not have it.
    // spec: GRUN#preparation
    async fn ensure_image(&self, image: &str) -> Result<(), Stopped> {
        let runtime = &self.state.container_runtime;
        // A host that cannot say whether it has the image is asked to pull it.
        let present = runtime.image_exists(image).await.unwrap_or(false);
        if present {
            return Ok(());
        }
        tracing::info!(app = %self.app_name, gate = %self.gate, %image, "pulling an image for a gate run");
        tokio::select! {
            _ = cancelled(&self.cancel) => Err(Stopped::Cancelled),
            pulled = runtime.pull_image(image) => pulled
                .map_err(|e| Stopped::Failed(format!("the image {image} could not be pulled: {e}"))),
        }
    }

    /// Wait until every instance of the exclusive selection that the gate's
    /// hold still covers has stopped.
    ///
    /// An instance that never started is as stopped as one that ended.
    // spec: GRUN#preparation
    async fn wait_for_selection(&self, selection: &BTreeSet<ResourceId>) -> Result<(), Stopped> {
        let began = Instant::now();
        loop {
            if self.cancel.is_cancelled() {
                return Err(Stopped::Cancelled);
            }
            let (app, gate, selection) =
                (self.app_name.clone(), self.gate.clone(), selection.clone());
            match self
                .state
                .db
                .call(move |db| held_and_running(db, &app, &gate, &selection))
            {
                Ok(None) => return Ok(()),
                Ok(Some(what)) => {
                    tracing::debug!(app = %self.app_name, gate = %self.gate, "waiting for {what} to stop");
                    if selection_waited_too_long(began, Instant::now()) {
                        return Err(Stopped::Failed(format!(
                            "its exclusive selection had not stopped after {} seconds: {what}",
                            SELECTION_WAIT.as_secs()
                        )));
                    }
                }
                Err(e) => {
                    tracing::warn!(app = %self.app_name, gate = %self.gate, "cannot tell whether the exclusive selection has stopped: {e}");
                    if selection_waited_too_long(began, Instant::now()) {
                        return Err(Stopped::Failed(format!(
                            "whether its exclusive selection had stopped could not be read: {e}"
                        )));
                    }
                }
            }
            self.tick_notify.notify_one();
            tokio::time::sleep(PREPARE_POLL).await;
        }
    }

    /// Run the body to its end.
    async fn body(&self, current: &App, last: Option<App>) -> OperationEnd {
        let state = Arc::clone(self.state);
        let (app, db_path, op_ctx) = (current.clone(), self.db_path.clone(), self.op_ctx.clone());
        let (active_progress, tick_notify, cancel) = (
            Arc::clone(&self.active_progress),
            Arc::clone(&self.tick_notify),
            Arc::clone(&self.cancel),
        );
        let signaler = crate::oi::handler::actions::lifecycle::make_container_signaler(&state);
        let writer = crate::oi::handler::actions::lifecycle::make_volume_writer(&state);
        let executor = crate::oi::handler::actions::lifecycle::make_executor(&state);
        let ran = tokio::task::spawn_blocking(move || {
            let db = match DbHandle::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    return OperationEnd::NotRun(format!("the database could not be opened: {e}"));
                }
            };
            run_operation_loop(
                &app,
                db,
                Ok(BoundParams {
                    params: serde_json::Map::new(),
                    bindings: HashMap::new(),
                }),
                active_progress,
                tick_notify,
                &op_ctx,
                &state.script_limits,
                Arc::clone(&state.cipher),
                &state.gate_runtime,
                cancel,
                signaler,
                writer,
                executor,
                Some(GateInput {
                    last: last.as_ref(),
                }),
                Persisted::Yes,
            )
        })
        .await;
        match ran {
            Ok(end) => end,
            Err(e) => OperationEnd::Failed(format!("the body's task failed: {e}")),
        }
    }

    /// Record how the run ended and report it.
    // spec: GRUN#outcome
    // spec: GRUN#retries
    // spec: GRUN#pauses
    async fn finish(&self, end: RunEnd) {
        let state = self.state;
        let (app, gate) = (self.app_name.clone(), self.gate.clone());
        let end = match end {
            RunEnd::Succeeded {
                key,
                generation,
                checked,
            } => {
                let (a, g) = (app.clone(), gate.clone());
                match state
                    .db
                    .call(move |db| store::record_success(db, &a, &g, &key, generation))
                {
                    Ok(()) => {
                        state.gate_runtime.forget_failed_runs(&app, &gate);
                        // spec: FLT#app-faults
                        // A gate run is a lifecycle operation, so its success
                        // supersedes an earlier operation's failure.
                        let a = app.clone();
                        state.db.call(move |db| {
                            crate::oi::handler::actions::lifecycle::clear_superseded_operation_faults(db, &a)
                        });
                        if checked {
                            state.gate_runtime.clear_for_start(&app, &gate, key);
                        }
                        super::drive::release_hold_if_satisfied(state, &app, &gate).await;
                        Finished::Completed
                    }
                    // spec: GRUN#outcome
                    // A run whose success cannot be recorded did not succeed.
                    Err(e) => {
                        let how = format!("its success could not be recorded: {e}");
                        self.record_failure(Some(key), &how, false).await;
                        Finished::Failed(how)
                    }
                }
            }
            RunEnd::Failed { how, key } => {
                self.record_failure(key, &how, false).await;
                Finished::Failed(how)
            }
            RunEnd::Cancelled { key } => {
                self.record_failure(key, "cancelled by an operator", true)
                    .await;
                Finished::Cancelled
            }
        };
        state.db.call(|db| {
            if let Err(e) = clear_current_operation(db) {
                tracing::error!("cannot clear the persisted operation: {e}");
            }
        });
        super::drive::sync_app(state, &app).await;
        match end {
            Finished::Completed => self.op_ctx.completed(),
            Finished::Failed(how) => self.op_ctx.failed(&how),
            Finished::Cancelled => self.op_ctx.cancelled(),
        }
    }

    async fn record_failure(&self, key: Option<KeyDigest>, how: &str, pause: bool) {
        let Some(key) = key else {
            tracing::error!(app = %self.app_name, gate = %self.gate, "a run of the gate failed with no key to record it against: {how}");
            return;
        };
        tracing::warn!(app = %self.app_name, gate = %self.gate, "a run of the gate did not succeed: {how}");
        // spec: GRUN#retries
        // The failure is held in memory until it is written, so that a
        // failure that cannot be written still spaces the next run.
        let failed = self.state.gate_runtime.note_failed_runs(
            &self.app_name,
            &self.gate,
            FailedRuns::one(key, jiff::Timestamp::now(), how, pause),
        );
        if let Err(e) =
            super::drive::write_failed_runs(self.state, &self.app_name, &self.gate, &failed)
        {
            tracing::error!(app = %self.app_name, gate = %self.gate, "cannot record that a run of the gate did not succeed, so it is held until it can be: {e}");
        }
    }
}

/// The app as it was at the last success of `gate`, as a run gives it to the
/// gate's body, or `None` for a gate with none. `current` is the app as it
/// stands, standing in for a last success at its own generation.
///
/// `Err` where the record cannot be read or the generation cannot be rebuilt,
/// which is not a gate with no last success.
// spec: GATE#body
pub(crate) fn last_success(
    db: &DbHandle,
    app_name: &AppName,
    gate: &GateName,
    current: &App,
    limits: &crate::ScriptLimits,
    cipher: &Arc<crate::runtime::secrets::Cipher>,
) -> Result<Option<App>, String> {
    let (app, name) = (app_name.clone(), gate.clone());
    let generation = db
        .call(move |db| store::load_records(db, &app))
        .map_err(|e| format!("the gate's record cannot be read: {e}"))?
        .remove(&name)
        .map(|record| record.map_err(|why| format!("the gate's record cannot be read: {why}")))
        .transpose()?
        .and_then(|record| record.success_generation);
    let Some(generation) = generation else {
        return Ok(None);
    };
    let rebuilt =
        app_at_generation(db, app_name, current, generation, limits, cipher).map_err(|e| {
            format!("generation {generation}, the gate's last success, cannot be rebuilt: {e}")
        })?;
    Ok(Some(rebuilt.unwrap_or_else(|| current.clone())))
}

enum Finished {
    Completed,
    Failed(String),
    Cancelled,
}

/// Resolves once `cancel` is requested.
async fn cancelled(cancel: &CancelToken) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The image of a Deployment or a Job.
fn image_of(resource: &Resource) -> Option<String> {
    match resource {
        Resource::Deployment(dep) => dep.def.lock().pod.lock().container.lock().image.clone(),
        Resource::Job(job) => job.def.lock().pod.lock().container.lock().image.clone(),
        _ => None,
    }
    .filter(|image| !image.is_empty())
}

/// What of `selection` that the hold of `gate` still covers is running or
/// stopping, if any.
///
/// A resource a change has removed from the selection is released once the
/// hold no longer covers it, and is no longer the run's to wait for.
// spec: GRUN#preparation
// spec: RCN#gate-holds
fn held_and_running(
    db: &Db,
    app: &AppName,
    gate: &GateName,
    selection: &BTreeSet<ResourceId>,
) -> rusqlite::Result<Option<String>> {
    let held = store::load_holds(db, app)?.remove(gate).unwrap_or_default();
    still_running(db, app, &selection.intersection(&held).cloned().collect())
}

/// What of `selection` is still running or stopping, if any.
///
/// An instance that was never started is as stopped as one that ended. One
/// whose latest start has not failed and has not yet been observed is still
/// coming up, and the reconciler stops it once the host shows it.
// spec: GRUN#preparation
fn still_running(
    db: &Db,
    app: &AppName,
    selection: &BTreeSet<ResourceId>,
) -> rusqlite::Result<Option<String>> {
    for id in selection {
        for instance in find_instances_for_group(db, app, id.kind, Some(id.name.as_str()))? {
            let observations = query_observations(db, &instance)?;
            match derive_lifecycle_state(&instance, &observations) {
                state @ (LifecycleState::Pending
                | LifecycleState::Terminated
                | LifecycleState::Unscheduled) => {
                    let seen = observations.last().map_or(i64::MIN, |o| o.recorded_at);
                    if start_unobserved_since(db, &instance, seen)? {
                        return Ok(Some(format!(
                            "{} ({state:?}, being started)",
                            instance.display_name
                        )));
                    }
                }
                other => return Ok(Some(format!("{} ({other:?})", instance.display_name))),
            }
        }
    }
    Ok(None)
}

/// Whether the latest start of `instance` was made at or after `seen`, the time
/// of its latest observation, and has not failed: it is under way, or the host
/// has not yet shown what it did.
fn start_unobserved_since(
    db: &Db,
    instance: &ResourceInstance,
    seen: i64,
) -> rusqlite::Result<bool> {
    let latest = query_autonomous_operations(db, instance.id)?
        .into_iter()
        .rfind(|op| op.operation == "start");
    Ok(latest.is_some_and(|op| {
        op.recorded_at >= seen && op.outcome.as_deref().is_none_or(|outcome| outcome == "ok")
    }))
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

/// How a check of a gate came out.
// spec: GATE#check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// An instance of the check's Job ran to completion and exited with
    /// status 0.
    Passed,
    /// It exited with any other status, or was stopped at its deadline.
    Failed(String),
    /// The runtime could not start its Job's instance.
    Unavailable(String),
    /// The check was cancelled.
    Cancelled,
}

/// Run `job`, the check of `gate`, and judge it by how it exits.
///
/// The Job's instance is not part of the app's steady-state desired state: it
/// is held in a slot of the check's own, beside the app's operation and shell
/// sessions, so that it runs beside whatever else the app is doing. It is torn
/// down once its outcome is known.
// spec: GRUN#checks
// spec: RCN#gate-holds
pub async fn run_check(
    state: &Arc<OiState>,
    app: &AppName,
    gate: &GateName,
    job: Resource,
    cancel: Option<&Arc<CancelToken>>,
) -> CheckOutcome {
    let Some((checks, tick_notify)) = state
        .registry
        .read()
        .get(app.as_str())
        .map(|entry| (Arc::clone(&entry.checks), Arc::clone(&entry.tick_notify)))
    else {
        return CheckOutcome::Unavailable(format!("{app} is not registered"));
    };
    let deadline = match &job {
        Resource::Job(j) => j.def.lock().deadline.map(Duration::from_secs),
        _ => None,
    };
    let check = CheckId::generate();
    let slot = checks.open(check);
    let instance = ResourceInstance::new_anonymous(app.clone(), ResourceKind::Job);

    let recorded = {
        let (instance, owner) = (instance.clone(), check.to_string());
        let description = format!("the check of gate {gate}");
        state.db.call(move |db| {
            insert_dynamic_resource(
                db,
                &instance,
                &owner,
                DynamicCreator::GateCheck,
                Some(&description),
            )
        })
    };
    if let Err(e) = recorded {
        checks.close(&check);
        return CheckOutcome::Unavailable(format!(
            "its instance could not be recorded before it was started: {e}"
        ));
    }

    let mut progress = OperationProgress::new();
    progress.started_dynamic(instance.clone(), job);
    *slot.write() = Some(progress);
    tick_notify.notify_one();

    let outcome = wait_for_check(state, &instance, deadline, cancel).await;

    // The Job is torn down in the background: the outcome is known, and
    // tearing it down takes as long as it takes.
    // spec: GRUN#cancellation
    let teardown = {
        let (state, app, checks, slot, tick_notify) = (
            Arc::clone(state),
            app.clone(),
            Arc::clone(&checks),
            Arc::clone(&slot),
            Arc::clone(&tick_notify),
        );
        async move {
            cleanup_dynamic_resources(
                &state.db,
                &state.driver,
                &app,
                &check.to_string(),
                &slot,
                &tick_notify,
            )
            .await;
            checks.close(&check);
            tick_notify.notify_one();
        }
    };
    tokio::spawn(teardown);
    outcome
}

async fn wait_for_check(
    state: &Arc<OiState>,
    instance: &ResourceInstance,
    deadline: Option<Duration>,
    cancel: Option<&Arc<CancelToken>>,
) -> CheckOutcome {
    let began = Instant::now();
    let mut running_since: Option<Instant> = None;
    let mut ended_at: Option<Instant> = None;
    loop {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return CheckOutcome::Cancelled;
        }
        let observed = {
            let instance = instance.clone();
            state.db.call(move |db| query_observations(db, &instance))
        };
        match observed {
            Ok(observations) => {
                let lifecycle = derive_lifecycle_state(instance, &observations);
                if lifecycle.has_reached(LifecycleState::Running) {
                    running_since.get_or_insert_with(Instant::now);
                }
                if lifecycle.has_reached(LifecycleState::Terminated) {
                    // spec: JOB#deadline
                    let ran_for = running_since.unwrap_or(began).elapsed();
                    match judge(&observations, deadline, ran_for) {
                        Some(outcome) => return outcome,
                        None => {
                            let since = *ended_at.get_or_insert_with(Instant::now);
                            if since.elapsed() >= START_WAIT {
                                return CheckOutcome::Unavailable(
                                    "it ended, and how it ended could not be told".to_owned(),
                                );
                            }
                        }
                    }
                } else if lifecycle == LifecycleState::Pending && began.elapsed() >= START_WAIT {
                    return CheckOutcome::Unavailable(format!(
                        "it did not start within {} seconds",
                        START_WAIT.as_secs()
                    ));
                }
            }
            Err(e) => {
                tracing::warn!(instance = %instance.display_name, "cannot read how a check is going: {e}");
                if began.elapsed() >= START_WAIT {
                    return CheckOutcome::Unavailable(format!(
                        "how it was going could not be read: {e}"
                    ));
                }
            }
        }
        tokio::time::sleep(CHECK_POLL).await;
    }
}

/// How a check that has terminated came out, or `None` while it cannot yet be
/// told.
///
/// A run that was stopped at its deadline failed, whatever the supervisor
/// went on to say of the unit.
// spec: GATE#check
fn judge(
    observations: &[crate::runtime::history::WorldObservation],
    deadline: Option<Duration>,
    ran_for: Duration,
) -> Option<CheckOutcome> {
    let exited_zero = observations.iter().rev().any(|o| {
        o.obs_kind == "container_exited"
            && o.payload.get("exit_code").and_then(|c| c.as_i64()) == Some(0)
    });
    match container_run_succeeded(observations)? {
        true if exited_zero || deadline.is_none_or(|d| ran_for < d) => Some(CheckOutcome::Passed),
        true => Some(CheckOutcome::Failed(
            "it was stopped at its deadline".to_owned(),
        )),
        false => Some(CheckOutcome::Failed(
            match deadline {
                Some(d) if ran_for >= d => "it was stopped at its deadline",
                _ => "it exited with a status other than 0",
            }
            .to_owned(),
        )),
    }
}

/// Whether `status` is a gate whose check is due to be tried again after one
/// that could not be run.
pub fn check_retry_is_due(record: Option<&GateRecord>, now: jiff::Timestamp) -> bool {
    record
        .and_then(super::check_retry_due)
        .is_none_or(|due| now >= due)
}

/// The state a gate's checks are eligible in: it holds a valid success for its
/// current key, whether or not its last check could be run.
pub fn eligible_for_check(status: &GateStatus) -> bool {
    use super::{GateState, Undetermined};
    matches!(
        status.state,
        GateState::Satisfied | GateState::Undetermined(Undetermined::Check(_))
    )
}

#[cfg(test)]
mod tests;
