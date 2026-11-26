//! Breakers: what keeps the runtime from evaluating again, with the same
//! definition and inputs, an app's definition or action closure that ran past
//! the evaluation time limit.
//!
//! A breaker is tripped exactly while its `evaluation_runaway` fault is
//! active. Its row records the digest of the definition and inputs that
//! tripped it, and means nothing once the fault has cleared, so clearing the
//! fault is the whole of resetting the breaker, and no write leaves the two
//! out of step.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use jiff::Timestamp;
use parking_lot::Mutex;
use rusqlite::OptionalExtension as _;
use seedling_protocol::names::{ActionName, AppName, GateName, ShellName};

use crate::runtime::db::Db;
use crate::runtime::faults::{self, FaultKey, FaultMeta};
use crate::runtime::secrets::{Cipher, KeyDigest};

pub const EVALUATION_RUNAWAY: &str = "evaluation_runaway";
pub const BREAKER_STATE_UNREADABLE: &str = "breaker_state_unreadable";

/// Domain separation for breaker digests, which share the secret key's
/// digest subkey with gate keys.
const DIGEST_DOMAIN: &[u8] = b"seedling breaker v1\0";

/// The thing a breaker is kept for.
// spec: LIM#breakers
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Thing {
    Definition,
    Action(ActionName),
    Shell(ShellName),
    Gate(GateName),
}

impl Thing {
    /// The subject of the thing's faults, naming its kind as well as its
    /// name, so that an action and a gate of one name are separate subjects.
    // spec: FLT#app-faults
    pub fn subject(&self) -> String {
        match self {
            Self::Definition => String::new(),
            Self::Action(name) => format!("action:{name}"),
            Self::Shell(name) => format!("shell:{name}"),
            Self::Gate(name) => format!("gate:{name}"),
        }
    }

    /// The thing a fault subject names, or `None` for one no thing writes.
    pub fn from_subject(subject: &str) -> Option<Self> {
        if subject.is_empty() {
            return Some(Self::Definition);
        }
        let (kind, name) = subject.split_once(':')?;
        match kind {
            "action" => ActionName::new(name).ok().map(Self::Action),
            "shell" => ShellName::new(name).ok().map(Self::Shell),
            "gate" => GateName::new(name).ok().map(Self::Gate),
            _ => None,
        }
    }

    /// Whether a fault about this thing holds back what the app needs.
    // spec: FLT#withholding-faults
    pub fn withholds(&self) -> bool {
        matches!(self, Self::Definition | Self::Gate(_))
    }

    /// How the thing's faults name it for display: the action, shell or gate
    /// by kind and name, and the definition not at all.
    // spec: FLTS#fault-records
    fn fault_meta(&self) -> FaultMeta {
        match self {
            Self::Definition => FaultMeta::default(),
            Self::Action(name) => FaultMeta::resource("action", name.as_str()),
            Self::Shell(name) => FaultMeta::resource("shell", name.as_str()),
            Self::Gate(name) => FaultMeta::resource("gate", name.as_str()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Definition => "the app's definition".to_owned(),
            Self::Action(name) => format!("action `{name}`"),
            Self::Shell(name) => format!("shell `{name}`"),
            Self::Gate(name) => format!("the body of gate `{name}`"),
        }
    }
}

/// The definition and inputs one evaluation of a thing runs with.
///
/// Each input is kept by a label that names it without its value, so that a
/// fault can say which inputs differ.
// spec: LIM#breakers
#[derive(Clone)]
pub struct Run {
    definition: String,
    inputs: BTreeMap<String, Vec<u8>>,
}

impl Run {
    /// An evaluation of the definition `bundle_hash` names, with the inputs
    /// every thing has: the app's stored parameter values, its host
    /// constants, its app seed, the running Seedling version and the limit
    /// in force.
    pub fn new(
        bundle_hash: &str,
        stored: &BTreeMap<String, String>,
        seed: Option<&crate::runtime::seeds::AppSeed>,
        limits: &crate::ScriptLimits,
    ) -> Self {
        let mut inputs = BTreeMap::new();
        for (name, value) in stored {
            inputs.insert(format!("parameter `{name}`"), value.as_bytes().to_vec());
        }
        for (name, value) in host_constants() {
            inputs.insert(format!("host constant `{name}`"), value.into_bytes());
        }
        let seed = match seed {
            Some(crate::runtime::seeds::AppSeed::Ready(seed)) => seed.expose().to_vec(),
            Some(crate::runtime::seeds::AppSeed::Unreadable(_)) => b"unreadable".to_vec(),
            None => Vec::new(),
        };
        inputs.insert("the app seed".to_owned(), seed);
        inputs.insert(
            "the Seedling version".to_owned(),
            env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
        );
        inputs.insert(
            "the evaluation time limit".to_owned(),
            limits.max_evaluation_time.to_be_bytes().to_vec(),
        );
        Self {
            definition: bundle_hash.to_owned(),
            inputs,
        }
    }

    /// The same run of `app`: its definition, stored values and seed.
    pub fn of_app(app: &crate::defs::app::App, limits: &crate::ScriptLimits) -> Self {
        Self::new(
            app.bundle.hash(),
            &app.stored.lock(),
            app.seed.as_ref(),
            limits,
        )
    }

    /// Add the params of an invocation or session, in their canonical form,
    /// leaving out volume params, which the runtime replaces with keys of its
    /// own when it dispatches the invocation, and the keys it inserts.
    pub fn with_params(
        mut self,
        params: &crate::runtime::param_map::ParamMap,
        schema: &BTreeMap<seedling_protocol::names::ParamName, crate::defs::param_def::ParamDef>,
    ) -> Self {
        for (name, value) in params.values() {
            let volume = schema
                .get(name.as_str())
                .is_some_and(|p| p.kind == crate::defs::param_def::ParamKind::Volume);
            if volume || crate::runtime::action_params::is_reserved(name) {
                continue;
            }
            let typed = params.types().get(name).map(|t| format!("{t:?}"));
            let canonical = serde_json::json!([value, typed]).to_string();
            self.inputs
                .insert(format!("param `{name}`"), canonical.into_bytes());
        }
        self
    }

    /// Add the app a gate's body is passed as `last`: the definition and
    /// values of the gate's last success, or none.
    pub fn with_last_success(mut self, last: Option<&crate::defs::app::App>) -> Self {
        let value = last
            .map(|last| {
                serde_json::json!([last.bundle.hash(), *last.stored.lock()])
                    .to_string()
                    .into_bytes()
            })
            .unwrap_or_default();
        self.inputs
            .insert("the gate's last success".to_owned(), value);
        self
    }

    // spec: LIM#breakers
    fn digest(&self, cipher: &Cipher) -> Result<KeyDigest, String> {
        let mut bytes = DIGEST_DOMAIN.to_vec();
        push_field(&mut bytes, self.definition.as_bytes());
        for (label, value) in &self.inputs {
            push_field(&mut bytes, label.as_bytes());
            push_field(&mut bytes, value);
        }
        cipher
            .key_digest(&bytes)
            .map_err(|e| format!("cannot digest the evaluation's inputs: {e:?}"))
    }

    fn input_digests(&self, cipher: &Cipher) -> BTreeMap<String, Option<KeyDigest>> {
        self.inputs
            .iter()
            .map(|(label, value)| {
                let mut bytes = DIGEST_DOMAIN.to_vec();
                push_field(&mut bytes, label.as_bytes());
                push_field(&mut bytes, value);
                (label.clone(), cipher.key_digest(&bytes).ok())
            })
            .collect()
    }
}

fn push_field(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
    bytes.extend_from_slice(field);
}

/// Each host constant by its name, as an evaluation sees it.
// spec: CONST#host-constants
fn host_constants() -> Vec<(&'static str, String)> {
    let facts = crate::sysconst::get();
    vec![
        (
            "AVAILABLE_THREADS",
            format!("{:?}", facts.available_threads),
        ),
        ("AVAILABLE_MEMORY", format!("{:?}", facts.available_memory)),
        ("CPU_ARCHITECTURE", std::env::consts::ARCH.to_owned()),
        ("HOST_HAS_IPV4", facts.ipv4_egress.to_string()),
        ("HOST_HAS_IPV6", facts.ipv6_egress.to_string()),
        ("NAT64_ACTIVE", facts.nat64_active.to_string()),
        ("HAS_SNAPSHOTS", facts.has_snapshots.to_string()),
        ("NODE_NAME", format!("{:?}", facts.node_name)),
        ("TIMEZONE", facts.timezone.clone()),
    ]
}

/// Where a thing's breaker stands for one run of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// The breaker is not tripped.
    Clear,
    /// The breaker is tripped for another definition or other inputs, so
    /// this run evaluates as usual.
    TrippedForOther,
    /// The breaker is tripped for this run's definition and inputs.
    Tripped,
}

/// Where `thing`'s breaker stands for `run`, or why that cannot be read.
///
/// A failed read is never taken for a clear breaker.
// spec: LIM#breakers
fn read_standing(
    db: &Db,
    cipher: &Cipher,
    app: &AppName,
    thing: &Thing,
    run: &Run,
) -> Result<Standing, String> {
    let subject = thing.subject();
    let active: bool = db
        .conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM faults
             WHERE app = ?1 AND kind = ?2 AND subject = ?3 AND cleared_at IS NULL)",
            rusqlite::params![app, EVALUATION_RUNAWAY, subject],
            |row| row.get(0),
        )
        .map_err(|e| format!("cannot read the breaker's fault: {e}"))?;
    if !active {
        return Ok(Standing::Clear);
    }
    let recorded: Option<Vec<u8>> = db
        .conn
        .query_row(
            "SELECT digest FROM breakers WHERE app = ?1 AND subject = ?2",
            rusqlite::params![app, subject],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("cannot read the breaker: {e}"))?;
    let recorded =
        recorded.ok_or_else(|| "the breaker's fault is active but it has no record".to_owned())?;
    let recorded = KeyDigest::from_bytes(&recorded)
        .ok_or_else(|| "the breaker's record holds no digest".to_owned())?;
    Ok(if recorded == run.digest(cipher)? {
        Standing::Tripped
    } else {
        Standing::TrippedForOther
    })
}

/// Where `thing`'s breaker stands for `run`, holding
/// `breaker_state_unreadable` while it cannot be read and clearing it once
/// it can.
// spec: LIM#breakers
// spec: FLT#app-faults
pub fn standing(
    db: &Db,
    cipher: &Cipher,
    app: &AppName,
    thing: &Thing,
    run: &Run,
) -> Result<Standing, String> {
    let key = FaultKey::new(app, BREAKER_STATE_UNREADABLE, thing.subject());
    match read_standing(db, cipher, app, thing, run) {
        Ok(standing) => {
            if let Err(e) = faults::clear_fault_by_key(db, &key) {
                tracing::error!(%app, "cannot clear {BREAKER_STATE_UNREADABLE}: {e}");
            }
            Ok(standing)
        }
        Err(why) => {
            tracing::error!(%app, "cannot read the breaker of {}: {why}", thing.describe());
            let description = format!(
                "cannot read whether the breaker of {} is tripped, so it is not evaluated: {why}",
                thing.describe()
            );
            if let Err(e) = faults::file_once(db, &key, &thing.fault_meta(), &description) {
                tracing::error!(%app, "cannot file {BREAKER_STATE_UNREADABLE}: {e}");
            }
            Err(why)
        }
    }
}

/// Trip `thing`'s breaker for `run`, which ran past the limit at
/// `generation`, recording its definition and inputs in place of any it
/// held, and file its fault.
// spec: LIM#breakers
pub fn trip(
    db: &Db,
    cipher: &Cipher,
    app: &AppName,
    thing: &Thing,
    run: &Run,
    generation: u64,
    limits: &crate::ScriptLimits,
) -> Result<(), String> {
    let subject = thing.subject();
    let digest = run.digest(cipher)?;
    // The record is written before the fault, so the breaker is tripped, at
    // the fault, only once it records what tripped it.
    db.conn
        .execute(
            "INSERT INTO breakers (app, subject, digest, generation, tripped_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (app, subject) DO UPDATE SET
                 digest = excluded.digest,
                 generation = excluded.generation,
                 tripped_at = excluded.tripped_at",
            rusqlite::params![
                app,
                subject,
                digest.as_bytes(),
                generation as i64,
                Timestamp::now().to_string()
            ],
        )
        .map_err(|e| format!("cannot record the breaker: {e}"))?;
    let description = format!(
        "{} ran past the evaluation time limit of {}s at generation {generation}, and is not \
         evaluated again with the same definition and inputs; {}",
        thing.describe(),
        limits.max_evaluation_time,
        likely_cause(cipher, app, thing, run),
    );
    tracing::error!(%app, "{description}");
    let key = FaultKey::new(app, EVALUATION_RUNAWAY, subject);
    let filed = faults::file_once(db, &key, &thing.fault_meta(), &description)
        .map_err(|e| format!("cannot file {EVALUATION_RUNAWAY}: {e}"))?;
    if !filed {
        faults::update_description(db, &key, &description)
            .map_err(|e| format!("cannot update {EVALUATION_RUNAWAY}: {e}"))?;
    }
    Ok(())
}

/// Reset `thing`'s breaker, clearing its fault, after an evaluation of it
/// finished within the limit.
// spec: LIM#breakers
pub fn reset(db: &Db, app: &AppName, thing: &Thing) -> Result<(), String> {
    let key = FaultKey::new(app, EVALUATION_RUNAWAY, thing.subject());
    faults::clear_fault_by_key(db, &key).map_err(|e| format!("cannot reset the breaker: {e}"))?;
    forget_row(db, app, &thing.subject())
}

fn forget_row(db: &Db, app: &AppName, subject: &str) -> Result<(), String> {
    db.conn
        .execute(
            "DELETE FROM breakers WHERE app = ?1 AND subject = ?2",
            rusqlite::params![app, subject],
        )
        .map(drop)
        .map_err(|e| format!("cannot forget the breaker: {e}"))
}

/// Reset the breaker of every action closure `declared` no longer holds, and
/// clear its faults.
// spec: LIM#breakers
// spec: FLT#app-faults
pub fn reset_undeclared(
    db: &Db,
    app: &AppName,
    declared: &crate::defs::app::AppDef,
) -> Result<(), String> {
    let still = |thing: &Thing| match thing {
        Thing::Definition => true,
        Thing::Action(name) => declared.actions.contains_key(name),
        Thing::Shell(name) => declared.shells.contains_key(name),
        Thing::Gate(name) => declared.gates.contains_key(name),
    };
    for kind in [EVALUATION_RUNAWAY, BREAKER_STATE_UNREADABLE] {
        let active = faults::list_active_faults_in_scope(
            db,
            &faults::FaultScope::AppKind(app.clone(), kind.to_owned()),
        )
        .map_err(|e| format!("cannot read {kind} faults: {e}"))?;
        for record in active {
            if Thing::from_subject(&record.subject).is_some_and(|thing| !still(&thing)) {
                faults::clear_fault(db, &record.id, app)
                    .map_err(|e| format!("cannot clear {kind}: {e}"))?;
                forget_row(db, app, &record.subject)?;
            }
        }
    }
    LAST_IN_TIME
        .lock()
        .retain(|(a, thing), _| a != app || still(thing));
    Ok(())
}

/// Forget every breaker of a deregistered app.
// spec: LIM#breakers
pub fn discard_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    LAST_IN_TIME.lock().retain(|(a, _), _| a != app);
    db.conn
        .execute("DELETE FROM breakers WHERE app = ?1", [app])
        .map(drop)
}

/// The digests of the definition and each input of the last evaluation of
/// each thing that finished within the limit, which a fault's likely cause
/// is judged against. Held in memory alone.
// spec: FLT#app-faults
type InTime = (String, BTreeMap<String, Option<KeyDigest>>);
static LAST_IN_TIME: LazyLock<Mutex<HashMap<(AppName, Thing), InTime>>> =
    LazyLock::new(Mutex::default);

/// Record that `run` of `thing` finished within the limit, and reset its
/// breaker where it was tripped for another run.
// spec: LIM#breakers
pub fn finished_in_time(
    db: &Db,
    cipher: &Cipher,
    app: &AppName,
    thing: &Thing,
    run: &Run,
    standing: Standing,
) {
    LAST_IN_TIME.lock().insert(
        (app.clone(), thing.clone()),
        (run.definition.clone(), run.input_digests(cipher)),
    );
    if standing == Standing::TrippedForOther
        && let Err(e) = reset(db, app, thing)
    {
        tracing::error!(%app, "cannot reset the breaker of {}: {e}", thing.describe());
    }
}

/// The likely cause of a runaway of `run`, judged against the last
/// evaluation of `thing` that finished in time. Names inputs, never their
/// values.
// spec: FLT#app-faults
fn likely_cause(cipher: &Cipher, app: &AppName, thing: &Thing, run: &Run) -> String {
    let held = LAST_IN_TIME.lock();
    let Some((definition, inputs)) = held.get(&(app.clone(), thing.clone())) else {
        return "the likely cause is unknown".to_owned();
    };
    let now = run.input_digests(cipher);
    let differing: Vec<&str> = now
        .keys()
        .chain(inputs.keys())
        .filter(|label| match (now.get(*label), inputs.get(*label)) {
            (Some(Some(a)), Some(Some(b))) => a != b,
            _ => true,
        })
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let same_definition = *definition == run.definition;
    match (same_definition, differing.is_empty()) {
        (true, false) => format!(
            "the likely cause is the inputs, which differ in {}",
            differing.join(", ")
        ),
        (false, true) => "the likely cause is the definition".to_owned(),
        _ => "the likely cause is unknown".to_owned(),
    }
}

/// What `Action.invoke` consults the invoked action's breaker with, for the
/// passes of one operation or shell session on this thread.
struct Invokes {
    app: AppName,
    base: Run,
    db: crate::runtime::db::DbHandle,
    cipher: std::sync::Arc<Cipher>,
}

thread_local! {
    static INVOKES: std::cell::RefCell<Option<Invokes>> = const { std::cell::RefCell::new(None) };
}

/// Lets `Action.invoke` consult breakers while it is held.
#[must_use = "breakers are consulted only while this is held"]
pub struct InvokesGuard(());

impl InvokesGuard {
    /// Consult the breakers of `app`, whose evaluations run with `base`'s
    /// definition and inputs, before any params.
    pub fn enter(
        app: &AppName,
        base: Run,
        db: crate::runtime::db::DbHandle,
        cipher: std::sync::Arc<Cipher>,
    ) -> Self {
        INVOKES.with_borrow_mut(|held| {
            *held = Some(Invokes {
                app: app.clone(),
                base,
                db,
                cipher,
            })
        });
        Self(())
    }
}

impl Drop for InvokesGuard {
    fn drop(&mut self) {
        INVOKES.with_borrow_mut(|held| *held = None);
    }
}

/// Why `Action.invoke` does not run `action` with `params`: its breaker is
/// tripped for them, or cannot be read. `None` to run it, as it is where no
/// operation's breakers are being consulted.
// spec: ACN#action-invocation
pub fn invoke_refused(
    action: &ActionName,
    params: &crate::runtime::param_map::ParamMap,
    schema: &BTreeMap<seedling_protocol::names::ParamName, crate::defs::param_def::ParamDef>,
) -> Option<String> {
    let (app, run, db, cipher) = INVOKES.with_borrow(|held| {
        held.as_ref().map(|i| {
            (
                i.app.clone(),
                i.base.clone().with_params(params, schema),
                i.db.clone(),
                std::sync::Arc::clone(&i.cipher),
            )
        })
    })?;
    let thing = Thing::Action(action.clone());
    let checked = thing.clone();
    match db.call(move |db| standing(db, &cipher, &app, &checked, &run)) {
        Ok(Standing::Tripped) => Some(tripped_message(&thing)),
        Ok(_) => None,
        Err(why) => Some(unreadable_message(&thing, &why)),
    }
}

/// What an evaluation refused by a tripped breaker reports.
pub fn tripped_message(thing: &Thing) -> String {
    format!(
        "{} ran past the evaluation time limit with the same definition and inputs, and is not \
         evaluated again until its {EVALUATION_RUNAWAY} fault clears",
        thing.describe()
    )
}

/// What an evaluation that cannot tell whether its breaker is tripped
/// reports.
pub fn unreadable_message(thing: &Thing, why: &str) -> String {
    format!(
        "cannot read whether the breaker of {} is tripped: {why}",
        thing.describe()
    )
}

#[cfg(test)]
mod tests;
