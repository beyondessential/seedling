use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use jiff::Timestamp;
use parking_lot::{Mutex, RwLock};
use seedling_bundle::Bundle;
use seedling_protocol::events::OperationTarget;
use seedling_protocol::names::{AppName, ParamName, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{
    defs::{
        app::App,
        param_def::{ParsedValue, SuppliedValue},
        regex::Budget,
    },
    runtime::{
        breakers,
        db::{Db, DbHandle},
        definition::Source,
        desired::OperationProgress,
        generations, scaling,
        secrets::Cipher,
    },
    setup_language,
};

mod params;
mod registry_faults;
pub mod secret_params;
pub mod seeds;
pub mod status;

pub use params::{
    delete_app_params, delete_one_param, load_params_for_app, sync_script_error_fault, upsert_param,
};
pub use registry_faults::{sync_registry_faults, sync_used_registries};

/// Why an evaluation of a definition failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// The script failed to parse or evaluate.
    Failed {
        message: String,
        /// The script file, and the line within it, where the error arose,
        /// for an error that arose at a position in the script.
        // spec: CLI#check-results
        at: Option<(String, usize)>,
    },
    /// The evaluation ran past the evaluation time limit, or a tripped
    /// breaker failed it.
    // spec: LIM#measuring-evaluation-time
    // spec: LIM#breakers
    Runaway(String),
    /// Whether the breaker is tripped could not be read, so the definition
    /// was not evaluated.
    // spec: LIM#breakers
    BreakerUnreadable(String),
}

impl ScriptError {
    /// A failure that arose at no position in the script.
    pub fn failed(message: String) -> Self {
        Self::Failed { message, at: None }
    }

    /// An error Rhai raised evaluating `script`, at the position where it
    /// arose: for one raised inside a called function, the position in the
    /// function rather than where it was called, and none where it arose at
    /// no position.
    // spec: BND#script-formation
    // spec: CLI#check-results
    fn raised(script: &seedling_bundle::Script, e: &rhai::EvalAltResult) -> Self {
        fn innermost(e: &rhai::EvalAltResult) -> rhai::Position {
            match e {
                rhai::EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
                | rhai::EvalAltResult::ErrorInModule(_, inner, _) => innermost(inner),
                _ => e.position(),
            }
        }
        let at = innermost(e)
            .line()
            .and_then(|line| script.locate(line))
            .map(|(file, line)| (file.to_owned(), line));
        Self::Failed {
            message: script.remap_error(&e.to_string()),
            at,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Failed { message, .. }
            | Self::Runaway(message)
            | Self::BreakerUnreadable(message) => message,
        }
    }

    /// The script file and line a failure arose at, where it arose at one.
    pub fn at(&self) -> Option<&(String, usize)> {
        match self {
            Self::Failed { at, .. } => at.as_ref(),
            Self::Runaway(_) | Self::BreakerUnreadable(_) => None,
        }
    }

    pub fn is_runaway(&self) -> bool {
        matches!(self, Self::Runaway(_))
    }

    fn map(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::Failed { message, at } => Self::Failed {
                message: f(message),
                at,
            },
            Self::Runaway(m) => Self::Runaway(f(m)),
            Self::BreakerUnreadable(m) => Self::BreakerUnreadable(f(m)),
        }
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ScriptError {}

/// What [`AppRegistry::reload`] did to the registry.
///
/// `evaluate_script` always returns an `App`, populated up to wherever the
/// script threw. That contract is right for registration, where there is no
/// previous definition to lose; it is wrong for reload, where publishing a
/// partial definition lets every consumer diff live state against a truncated
/// one. The distinction is in the type so callers have to make it.
#[derive(Debug)]
#[must_use]
pub enum ReloadOutcome {
    /// The script evaluated cleanly and the registry now holds its definition.
    Applied,
    /// Evaluation failed. The previous definition keeps running and the
    /// error is recorded; no state derived from the new evaluation may be
    /// diffed against the registry.
    KeptPrevious(ScriptError),
    /// The app is not registered, so nothing was evaluated or stored.
    NotRegistered,
}

impl ReloadOutcome {
    /// Whether the registry now reflects the script that was passed in.
    pub fn is_applied(&self) -> bool {
        matches!(self, Self::Applied)
    }
}

/// The installation phase of an app. Stored in `registered_apps` and shared
/// with the reconciler via Arc so the reconciler can transition it on cleanup.
// spec: APPS#status-derivation
#[derive(Debug, Clone, PartialEq)]
pub enum AppPhase {
    NotInstalled,
    Installed,
    Uninstalling,
}

// spec: APPS#status-derivation
#[derive(Debug, Clone)]
pub enum AppStatus {
    NotInstalled,
    Installing,
    Uninstalling,
    /// An operation of the app's is running: an action, or a gate's run.
    Operating {
        operation: OperationTarget,
    },
    Running,
    Degraded,
    Faulted,
}

impl AppStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Installing => "installing",
            Self::Uninstalling => "uninstalling",
            Self::Operating { .. } => "operating",
            Self::Running => "running",
            Self::Degraded => "degraded",
            Self::Faulted => "faulted",
        }
    }
}

/// Where the dynamic resources of a shell session or a gate check stand in the
/// desired state, as an operation's stand in [`AppEntry::active_progress`].
pub type DynamicSlot = Arc<RwLock<Option<OperationProgress>>>;

/// Identifies one check of a gate, and so the record of its Job.
// spec: GRUN#checks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckId(uuid::Uuid);

impl CheckId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl std::fmt::Display for CheckId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// The slot of each owner of dynamic resources of one kind that is running on
/// an app, or that has ended and is still tearing its dynamic resources down.
///
/// Unlike operations, of which one runs per host, any number of these run
/// beside an operation and each other, so each has a slot of its own.
// spec: RCN#desired-state
pub struct Slots<K> {
    slots: RwLock<BTreeMap<K, DynamicSlot>>,
}

/// The slots of an app's shell sessions.
pub type SessionSlots = Slots<SessionId>;

/// The slots of the checks of an app's gates.
pub type CheckSlots = Slots<CheckId>;

impl<K> Default for Slots<K> {
    fn default() -> Self {
        Self {
            slots: RwLock::default(),
        }
    }
}

impl<K: Ord + Copy + std::fmt::Display> Slots<K> {
    /// Give `owner` an empty slot, which stands until it is [closed](Self::close).
    pub fn open(&self, owner: K) -> DynamicSlot {
        Arc::clone(self.slots.write().entry(owner).or_default())
    }

    pub fn close(&self, owner: &K) {
        self.slots.write().remove(owner);
    }

    pub fn is_empty(&self) -> bool {
        self.slots.read().is_empty()
    }

    /// The owners that have a slot.
    pub fn owners(&self) -> Vec<K> {
        self.slots.read().keys().copied().collect()
    }

    /// Every owner's slot, to be read in place.
    pub fn slots(&self) -> Vec<DynamicSlot> {
        self.slots.read().values().map(Arc::clone).collect()
    }
}

pub struct AppEntry {
    pub name: AppName,
    /// The definition installed at the current generation.
    pub bundle: Arc<Bundle>,
    /// How that definition reached the runtime.
    // spec: DEFN#provenance-reporting
    pub source: Source,
    /// The most recent successful evaluation: the definition this app runs.
    /// Its `stored` map and `bundle` are the values and definition it was
    /// evaluated from, which is where validators are taken from when a
    /// proposed change fails to evaluate.
    pub app: App,
    /// Shared with the reconciler so it can transition the phase when cleanup completes.
    pub phase: Arc<Mutex<AppPhase>>,
    /// Shared with the reconciler and operation runner to track in-progress ops.
    pub active_progress: Arc<RwLock<Option<OperationProgress>>>,
    /// Shared with the reconciler and the shell sessions to track theirs.
    // spec: RCN#desired-state
    pub sessions: Arc<SessionSlots>,
    /// Shared with the reconciler and the gate checks to track their Jobs.
    // spec: RCN#gate-holds
    pub checks: Arc<CheckSlots>,
    /// Wakes the reconciler for an immediate tick.
    pub tick_notify: Arc<Notify>,
    /// Active script-evaluation fault, if the last reload failed.
    /// Cleared on the next successful evaluation.
    pub script_error: Option<(ScriptError, Timestamp)>,
    /// Current app generation (0 if not yet bumped).
    // spec: APPS#generations
    pub current_generation: generations::Generation,
    /// Where the app's latest completed install stands in the order installs
    /// completed, lower first; `None` before any install has completed.
    // spec: ING#claims-across-apps
    pub install_seq: Option<i64>,
    /// The seed the app's static and validator contexts derive from.
    // spec: SEC#app-seeds
    pub seed: crate::runtime::seeds::AppSeed,
}

impl AppEntry {
    /// Whether the app's desired state is its whole resource graph, which
    /// nothing in flight can change: it is installed, and no operation or shell
    /// session is placing resources of its own.
    ///
    /// A running gate check leaves the app steady: its Job is anonymous and
    /// joins the desired state beside the resource graph without changing it.
    // spec: RCN#desired-state
    // spec: GC#unscheduled-instances
    pub fn is_steady(&self) -> bool {
        *self.phase.lock() == AppPhase::Installed
            && self.active_progress.read().is_none()
            && self.sessions.is_empty()
    }

    /// Whether a shell session or a gate check has a slot on the app, so that
    /// the reconciler still has dynamic resources of the app to drive.
    pub fn has_dynamic_slots(&self) -> bool {
        !self.sessions.is_empty() || !self.checks.is_empty()
    }
}

pub struct AppRegistry {
    entries: HashMap<String, AppEntry>,
}

impl AppRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AppRegistry {
    pub fn is_registered(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    #[cfg(test)]
    pub fn register(
        &mut self,
        name: AppName,
        bundle: Arc<Bundle>,
        source: Source,
        tick_notify: Arc<Notify>,
        limits: &crate::ScriptLimits,
    ) -> Result<(), ScriptError> {
        let seed = crate::runtime::seeds::AppSeed::for_tests();
        let (app, script_error) = evaluate(&name, &bundle, &BTreeMap::new(), &seed, limits);
        self.insert_registered(
            name,
            bundle,
            source,
            app,
            script_error,
            tick_notify,
            0,
            seed,
        );
        Ok(())
    }

    // spec: APPS#registration
    /// Make an already-evaluated app observable at `generation`.
    ///
    /// Separate from [`Self::register`] so a caller can commit the app's rows
    /// before the entry exists: a registration whose persistence failed must
    /// not leave an app that `/apps/list` shows, a restart silently drops,
    /// and a retried `/apps/create` rejects as already registered.
    #[expect(
        clippy::too_many_arguments,
        reason = "an entry is exactly these parts; a builder would only rename them"
    )]
    pub fn insert_registered(
        &mut self,
        name: AppName,
        bundle: Arc<Bundle>,
        source: Source,
        app: App,
        script_error: Option<ScriptError>,
        tick_notify: Arc<Notify>,
        generation: u64,
        seed: crate::runtime::seeds::AppSeed,
    ) {
        let script_error = script_error.map(|e| {
            tracing::warn!(app = %name, error = %e, "script has errors at registration; params may need to be set");
            (e, Timestamp::now())
        });
        self.entries.insert(
            name.as_str().to_owned(),
            AppEntry {
                name,
                bundle,
                source,
                app,
                phase: Arc::new(Mutex::new(AppPhase::NotInstalled)),
                active_progress: Arc::new(RwLock::new(None)),
                sessions: Arc::default(),
                checks: Arc::default(),
                tick_notify,
                script_error,
                current_generation: generation,
                install_seq: None,
                seed,
            },
        );
    }

    /// The id of every shell session and gate check that has a slot on any
    /// app: those running, and those ended whose dynamic resources are still
    /// being torn down.
    // spec: OPS#shell-sessions
    // spec: GRUN#checks
    pub fn live_slot_owners(&self) -> std::collections::HashSet<String> {
        self.entries
            .values()
            .flat_map(|entry| {
                let sessions = entry.sessions.owners().into_iter().map(|s| s.to_string());
                let checks = entry.checks.owners().into_iter().map(|c| c.to_string());
                sessions.chain(checks).collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn deregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    // spec: APPS#update
    // spec: PRM#parameter-changes
    /// Re-evaluate the app's current definition with updated stored params.
    ///
    /// On success the entry's app is updated and any active script-error
    /// fault is cleared. On failure the existing AppDef keeps running and the
    /// fault is recorded — the caller always succeeds.
    ///
    /// The returned outcome is what tells a caller whether the registry now
    /// holds a definition derived from these params. Anything that diffs the
    /// registry against previous state — volume holds, scaling bounds,
    /// forwards, schedules — is only meaningful on [`ReloadOutcome::Applied`].
    ///
    /// Reached only through [`reload`], which follows an applied definition
    /// with the secret migration it calls for.
    #[must_use = "a KeptPrevious reload must not be followed by state derived from the new values"]
    fn reevaluate(
        &mut self,
        name: &AppName,
        params: &BTreeMap<String, String>,
        limits: &crate::ScriptLimits,
    ) -> ReloadOutcome {
        // Checked before evaluating: an unregistered app has no definition to
        // replace, and reporting `Applied` for one would tell a caller the
        // registry reflects values it never stored.
        let Some((bundle, seed)) = self
            .entries
            .get(name.as_str())
            .map(|e| (Arc::clone(&e.bundle), e.seed.clone()))
        else {
            return ReloadOutcome::NotRegistered;
        };
        let (app, raw_error) = evaluate(name, &bundle, params, &seed, limits);
        let entry = self
            .entries
            .get_mut(name.as_str())
            .expect("checked just above");
        match raw_error {
            None => {
                entry.app = app;
                entry.script_error = None;
                ReloadOutcome::Applied
            }
            // spec: APPS#update — `app` here is a partial evaluation: the
            // builders that ran before the script threw, and none after. It is
            // dropped rather than published, so the previous good definition
            // keeps running and nothing downstream can diff against it.
            Some(e) => {
                entry.script_error = Some((e.clone(), Timestamp::now()));
                ReloadOutcome::KeptPrevious(e)
            }
        }
    }

    /// Keep the app's definition as it is, recording why its definition was
    /// not evaluated.
    // spec: LIM#breakers
    fn keep_previous(&mut self, name: &AppName, e: ScriptError) -> ReloadOutcome {
        let Some(entry) = self.entries.get_mut(name.as_str()) else {
            return ReloadOutcome::NotRegistered;
        };
        entry.script_error = Some((e.clone(), Timestamp::now()));
        ReloadOutcome::KeptPrevious(e)
    }

    /// Install an already-evaluated, already-validated definition, whose
    /// generation has been committed, as the one the app runs.
    // spec: APPS#update
    pub fn replace_definition(
        &mut self,
        name: &AppName,
        bundle: Arc<Bundle>,
        source: Source,
        app: App,
        generation: generations::Generation,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(name.as_str()) else {
            return false;
        };
        entry.bundle = bundle;
        entry.source = source;
        entry.app = app;
        entry.script_error = None;
        entry.current_generation = generation;
        true
    }

    pub fn get(&self, name: &str) -> Option<&AppEntry> {
        self.entries.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AppEntry> {
        self.entries.values()
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut AppEntry> {
        self.entries.get_mut(name)
    }

    /// Every registered app's name, sorted.
    // spec: APPS#listing
    pub fn names(&self) -> Vec<AppName> {
        let mut names: Vec<_> = self.entries.values().map(|e| e.name.clone()).collect();
        names.sort();
        names
    }

    // spec: APPS#persistence
    pub fn load_from_db(
        db: &Db,
        cipher: &Cipher,
        tick_notify: Arc<Notify>,
        limits: &crate::ScriptLimits,
    ) -> rusqlite::Result<Self> {
        let mut registry = Self::new();
        let mut stmt = db.conn.prepare(
            "SELECT name, installed, uninstalling, current_generation, install_seq \
             FROM registered_apps ORDER BY name",
        )?;
        type Row = (AppName, bool, bool, i64, Option<i64>);
        let rows: Vec<Row> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, AppName>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for (name, installed, uninstalling, current_gen, install_seq) in rows {
            let phase = decode_phase(installed, uninstalling);
            if current_gen <= 0 {
                tracing::warn!(app = %name, "skipping app with no current generation");
                continue;
            }
            let current_generation = current_gen as generations::Generation;

            let (hash, source) = match generations::definition_at(db, &name, current_generation) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(app = %name, generation = current_generation, "failed to resolve definition: {e}");
                    continue;
                }
            };
            let bundle = match generations::load_bundle(db, &hash) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(app = %name, hash = %hash, "failed to load definition bundle: {e}");
                    continue;
                }
            };
            let stored = match load_all_params_for_app(db, cipher, &name) {
                Ok(stored) => stored,
                Err(e) => {
                    tracing::warn!(app = %name, "failed to load stored params: {e}");
                    continue;
                }
            };
            let seed = match seeds::load_app_seed(db, cipher, &name) {
                Ok(seed) => seed,
                Err(e) => {
                    tracing::warn!(app = %name, "failed to load app seed: {e}");
                    continue;
                }
            };
            // spec: APPS#persistence — reload runs no validators: the stored
            // combination was validated when it was written.
            // spec: LIM#breakers
            let run = breakers::Run::new(bundle.hash(), &stored, Some(&seed), limits);
            let (app, raw_error) =
                match breakers::standing(db, cipher, &name, &breakers::Thing::Definition, &run) {
                    Ok(breakers::Standing::Tripped) => (
                        App::with_bundle(Arc::clone(&bundle)),
                        Some(ScriptError::Runaway(breakers::tripped_message(
                            &breakers::Thing::Definition,
                        ))),
                    ),
                    Err(why) => (
                        App::with_bundle(Arc::clone(&bundle)),
                        Some(ScriptError::BreakerUnreadable(
                            breakers::unreadable_message(&breakers::Thing::Definition, &why),
                        )),
                    ),
                    Ok(standing) => {
                        let (app, raw_error) = evaluate(&name, &bundle, &stored, &seed, limits);
                        record_definition_outcome(
                            db,
                            cipher,
                            &name,
                            &run,
                            standing,
                            current_generation,
                            raw_error.as_ref(),
                            &app.def.load(),
                            limits,
                        );
                        (app, raw_error)
                    }
                };
            // spec: SEC#migration — after the script declares which params are secret,
            // migrate any plaintext rows that should now be encrypted.
            migrate_param_storage(db, cipher, &name, &app, raw_error.is_none());
            // spec: AUTO#clamping
            // A definition that threw part-way declares only what ran before
            // it, so its bounds are not ones to clamp to.
            if raw_error.is_none()
                && let Err(e) = scaling::clamp_to_definition(db, &name, &app)
            {
                tracing::error!(app = %name, error = %e, "failed to clamp scaling decisions");
            }
            let script_error = raw_error.map(|e| {
                tracing::warn!("failed to reload script for app '{name}': {e}");
                (e, Timestamp::now())
            });
            registry.entries.insert(
                name.as_str().to_owned(),
                AppEntry {
                    name,
                    bundle,
                    source,
                    app,
                    phase: Arc::new(Mutex::new(phase)),
                    active_progress: Arc::new(RwLock::new(None)),
                    sessions: Arc::default(),
                    checks: Arc::default(),
                    tick_notify: Arc::clone(&tick_notify),
                    script_error,
                    current_generation,
                    install_seq,
                    seed,
                },
            );
        }

        Ok(registry)
    }

    /// Record that `name`'s install has just completed, placing it after
    /// every install completed before it, and return its place.
    // spec: ING#claims-across-apps
    pub fn record_install_completed(db: &Db, name: &AppName) -> rusqlite::Result<i64> {
        db.conn.query_row(
            "UPDATE registered_apps \
                SET install_seq = (SELECT COALESCE(MAX(install_seq), 0) + 1 FROM registered_apps) \
              WHERE name = ?1 \
             RETURNING install_seq",
            [name],
            |row| row.get(0),
        )
    }

    // spec: APPS#persistence
    pub fn persist_app(db: &Db, entry: &AppEntry) -> rusqlite::Result<()> {
        let (installed, uninstalling) = encode_phase(&entry.phase.lock());
        db.conn.execute(
            // spec: INFRA#database
            "INSERT INTO registered_apps \
                 (name, installed, uninstalling, current_generation) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(name) DO UPDATE SET \
                 installed = excluded.installed, \
                 uninstalling = excluded.uninstalling, \
                 current_generation = excluded.current_generation",
            rusqlite::params![
                entry.name,
                installed as i64,
                uninstalling as i64,
                entry.current_generation as i64,
            ],
        )?;
        Ok(())
    }

    pub fn remove_app(db: &Db, name: &AppName) -> rusqlite::Result<()> {
        db.conn
            .execute("DELETE FROM registered_apps WHERE name = ?1", [name])?;
        Ok(())
    }
}

/// Retrieve the definition active at a specific generation — its bundle and
/// provenance — or `None` when the app has no such generation.
// spec: APPS#definition-retrieval
pub fn definition_at_generation(
    db: &Db,
    app: &AppName,
    generation: generations::Generation,
) -> Result<Option<(Arc<Bundle>, Source)>, generations::Error> {
    // At-or-before resolution only applies to generations that exist (e.g.
    // param-change generations that share the previous definition); a
    // generation that was never recorded must not resolve to anything.
    if generations::get(db, app, generation)?.is_none() {
        return Ok(None);
    }
    let (hash, source) = match generations::definition_at(db, app, generation) {
        Ok(d) => d,
        Err(generations::Error::NotFound { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some((generations::load_bundle(db, &hash)?, source)))
}

/// Encode a phase as the `(installed, uninstalling)` pair stored in
/// `registered_apps`. `Uninstalling` sets `installed` alongside `uninstalling`
/// so that a halted migration or manual intervention reading only `installed`
/// still sees the app as "has been installed".
fn encode_phase(phase: &AppPhase) -> (bool, bool) {
    match phase {
        AppPhase::NotInstalled => (false, false),
        AppPhase::Installed => (true, false),
        AppPhase::Uninstalling => (true, true),
    }
}

/// Inverse of [`encode_phase`].
fn decode_phase(installed: bool, uninstalling: bool) -> AppPhase {
    if uninstalling {
        AppPhase::Uninstalling
    } else if installed {
        AppPhase::Installed
    } else {
        AppPhase::NotInstalled
    }
}

/// Update the phase in the database and then in the shared Arc, so the new
/// phase is observable only once it is durable.
///
/// A phase that cannot be persisted is an error and leaves the shared phase as
/// it was.
pub fn transition_phase(
    phase_arc: &Mutex<AppPhase>,
    new_phase: AppPhase,
    db: &Db,
    app_name: &AppName,
) -> rusqlite::Result<()> {
    write_phase(db, app_name, &new_phase)?;
    *phase_arc.lock() = new_phase;
    Ok(())
}

/// Persist the phase alone, for a caller that makes it observable once the
/// writes it belongs with are durable.
pub fn write_phase(db: &Db, app_name: &AppName, phase: &AppPhase) -> rusqlite::Result<()> {
    let (installed, uninstalling) = encode_phase(phase);
    db.conn.execute(
        "UPDATE registered_apps SET installed = ?1, uninstalling = ?2 WHERE name = ?3",
        rusqlite::params![installed as i64, uninstalling as i64, app_name],
    )?;
    Ok(())
}

/// Load both plaintext and secret params, merging into a single map.
/// Used before script evaluation, when schema isn't known yet.
///
/// Either table failing to read fails the whole call: a map missing the
/// values that could not be read would evaluate the script as though they
/// were unset.
// spec: PARAM#values
pub fn load_all_params_for_app(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut merged = params::load_params_for_app(db, app_name)?;
    merged.extend(secret_params::load_secret_params_for_app(
        db, cipher, app_name,
    )?);
    Ok(merged)
}

// spec: SEC#matching
/// The sensitive values of the app `app_name` holding `stored` under `def`:
/// those the definition and the stored values give, and every earlier value
/// of its secrets that its generation history records.
pub fn sensitive_values(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
    def: &crate::defs::app::AppDef,
    stored: &BTreeMap<String, String>,
) -> Result<crate::runtime::redact::SensitiveValues, generations::Error> {
    let mut values = def.sensitive(stored);
    let earlier = generations::earlier_secret_values(db, app_name, cipher, |name| {
        def.params
            .iter()
            .any(|(param, schema)| param.as_str() == name && schema.is_secret())
    })?;
    for value in &earlier {
        values.insert(value);
    }
    Ok(values)
}

/// Store or clear one parameter value, in whichever table its secrecy puts
/// it, leaving no copy in the other.
///
/// Every path that writes a parameter goes through here: two tables hold
/// them, and a value that landed in both, or in neither, is what disagreeing
/// call sites produce.
// spec: SEC#storage
pub fn store_param_value(
    db: &Db,
    cipher: &Cipher,
    app: &AppName,
    name: &ParamName,
    value: Option<&str>,
    is_secret: bool,
) -> rusqlite::Result<()> {
    match value {
        Some(v) if is_secret => {
            let secret = secrecy::SecretString::new(v.to_owned().into());
            secret_params::upsert_secret_param(db, cipher, app, name, &secret)?;
            delete_one_param(db, app, name)
        }
        Some(v) => {
            upsert_param(db, app, name, v)?;
            secret_params::delete_one_secret_param(db, app, name)
        }
        None => {
            delete_one_param(db, app, name)?;
            secret_params::delete_one_secret_param(db, app, name)
        }
    }
}

/// Re-evaluate `name`'s current definition with `params`, and move the stored
/// value of any param whose `secret` flag changed to the storage it now calls
/// for.
///
/// Migration follows only a definition that was applied: a reload that keeps
/// the previous one changes no param's `secret` flag.
// spec: SEC#migration
#[must_use = "a KeptPrevious reload must not be followed by state derived from the new values"]
pub fn reload(
    registry: &RwLock<AppRegistry>,
    db: &DbHandle,
    cipher: &Arc<Cipher>,
    name: &AppName,
    params: &BTreeMap<String, String>,
    limits: &crate::ScriptLimits,
) -> ReloadOutcome {
    // spec: LIM#breakers
    let Some((bundle, seed, generation)) = registry
        .read()
        .get(name.as_str())
        .map(|e| (Arc::clone(&e.bundle), e.seed.clone(), e.current_generation))
    else {
        return ReloadOutcome::NotRegistered;
    };
    let run = breakers::Run::new(bundle.hash(), params, Some(&seed), limits);
    let standing = {
        let (name, cipher, run) = (name.clone(), Arc::clone(cipher), run.clone());
        db.call(move |db| {
            breakers::standing(db, &cipher, &name, &breakers::Thing::Definition, &run)
        })
    };
    let outcome = match standing {
        Ok(breakers::Standing::Tripped) => registry.write().keep_previous(
            name,
            ScriptError::Runaway(breakers::tripped_message(&breakers::Thing::Definition)),
        ),
        Err(why) => registry.write().keep_previous(
            name,
            ScriptError::BreakerUnreadable(breakers::unreadable_message(
                &breakers::Thing::Definition,
                &why,
            )),
        ),
        Ok(standing) => {
            let outcome = registry.write().reevaluate(name, params, limits);
            let error = match &outcome {
                ReloadOutcome::KeptPrevious(e) => Some(e.clone()),
                _ => None,
            };
            let declared = registry
                .read()
                .get(name.as_str())
                .map(|e| e.app.def.load_full());
            if let Some(declared) = declared {
                let (name, cipher, limits) = (name.clone(), Arc::clone(cipher), limits.clone());
                db.call(move |db| {
                    record_definition_outcome(
                        db,
                        &cipher,
                        &name,
                        &run,
                        standing,
                        generation,
                        error.as_ref(),
                        &declared,
                        &limits,
                    );
                });
            }
            outcome
        }
    };
    if outcome.is_applied() {
        let running = registry.read().get(name.as_str()).map(|e| e.app.clone());
        if let Some(running) = running {
            let name = name.clone();
            let cipher = Arc::clone(cipher);
            db.call(move |db| {
                migrate_param_storage(db, &cipher, &name, &running, true);
                // spec: AUTO#clamping
                if let Err(e) = scaling::clamp_to_definition(db, &name, &running) {
                    tracing::error!(app = %name, error = %e, "failed to clamp scaling decisions");
                }
            });
        }
    }
    outcome
}

/// Record how a tracked evaluation of `name`'s definition with `run` ended:
/// trip its breaker where it ran past the limit, and otherwise note that it
/// finished in time, resetting its breaker where it was tripped for another
/// run, and the breakers of closures `declared` no longer holds.
// spec: LIM#breakers
#[expect(
    clippy::too_many_arguments,
    reason = "the outcome is these parts of one evaluation"
)]
pub fn record_definition_outcome(
    db: &Db,
    cipher: &Cipher,
    name: &AppName,
    run: &breakers::Run,
    standing: breakers::Standing,
    generation: u64,
    error: Option<&ScriptError>,
    declared: &crate::defs::app::AppDef,
    limits: &crate::ScriptLimits,
) {
    let thing = breakers::Thing::Definition;
    match error {
        Some(ScriptError::Runaway(_)) => {
            if let Err(e) = breakers::trip(db, cipher, name, &thing, run, generation, limits) {
                tracing::error!(app = %name, "cannot trip the definition's breaker: {e}");
            }
        }
        Some(ScriptError::BreakerUnreadable(_)) => {}
        Some(ScriptError::Failed { .. }) => {
            breakers::finished_in_time(db, cipher, name, &thing, run, standing);
        }
        None => {
            breakers::finished_in_time(db, cipher, name, &thing, run, standing);
            if let Err(e) = breakers::reset_undeclared(db, name, declared) {
                tracing::error!(app = %name, "cannot reset breakers of undeclared closures: {e}");
            }
        }
    }
}

/// Move each declared parameter's stored value to the storage its definition
/// now calls for: secret storage for a secret parameter, plain storage for one
/// that is not.
///
/// `evaluated_in_full` is false for a definition that threw part-way: a
/// parameter it declared before throwing may have been going to be marked
/// secret by a call it never reached, so none is moved out of secret storage.
// spec: SEC#migration
pub(crate) fn migrate_param_storage(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
    app: &App,
    evaluated_in_full: bool,
) {
    let def = app.def.load();
    for (param_name, param_def) in &def.params {
        let (moved, to) = if param_def.is_secret() {
            (
                secret_params::migrate_to_secret(db, cipher, app_name, param_name),
                "secret",
            )
        } else if evaluated_in_full {
            (
                secret_params::migrate_to_plain(db, cipher, app_name, param_name),
                "plain",
            )
        } else {
            continue;
        };
        if let Err(e) = moved {
            tracing::warn!(
                app = %app_name,
                param = %param_name,
                "failed to migrate param to {to} storage: {e}"
            );
        }
    }
}

/// A parameter whose kind or validator rejected the proposed values.
// spec: PRM#validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub name: String,
    pub reason: String,
    /// The validator ran past the evaluation time limit, so it neither
    /// accepted nor rejected the value.
    // spec: PARAM#validators
    pub runaway: bool,
}

/// Evaluate a definition with `params`.
///
/// The returned `App` is populated up to wherever the script threw; see
/// [`ReloadOutcome`] for why callers must not publish a failed one over a
/// good one.
pub fn evaluate(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    seed: &crate::runtime::seeds::AppSeed,
    limits: &crate::ScriptLimits,
) -> (App, Option<ScriptError>) {
    let (app, result) = run_definition(name, bundle, params, seed, None, limits);
    (app, result.err())
}

/// Evaluate a definition with `params`, then check the kind of every
/// parameter set in `proposed` and, if every kind accepts its value, run the
/// validator of each, against `proposed`.
///
/// `params` and `proposed` are the same map except when validators are taken
/// from an earlier evaluation because the proposed values fail to evaluate.
// spec: PRM#validation
pub fn evaluate_validated(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    proposed: &BTreeMap<String, SuppliedValue>,
    seed: &crate::runtime::seeds::AppSeed,
    limits: &crate::ScriptLimits,
) -> (App, Result<Vec<Rejection>, ScriptError>) {
    run_definition(name, bundle, params, seed, Some(proposed), limits)
}

/// Evaluate a lone script, as a one-file bundle.
#[cfg(test)]
pub fn evaluate_script(
    name: &AppName,
    script: &str,
    params: &BTreeMap<String, String>,
    limits: &crate::ScriptLimits,
) -> (App, Option<ScriptError>) {
    evaluate(
        name,
        &Arc::new(Bundle::from_stored_script(script)),
        params,
        &crate::runtime::seeds::AppSeed::for_tests(),
        limits,
    )
}

/// Evaluate a definition against `seed`, recording on its definition what the
/// evaluation derived from the seed.
// spec: RND#seeds
fn run_definition(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    seed: &crate::runtime::seeds::AppSeed,
    validate: Option<&BTreeMap<String, SuppliedValue>>,
    limits: &crate::ScriptLimits,
) -> (App, Result<Vec<Rejection>, ScriptError>) {
    let frame = crate::runtime::seeds::AppSeedFrame::enter(seed);
    let (mut app, mut result) = run_seeded_definition(name, bundle, params, validate, limits);
    app.seed = Some(seed.clone());
    // spec: SEC#redaction
    // A parameter's secret flag is settled only once the script has run, so
    // the helper results computed from parameter values are withheld now.
    let secret: std::collections::BTreeSet<String> = app
        .def
        .load()
        .params
        .iter()
        .filter(|(_, def)| def.is_secret())
        .map(|(name, _)| name.to_string())
        .collect();
    let derived = match frame.take_derived(|p| secret.contains(p), limits.max_derived_values) {
        Ok(derived) => derived,
        Err(e) => {
            if result.is_ok() {
                result = Err(ScriptError::failed(e));
            }
            crate::runtime::redact::SensitiveValues::default()
        }
    };
    // Most definitions derive nothing, and their definition is left as it is.
    if !derived.is_empty() {
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.derived = derived.clone();
            d
        });
    }
    // spec: SEC#withheld-values
    // What the script threw, or a validator rejected with, may quote any
    // value it could read: the app's sensitive values as this evaluation
    // sees them, and those of the proposed values it validated.
    let mut sensitive = app.sensitive();
    if let Some(proposed) = validate {
        let proposed = proposed
            .iter()
            .map(|(name, value)| (name.clone(), value.canonical()))
            .collect();
        sensitive.extend(&app.def.load().sensitive(&proposed));
    }
    let result = match result {
        Ok(rejections) => Ok(rejections
            .into_iter()
            .map(|r| Rejection {
                reason: sensitive.mask(&r.reason).into_owned(),
                ..r
            })
            .collect()),
        Err(e) => Err(e.map(|e| sensitive.mask(&e).into_owned())),
    };
    (app, result)
}

fn run_seeded_definition(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    validate: Option<&BTreeMap<String, SuppliedValue>>,
    limits: &crate::ScriptLimits,
) -> (App, Result<Vec<Rejection>, ScriptError>) {
    let (engine, mut scope, app) = match setup_language(limits, Arc::clone(bundle)) {
        Ok(language) => language,
        // spec: CONST#host-constants
        // A host constant the evaluation cannot stand without fails it, so
        // it declares nothing.
        Err(e) => {
            return (
                App::with_bundle(Arc::clone(bundle)),
                Err(ScriptError::failed(e)),
            );
        }
    };
    let _evaluation = app.guard_evaluation();
    let script = match bundle.script() {
        Ok(s) => s,
        // A stored definition that no longer reads as one (only possible
        // for a bundle written for another Seedling) evaluates to nothing.
        Err(e) => return (app, Err(ScriptError::failed(e.to_string()))),
    };
    // spec: PRM#parameter-storage — pre-populate stored values so is_set()/value() work
    // during script evaluation. AppDef.params (the BSL-declared set) is
    // populated by the script itself via app.param() calls.
    *app.stored.lock() = params.clone();
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        d.name = name.clone();
        d
    });
    crate::defs::app::set_appdef_holder(&app.def);
    if validate.is_some() {
        crate::defs::app::begin_validator_capture();
    }
    // spec: BSL#evaluation
    // Unhandled Rhai exceptions bubble up from `run_ast_with_scope` and stop
    // further execution of this script evaluation. Rhai's native try/catch
    // is available to BSL authors for recovery; anything that escapes it
    // becomes a ScriptError and is surfaced as a fault by the caller.
    // spec: BND#script-formation
    let remap = |e: Box<rhai::EvalAltResult>| ScriptError::raised(script, &e);
    // spec: LIM#measuring-evaluation-time
    let evaluation = crate::evaluation::Evaluation::start(limits);
    let ran = crate::defs::compile(&engine, script.text())
        .map_err(remap)
        .and_then(|ast| {
            engine
                .run_ast_with_scope(&mut scope, &ast)
                .map(|()| ast)
                .map_err(remap)
        });
    crate::defs::app::clear_appdef_holder();
    let validators = validate.map(|_| crate::defs::app::end_validator_capture());
    // spec: LIM#measuring-evaluation-time
    // An evaluation that returns having run past the limit fails all the
    // same, whatever it returned.
    if evaluation.exceeded() {
        return (
            app,
            Err(ScriptError::Runaway(
                crate::evaluation::out_of_time_message(limits),
            )),
        );
    }
    drop(evaluation);
    let ast = match ran {
        Ok(ast) => ast,
        Err(e) => return (app, Err(e)),
    };
    // spec: BSL#evaluation
    if let Err(e) = crate::defs::finish_evaluation(&app, limits) {
        return (app, Err(ScriptError::failed(e)));
    }
    let (Some(proposed), Some(mut validators)) = (validate, validators) else {
        return (app, Ok(Vec::new()));
    };
    let parsed = match check_kinds(&app.def.load().params, proposed, &limits.helpers()) {
        Ok(parsed) => parsed,
        // spec: PARAM#kind-checks
        // No validator runs while any kind rejects its value.
        Err(rejections) => return (app, Ok(rejections)),
    };
    validators.retain(|name, _| parsed.contains_key(name.as_str()));
    let mut rejections = run_validators(&engine, &ast, script, &validators, parsed, limits);
    rejections.sort_by(|a, b| a.name.cmp(&b.name));
    (app, Ok(rejections))
}

/// Stored values, as values supplied as strings.
pub fn supplied_values(values: &BTreeMap<String, String>) -> BTreeMap<String, SuppliedValue> {
    values
        .iter()
        .map(|(k, v)| (k.clone(), SuppliedValue::Text(v.clone())))
        .collect()
}

/// Each value as the text it is stored as.
// spec: PRM#parameter-storage
pub fn canonical_values(values: &BTreeMap<String, SuppliedValue>) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(k, v)| (k.clone(), v.canonical()))
        .collect()
}

/// Check every parameter set in `proposed` against the kind `declared` gives
/// it, giving each declared one's parser result, or a rejection for each
/// value its kind refuses and each boolean or integer given for a parameter
/// that is not declared.
// spec: PARAM#kind-checks
// spec: PRM#validation
fn check_kinds(
    declared: &BTreeMap<seedling_protocol::names::ParamName, crate::defs::param_def::ParamDef>,
    proposed: &BTreeMap<String, SuppliedValue>,
    limits: &crate::defs::stdlib::HelperLimits,
) -> Result<BTreeMap<String, ParsedValue>, Vec<Rejection>> {
    let mut parsed = BTreeMap::new();
    let mut rejections = Vec::new();
    // spec: REX#compiled-size
    let mut budget = Budget::of(limits);
    for (name, value) in proposed {
        // An unset parameter is not checked; an empty stored value is.
        let result = match declared.get(name.as_str()) {
            Some(def) => def.kind.parse_supplied(value, &mut budget),
            None if matches!(value, SuppliedValue::Text(_)) => continue,
            None => Err(
                "a boolean or integer is given for a parameter the definition does not \
                 declare"
                    .to_owned(),
            ),
        };
        match result {
            Ok(value) => {
                parsed.insert(name.clone(), value);
            }
            Err(reason) => rejections.push(Rejection {
                name: name.clone(),
                reason,
                runaway: false,
            }),
        }
    }
    if rejections.is_empty() {
        Ok(parsed)
    } else {
        Err(rejections)
    }
}

// spec: PARAM#validators
fn run_validators(
    engine: &rhai::Engine,
    ast: &rhai::AST,
    script: &seedling_bundle::Script,
    validators: &BTreeMap<seedling_protocol::names::ParamName, rhai::FnPtr>,
    parsed: BTreeMap<String, ParsedValue>,
    limits: &crate::ScriptLimits,
) -> Vec<Rejection> {
    let values: rhai::Map = parsed
        .into_iter()
        .map(|(k, v)| (k.as_str().into(), v.into_dynamic()))
        .collect();
    let mut rejections = Vec::new();
    for (name, validator) in validators {
        // An unset parameter always passes; `required` governs presence.
        let Some(value) = values.get(name.as_str()) else {
            continue;
        };
        let _frame = crate::defs::app::ValidatorFrame::enter();
        // spec: LIM#measuring-evaluation-time
        // Each call of a validator is an evaluation of its own.
        let evaluation = crate::evaluation::Evaluation::start(limits);
        let result: Result<rhai::Dynamic, _> =
            validator.call(engine, ast, (value.clone(), values.clone()));
        // spec: PARAM#validators
        if evaluation.exceeded() {
            rejections.push(Rejection {
                name: name.as_str().to_owned(),
                reason: crate::evaluation::out_of_time_message(limits),
                runaway: true,
            });
            continue;
        }
        drop(evaluation);
        if let Err(e) = result {
            // A thrown value is the reason, whether the validator threw it or
            // a function it called did, as a kind parser does; any other
            // failure rejects too, with the error itself as the reason.
            let mut cause = &*e;
            while let rhai::EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = cause {
                cause = inner;
            }
            let reason = match cause {
                rhai::EvalAltResult::ErrorRuntime(thrown, _) => thrown.to_string(),
                _ => script.remap_error(&e.to_string()),
            };
            rejections.push(Rejection {
                name: name.as_str().to_owned(),
                reason,
                runaway: false,
            });
        }
    }
    rejections
}

#[cfg(test)]
mod tests;
