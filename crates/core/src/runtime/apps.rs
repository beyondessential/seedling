use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use jiff::Timestamp;
use parking_lot::{Mutex, RwLock};
use seedling_protocol::names::{ActionName, AppName, ParamName};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::{
    defs::app::App,
    runtime::{
        db::{Db, DbHandle},
        definition::{Bundle, Source},
        desired::OperationProgress,
        generations,
        secrets::Cipher,
    },
    setup_language,
};

mod params;
mod registry_faults;
pub mod secret_params;
pub mod status;

pub use params::{
    delete_app_params, delete_one_param, load_params_for_app, sync_script_error_fault, upsert_param,
};
pub use registry_faults::{sync_registry_faults, sync_used_registries};

#[derive(Debug)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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
    Installing,
    Installed,
    Uninstalling,
}

// spec: APPS#status-derivation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    NotInstalled,
    Installing,
    Uninstalling,
    Operating { action_name: ActionName },
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
    /// Wakes the reconciler for an immediate tick.
    pub tick_notify: Arc<Notify>,
    /// Active script-evaluation fault, if the last reload failed.
    /// Cleared on the next successful evaluation.
    pub script_error: Option<(String, Timestamp)>,
    /// Current app generation (0 if not yet bumped).
    // spec: APPS#generations
    pub current_generation: generations::Generation,
    /// Where the app's latest completed install stands in the order installs
    /// completed, lower first; `None` before any install has completed.
    // spec: ING#claims-across-apps
    pub install_seq: Option<i64>,
    /// The app's start action has been initiated and has not yet ended,
    /// including while it is queued. Until it ends, the app's desired state is
    /// held as for an app with an operation in progress.
    // spec: RCN#desired-state
    pub start_pending: Arc<std::sync::atomic::AtomicBool>,
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

    // spec: APPS#registration
    pub fn register(
        &mut self,
        name: AppName,
        bundle: Arc<Bundle>,
        source: Source,
        tick_notify: Arc<Notify>,
        limits: &crate::ScriptLimits,
    ) -> Result<(), ScriptError> {
        let (app, script_error) = evaluate(&name, &bundle, &BTreeMap::new(), limits);
        self.insert_registered(name, bundle, source, app, script_error, tick_notify, 0);
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
    ) {
        let script_error = script_error.map(|e| {
            tracing::warn!(app = %name, error = %e, "script has errors at registration; params may need to be set");
            (e.to_string(), Timestamp::now())
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
                tick_notify,
                script_error,
                current_generation: generation,
                install_seq: None,
                start_pending: Arc::default(),
            },
        );
    }

    pub fn deregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    // spec: APPS#update
    // spec: PRM#parameter-set
    // spec: PRM#parameter-unset
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
        let Some(bundle) = self
            .entries
            .get(name.as_str())
            .map(|e| Arc::clone(&e.bundle))
        else {
            return ReloadOutcome::NotRegistered;
        };
        let (app, raw_error) = evaluate(name, &bundle, params, limits);
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
                entry.script_error = Some((e.to_string(), Timestamp::now()));
                ReloadOutcome::KeptPrevious(e)
            }
        }
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
            "SELECT name, installed, uninstalling, installing, current_generation, install_seq \
             FROM registered_apps ORDER BY name",
        )?;
        type Row = (AppName, bool, bool, bool, i64, Option<i64>);
        let rows: Vec<Row> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, AppName>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;

        for (name, installed, uninstalling, installing, current_gen, install_seq) in rows {
            let phase = decode_phase(name.as_str(), installed, uninstalling, installing);
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
            // spec: APPS#persistence — reload runs no validators: the stored
            // combination was validated when it was written.
            let (app, raw_error) = evaluate(&name, &bundle, &stored, limits);
            // spec: SEC#migration — after the script declares which params are secret,
            // migrate any plaintext rows that should now be encrypted.
            migrate_newly_secret_params(db, cipher, &name, &app);
            let script_error = raw_error.map(|e| {
                tracing::warn!("failed to reload script for app '{name}': {e}");
                (e.to_string(), Timestamp::now())
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
                    tick_notify: Arc::clone(&tick_notify),
                    script_error,
                    current_generation,
                    install_seq,
                    start_pending: Arc::default(),
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
        let (installed, uninstalling, installing) = encode_phase(&entry.phase.lock());
        db.conn.execute(
            // spec: INFRA#database
            "INSERT INTO registered_apps \
                 (name, installed, uninstalling, installing, current_generation) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(name) DO UPDATE SET \
                 installed = excluded.installed, \
                 uninstalling = excluded.uninstalling, \
                 installing = excluded.installing, \
                 current_generation = excluded.current_generation",
            rusqlite::params![
                entry.name,
                installed as i64,
                uninstalling as i64,
                installing as i64,
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

/// Encode a phase as the `(installed, uninstalling, installing)` triple stored
/// in `registered_apps`. Exactly one of the three is ever set, except that
/// `Uninstalling` sets `installed` alongside `uninstalling` so that a halted
/// migration or manual intervention reading only `installed` still sees the app
/// as "has been installed".
fn encode_phase(phase: &AppPhase) -> (bool, bool, bool) {
    match phase {
        AppPhase::NotInstalled => (false, false, false),
        AppPhase::Installing => (false, false, true),
        AppPhase::Installed => (true, false, false),
        AppPhase::Uninstalling => (true, true, false),
    }
}

/// Inverse of [`encode_phase`]. Tolerates inconsistent triples (e.g. both
/// `installed` and `installing` set) by preferring the earliest-in-state-
/// -machine interpretation and logging a warning — a defensive arm that
/// should not trigger under the invariant that the encoder is the only
/// writer.
fn decode_phase(app: &str, installed: bool, uninstalling: bool, installing: bool) -> AppPhase {
    if uninstalling {
        return AppPhase::Uninstalling;
    }
    if installing {
        if installed {
            tracing::warn!(
                app,
                "registered_apps row has both installed=1 and installing=1; treating as Installing"
            );
        }
        return AppPhase::Installing;
    }
    if installed {
        AppPhase::Installed
    } else {
        AppPhase::NotInstalled
    }
}

/// Update the phase both in the shared Arc and in the database.
pub fn transition_phase(
    phase_arc: &Mutex<AppPhase>,
    new_phase: AppPhase,
    db: &Db,
    app_name: &AppName,
    _script: &str,
) {
    *phase_arc.lock() = new_phase.clone();
    let (installed, uninstalling, installing) = encode_phase(&new_phase);
    if let Err(e) = db.conn.execute(
        "UPDATE registered_apps SET installed = ?1, uninstalling = ?2, installing = ?3 WHERE name = ?4",
        rusqlite::params![
            installed as i64,
            uninstalling as i64,
            installing as i64,
            app_name,
        ],
    ) {
        tracing::error!(app = %app_name, "failed to persist phase transition: {e}");
    }
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
/// value of any param the definition now declares secret into secret storage.
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
    let outcome = registry.write().reevaluate(name, params, limits);
    if outcome.is_applied() {
        let running = registry.read().get(name.as_str()).map(|e| e.app.clone());
        if let Some(running) = running {
            let name = name.clone();
            let cipher = Arc::clone(cipher);
            db.call(move |db| migrate_newly_secret_params(db, &cipher, &name, &running));
        }
    }
    outcome
}

// spec: SEC#migration
pub(crate) fn migrate_newly_secret_params(db: &Db, cipher: &Cipher, app_name: &AppName, app: &App) {
    let def = app.def.load();
    for (param_name, param_def) in &def.params {
        if param_def.is_secret()
            && let Err(e) = secret_params::migrate_to_secret(db, cipher, app_name, param_name)
        {
            tracing::warn!(
                app = %app_name,
                param = %param_name,
                "failed to migrate param to secret storage: {e}"
            );
        }
    }
}

/// A parameter whose validator rejected the proposed values.
// spec: PRM#validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub name: String,
    pub reason: String,
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
    limits: &crate::ScriptLimits,
) -> (App, Option<ScriptError>) {
    let (app, result) = run_definition(name, bundle, params, None, limits);
    (app, result.err())
}

/// Evaluate a definition with `params`, then run the validator of every
/// parameter set in `proposed` against `proposed`.
///
/// `params` and `proposed` are the same map except when validators are taken
/// from an earlier evaluation because the proposed values fail to evaluate.
// spec: PRM#validation
pub fn evaluate_validated(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    proposed: &BTreeMap<String, String>,
    limits: &crate::ScriptLimits,
) -> (App, Result<Vec<Rejection>, ScriptError>) {
    run_definition(name, bundle, params, Some(proposed), limits)
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
        limits,
    )
}

fn run_definition(
    name: &AppName,
    bundle: &Arc<Bundle>,
    params: &BTreeMap<String, String>,
    validate: Option<&BTreeMap<String, String>>,
    limits: &crate::ScriptLimits,
) -> (App, Result<Vec<Rejection>, ScriptError>) {
    let (engine, mut scope, app) = setup_language(limits, Arc::clone(bundle));
    let script = match bundle.script() {
        Ok(s) => s,
        // A stored definition that no longer reads as one (only possible
        // for a bundle written for another Seedling) evaluates to nothing.
        Err(e) => return (app, Err(ScriptError(e.to_string()))),
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
    let remap = |e: Box<rhai::EvalAltResult>| ScriptError(script.remap_error(&e.to_string()));
    let ran = engine
        .compile(script.text())
        .map_err(|e| remap(e.into()))
        .and_then(|ast| {
            engine
                .run_ast_with_scope(&mut scope, &ast)
                .map(|()| ast)
                .map_err(remap)
        });
    crate::defs::app::clear_appdef_holder();
    let validators = validate.map(|_| crate::defs::app::end_validator_capture());
    let ast = match ran {
        Ok(ast) => ast,
        Err(e) => return (app, Err(e)),
    };
    // spec: BSL#evaluation
    if let Err(e) = crate::defs::check_evaluated(&app.def.load()) {
        return (app, Err(ScriptError(e)));
    }
    let (Some(proposed), Some(validators)) = (validate, validators) else {
        return (app, Ok(Vec::new()));
    };
    let rejections = run_validators(&engine, &ast, script, &validators, proposed);
    (app, Ok(rejections))
}

// spec: PARAM#validators
fn run_validators(
    engine: &rhai::Engine,
    ast: &rhai::AST,
    script: &crate::runtime::definition::Script,
    validators: &BTreeMap<seedling_protocol::names::ParamName, rhai::FnPtr>,
    proposed: &BTreeMap<String, String>,
) -> Vec<Rejection> {
    let values: rhai::Map = proposed
        .iter()
        .map(|(k, v)| (k.as_str().into(), rhai::Dynamic::from(v.clone())))
        .collect();
    let mut rejections = Vec::new();
    for (name, validator) in validators {
        // An unset parameter always passes; `required` governs presence.
        let Some(value) = proposed.get(name.as_str()) else {
            continue;
        };
        let _frame = crate::defs::app::ValidatorFrame::enter();
        let result: Result<rhai::Dynamic, _> =
            validator.call(engine, ast, (value.clone(), values.clone()));
        if let Err(e) = result {
            // A thrown value is the reason; any other failure rejects too,
            // with the error itself as the reason.
            let reason = match *e {
                rhai::EvalAltResult::ErrorRuntime(ref thrown, _) => thrown.to_string(),
                ref other => script.remap_error(&other.to_string()),
            };
            rejections.push(Rejection {
                name: name.as_str().to_owned(),
                reason,
            });
        }
    }
    rejections
}

#[cfg(test)]
mod tests;
