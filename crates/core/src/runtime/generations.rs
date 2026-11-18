use std::{collections::BTreeMap, sync::Arc};

use secrecy::{ExposeSecret, SecretString};
use seedling_protocol::names::{AppName, ParamName};

use crate::{
    defs::app::App,
    runtime::{
        apps,
        db::Db,
        definition::{Bundle, Source},
        secrets::Cipher,
    },
};

/// Whether any generation of `app` was made after `since`.
///
/// Read from the history's own record of when each generation was made, so
/// the answer is the same after a restart as before one.
pub fn changed_since(db: &Db, app: &AppName, since: jiff::Timestamp) -> rusqlite::Result<bool> {
    let mut stmt = db
        .conn
        .prepare("SELECT created_at FROM generations WHERE app = ?1")?;
    let rows = stmt.query_map([app], |row| row.get::<_, String>(0))?;
    for created in rows {
        let created = created?;
        let made: jiff::Timestamp = created.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        if made > since {
            return Ok(true);
        }
    }
    Ok(false)
}

// spec: GEN#generation-numbers
pub type Generation = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Register,
    ScriptUpdate,
    ParamChange,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::ScriptUpdate => "script_update",
            Self::ParamChange => "param_change",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "register" => Some(Self::Register),
            "script_update" => Some(Self::ScriptUpdate),
            "param_change" => Some(Self::ParamChange),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Succeeded,
    Failed,
    Cancelled,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// What a generation did to one parameter, as the history holds it.
#[derive(Debug, Clone)]
// spec: GEN#history-entries
pub struct HistoryParam {
    pub name: String,
    pub previous_value: Option<String>,
    pub new_value: Option<String>,
    /// True when the previous value was stored encrypted (redact in responses).
    pub previous_value_redacted: bool,
    /// True when the new value was stored encrypted (redact in responses).
    pub new_value_redacted: bool,
}

#[derive(Debug, Clone)]
// spec: GEN#history-entries
pub struct HistoryEntry {
    pub generation: Generation,
    pub created_at: String,
    pub kind: Kind,
    /// The parameters the generation changed, in ascending order of name.
    pub params: Vec<HistoryParam>,
    pub bundle_hash: String,
    /// For `Register` and `ScriptUpdate`: how the installed definition
    /// reached the runtime.
    pub provenance: Option<Source>,
    pub operation_id: Option<String>,
    pub outcome: Option<Outcome>,
    pub outcome_error: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    Db(rusqlite::Error),
    Script(apps::ScriptError),
    NoCurrentGeneration(String),
    NotFound {
        app: AppName,
        generation: Generation,
    },
    MissingBundle(String),
    CorruptBundle(String),
    /// A recorded parameter value could not be read back, so the parameter
    /// map at the generation cannot be built.
    UnreadableParam {
        param: String,
        reason: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Script(e) => write!(f, "script error: {e}"),
            Self::NoCurrentGeneration(app) => write!(f, "app {app:?} has no current generation"),
            Self::NotFound { app, generation } => {
                write!(f, "generation {generation} not found for app {app:?}")
            }
            Self::MissingBundle(hash) => write!(f, "definition bundle not found for hash {hash}"),
            Self::CorruptBundle(msg) => write!(f, "stored definition bundle is unreadable: {msg}"),
            Self::UnreadableParam { param, reason } => {
                write!(
                    f,
                    "recorded value of parameter {param:?} is unreadable: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(e) => Some(e),
            Self::Script(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<apps::ScriptError> for Error {
    fn from(e: apps::ScriptError) -> Self {
        Self::Script(e)
    }
}

// spec: GEN#bundle-storage
pub fn store_bundle(db: &Db, bundle: &Bundle) -> rusqlite::Result<()> {
    db.conn.execute(
        "INSERT INTO definition_bundles (hash, contents) VALUES (?1, ?2)
         ON CONFLICT(hash) DO NOTHING",
        rusqlite::params![bundle.hash(), bundle.canonical_bytes()],
    )?;
    Ok(())
}

/// Load a stored bundle by content hash.
// spec: GEN#bundle-storage
pub fn load_bundle(db: &Db, hash: &str) -> Result<Arc<Bundle>, Error> {
    let mut stmt = db
        .conn
        .prepare("SELECT contents FROM definition_bundles WHERE hash = ?1")?;
    let contents: Vec<u8> = match stmt.query_row([hash], |row| row.get(0)) {
        Ok(c) => c,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(Error::MissingBundle(hash.to_owned()));
        }
        Err(e) => return Err(e.into()),
    };
    let bundle =
        Bundle::from_canonical_bytes(&contents).map_err(|e| Error::CorruptBundle(e.to_string()))?;
    // A row whose contents no longer hash to its key would let two different
    // definitions pass for one; refuse it rather than install the wrong one.
    if bundle.hash() != hash {
        return Err(Error::CorruptBundle(format!(
            "bundle stored under {hash} hashes to {}",
            bundle.hash()
        )));
    }
    Ok(Arc::new(bundle))
}

/// The Seedling version requirement a stored bundle declares, read without
/// materialising the bundle or re-hashing it.
// spec: GEN#bundle-storage
pub fn bundle_requirement(db: &Db, hash: &str) -> Result<Option<String>, Error> {
    let mut stmt = db
        .conn
        .prepare("SELECT contents FROM definition_bundles WHERE hash = ?1")?;
    let contents: Vec<u8> = match stmt.query_row([hash], |row| row.get(0)) {
        Ok(c) => c,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(Error::MissingBundle(hash.to_owned()));
        }
        Err(e) => return Err(e.into()),
    };
    Bundle::requirement_of_canonical(&contents)
        .map(|r| r.map(|r| r.as_str().to_owned()))
        .map_err(|e| Error::CorruptBundle(e.to_string()))
}

/// Delete stored bundles no generation or template references.
// spec: GEN#lineage
pub fn gc_bundles(db: &Db) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM definition_bundles
         WHERE hash NOT IN (SELECT DISTINCT bundle_hash FROM generations)
           AND hash NOT IN (SELECT bundle_hash FROM templates WHERE bundle_hash IS NOT NULL)",
        [],
    )?;
    Ok(())
}

pub fn current(db: &Db, app: &AppName) -> rusqlite::Result<Option<Generation>> {
    let mut stmt = db
        .conn
        .prepare("SELECT current_generation FROM registered_apps WHERE name = ?1")?;
    match stmt.query_row([app], |row| row.get::<_, i64>(0)) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n as Generation)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

// spec: GEN#generation-numbers
fn next_generation_for(db: &Db, app: &AppName) -> rusqlite::Result<Generation> {
    let mut stmt = db
        .conn
        .prepare("SELECT COALESCE(MAX(generation), 0) FROM generations WHERE app = ?1")?;
    let n: i64 = stmt.query_row([app], |row| row.get(0))?;
    Ok((n as Generation) + 1)
}

fn now() -> String {
    jiff::Timestamp::now().to_string()
}

/// A parameter change recorded in the generation history.
#[derive(Debug, Clone)]
pub struct ParamChange<'a> {
    pub name: &'a ParamName,
    pub previous: Option<&'a str>,
    /// `None` when the change unsets the parameter.
    pub new_value: Option<&'a str>,
    /// Whether the new value is held in secret storage.
    pub is_secret: bool,
    /// Whether the value being replaced was held in secret storage.
    ///
    /// Separate from `is_secret` because a definition may drop a
    /// parameter's `secret` flag in the same update that changes its value:
    /// what the old value needs is decided by where it was, not by what the
    /// definition now says.
    pub previous_is_secret: bool,
}

fn encrypt(cipher: &Cipher, value: Option<&str>) -> rusqlite::Result<Option<Vec<u8>>> {
    value
        .map(|v| {
            cipher
                .encrypt(&SecretString::new(v.to_owned().into()))
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })
        .transpose()
}

// spec: GEN#generation-numbers
fn insert_register_or_update(
    db: &Db,
    app: &AppName,
    kind: Kind,
    bundle: &Bundle,
    source: &Source,
    changes: Option<(&[ParamChange<'_>], &Cipher)>,
) -> rusqlite::Result<Generation> {
    db.atomically(|db| {
        store_bundle(db, bundle)?;
        let gen_n = next_generation_for(db, app)?;
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, bundle_hash, provenance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                app,
                gen_n as i64,
                now(),
                kind.as_str(),
                bundle.hash(),
                source.to_db()
            ],
        )?;
        if let Some((changes, cipher)) = changes {
            write_param_changes(db, app, gen_n, changes, cipher)?;
        }
        db.conn.execute(
            "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
            rusqlite::params![gen_n as i64, app],
        )?;
        Ok(gen_n)
    })
}

/// Bump the generation for the initial registration of an app.
/// Stores the bundle content-addressed and writes a `Register` history entry.
pub fn bump_register(
    db: &Db,
    app: &AppName,
    bundle: &Bundle,
    source: &Source,
) -> rusqlite::Result<Generation> {
    insert_register_or_update(db, app, Kind::Register, bundle, source, None)
}

/// Bump the generation for a definition update, recording the parameter
/// changes made alongside it, if any. Identical bundle content reuses the
/// existing stored bundle.
pub fn bump_script_update(
    db: &Db,
    app: &AppName,
    bundle: &Bundle,
    source: &Source,
    changes: Option<(&[ParamChange<'_>], &Cipher)>,
) -> rusqlite::Result<Generation> {
    insert_register_or_update(db, app, Kind::ScriptUpdate, bundle, source, changes)
}

/// Register `app` from a lone script, pushed by nobody known.
#[cfg(test)]
pub fn register_script(db: &Db, app: &AppName, script: &str) -> rusqlite::Result<Generation> {
    bump_register(
        db,
        app,
        &Bundle::from_stored_script(script),
        &Source::unknown_push(),
    )
}

/// Replace `app`'s definition with a lone script, pushed by nobody known.
#[cfg(test)]
pub fn update_script(db: &Db, app: &AppName, script: &str) -> rusqlite::Result<Generation> {
    bump_script_update(
        db,
        app,
        &Bundle::from_stored_script(script),
        &Source::unknown_push(),
        None,
    )
}

fn current_bundle_hash(db: &Db, app: &AppName) -> rusqlite::Result<String> {
    let mut stmt = db.conn.prepare(
        "SELECT bundle_hash FROM generations
         WHERE app = ?1
         ORDER BY generation DESC
         LIMIT 1",
    )?;
    stmt.query_row([app], |row| row.get::<_, String>(0))
}

/// Write the parameter changes of a generation, each of the two values of
/// each in the column its own storage calls for.
// spec: SEC#generation-history
fn write_param_changes(
    db: &Db,
    app: &AppName,
    generation: Generation,
    changes: &[ParamChange<'_>],
    cipher: &Cipher,
) -> rusqlite::Result<()> {
    for change in changes {
        let (previous, previous_ct) = if change.previous_is_secret {
            (None, encrypt(cipher, change.previous)?)
        } else {
            (change.previous, None)
        };
        let (new_value, new_ct) = if change.is_secret {
            (None, encrypt(cipher, change.new_value)?)
        } else {
            (change.new_value, None)
        };
        db.conn.execute(
            "INSERT INTO generation_params
                (app, generation, param_name, previous_value,
                 previous_value_ciphertext, new_value, new_value_ciphertext)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                app,
                generation as i64,
                change.name,
                previous,
                previous_ct,
                new_value,
                new_ct
            ],
        )?;
    }
    Ok(())
}

/// Bump the generation for a change to one or more parameters, under the
/// definition the app is already running. A `new_value` of `None` unsets.
// spec: SEC#generation-history
// spec: PRM#parameter-changes
pub fn bump_param_change(
    db: &Db,
    app: &AppName,
    changes: &[ParamChange<'_>],
    cipher: &Cipher,
) -> rusqlite::Result<Generation> {
    // spec: GEN#generation-numbers
    db.atomically(|db| {
        let hash = current_bundle_hash(db, app)?;
        let gen_n = next_generation_for(db, app)?;
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, bundle_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![app, gen_n as i64, now(), Kind::ParamChange.as_str(), hash],
        )?;
        write_param_changes(db, app, gen_n, changes, cipher)?;
        db.conn.execute(
            "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
            rusqlite::params![gen_n as i64, app],
        )?;
        Ok(gen_n)
    })
}

/// Attach a lifecycle operation id to a generation history entry, and mark its
/// outcome as Pending. Called when a generation bump schedules an `on_change`
/// (or other) lifecycle operation.
pub fn attach_operation(
    db: &Db,
    app: &AppName,
    generation: Generation,
    operation_id: &str,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE generations
            SET operation_id = ?1,
                outcome      = 'pending'
          WHERE app = ?2 AND generation = ?3",
        rusqlite::params![operation_id, app, generation as i64],
    )?;
    Ok(())
}

/// Record the final outcome of the lifecycle operation attached to a generation.
pub fn record_outcome(
    db: &Db,
    app: &AppName,
    generation: Generation,
    outcome: Outcome,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE generations
            SET outcome       = ?1,
                outcome_error = ?2
          WHERE app = ?3 AND generation = ?4",
        rusqlite::params![outcome.as_str(), error, app, generation as i64],
    )?;
    Ok(())
}

/// Record how the operation `operation_id` ended against the generation it
/// was attached to, if it was attached to one.
// spec: GEN#generation-numbers
pub fn record_operation_outcome(
    db: &Db,
    operation_id: &str,
    outcome: Outcome,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE generations
            SET outcome       = ?1,
                outcome_error = ?2
          WHERE operation_id = ?3",
        rusqlite::params![outcome.as_str(), error, operation_id],
    )?;
    Ok(())
}

/// List generation history entries for an app, newest first.
/// `before`, when provided, restricts results to `generation < before`.
pub fn list(
    db: &Db,
    app: &AppName,
    before: Option<Generation>,
    limit: usize,
) -> rusqlite::Result<Vec<HistoryEntry>> {
    let limit = limit.min(200) as i64;
    let entries = if let Some(before) = before {
        let mut stmt = db.conn.prepare(
            "SELECT generation, created_at, kind, bundle_hash, operation_id,
                    outcome, outcome_error, provenance
             FROM generations
             WHERE app = ?1 AND generation < ?2
             ORDER BY generation DESC
             LIMIT ?3",
        )?;
        rows_to_entries(
            db,
            app,
            stmt.query(rusqlite::params![app, before as i64, limit])?,
        )?
    } else {
        let mut stmt = db.conn.prepare(
            "SELECT generation, created_at, kind, bundle_hash, operation_id,
                    outcome, outcome_error, provenance
             FROM generations
             WHERE app = ?1
             ORDER BY generation DESC
             LIMIT ?2",
        )?;
        rows_to_entries(db, app, stmt.query(rusqlite::params![app, limit])?)?
    };
    Ok(entries)
}

fn rows_to_entries(
    db: &Db,
    app: &AppName,
    mut rows: rusqlite::Rows<'_>,
) -> rusqlite::Result<Vec<HistoryEntry>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let kind_str: String = row.get(2)?;
        let outcome_str: Option<String> = row.get(5)?;
        let provenance: Option<String> = row.get(7)?;
        let generation = row.get::<_, i64>(0)? as Generation;
        out.push(HistoryEntry {
            generation,
            created_at: row.get(1)?,
            kind: Kind::parse(&kind_str).unwrap_or(Kind::Register),
            params: params_of(db, app, generation)?,
            bundle_hash: row.get(3)?,
            provenance: provenance.as_deref().and_then(|p| Source::from_db(p).ok()),
            operation_id: row.get(4)?,
            outcome: outcome_str.as_deref().and_then(Outcome::parse),
            outcome_error: row.get(6)?,
        });
    }
    Ok(out)
}

/// The parameters a generation changed, in ascending order of name.
fn params_of(
    db: &Db,
    app: &AppName,
    generation: Generation,
) -> rusqlite::Result<Vec<HistoryParam>> {
    let mut stmt = db.conn.prepare(
        "SELECT param_name, previous_value, previous_value_ciphertext,
                new_value, new_value_ciphertext
         FROM generation_params
         WHERE app = ?1 AND generation = ?2
         ORDER BY param_name ASC",
    )?;
    stmt.query_map(rusqlite::params![app, generation as i64], |row| {
        let previous_ct: Option<Vec<u8>> = row.get(2)?;
        let new_ct: Option<Vec<u8>> = row.get(4)?;
        Ok(HistoryParam {
            name: row.get(0)?,
            previous_value: row.get(1)?,
            new_value: row.get(3)?,
            previous_value_redacted: previous_ct.is_some(),
            new_value_redacted: new_ct.is_some(),
        })
    })?
    .collect()
}

/// Look up a single generation entry.
pub fn get(
    db: &Db,
    app: &AppName,
    generation: Generation,
) -> rusqlite::Result<Option<HistoryEntry>> {
    let mut stmt = db.conn.prepare(
        "SELECT generation, created_at, kind, bundle_hash, operation_id,
                outcome, outcome_error, provenance
         FROM generations
         WHERE app = ?1 AND generation = ?2",
    )?;
    let mut entries = rows_to_entries(
        db,
        app,
        stmt.query(rusqlite::params![app, generation as i64])?,
    )?;
    Ok(entries.pop())
}

/// Build the parameter map at a specific generation by walking history.
/// For each parameter, the most recent change at or before `generation` is
/// taken, whether in a `param_change` or in a `script_update` that changed it
/// alongside its definition; an unset (or no change) yields None.
// spec: SEC#generation-history
// spec: GEN#reconstruction
pub fn param_map_at(
    db: &Db,
    app: &AppName,
    generation: Generation,
    cipher: &Cipher,
) -> Result<BTreeMap<String, String>, Error> {
    let mut stmt = db.conn.prepare(
        "SELECT param_name, new_value, new_value_ciphertext
         FROM generation_params
         WHERE app = ?1 AND generation <= ?2
         ORDER BY param_name ASC, generation DESC",
    )?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut last_param: Option<String> = None;
    let mut rows = stmt.query(rusqlite::params![app, generation as i64])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if last_param.as_deref() == Some(name.as_str()) {
            // Already took the most recent entry for this param.
            continue;
        }
        last_param = Some(name.clone());
        let plaintext: Option<String> = row.get(1)?;
        let ciphertext: Option<Vec<u8>> = row.get(2)?;
        // A change unset its parameter when it recorded no new value in
        // either column.
        let value = if let Some(ct) = ciphertext {
            match cipher.decrypt(&ct) {
                Ok(s) => Some(s.expose_secret().to_owned()),
                Err(e) => {
                    tracing::error!(app = %app, param = %name, "failed to decrypt param history for reconstruction: {e}");
                    return Err(Error::UnreadableParam {
                        param: name,
                        reason: e.to_string(),
                    });
                }
            }
        } else {
            plaintext
        };
        if let Some(v) = value {
            map.insert(name, v);
        }
    }
    Ok(map)
}

/// Look up the definition active at a specific generation: the bundle hash
/// and provenance of the most recent Register/ScriptUpdate at or before it.
// spec: GEN#reconstruction
pub fn definition_at(
    db: &Db,
    app: &AppName,
    generation: Generation,
) -> Result<(String, Source), Error> {
    let mut stmt = db.conn.prepare(
        "SELECT bundle_hash, provenance
         FROM generations
         WHERE app = ?1 AND generation <= ?2
           AND kind IN ('register', 'script_update')
         ORDER BY generation DESC
         LIMIT 1",
    )?;
    match stmt.query_row(rusqlite::params![app, generation as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    }) {
        Ok((hash, provenance)) => {
            let source = match provenance {
                Some(text) => Source::from_db(&text)
                    .map_err(|e| Error::CorruptBundle(format!("unreadable provenance: {e}")))?,
                None => Source::unknown_push(),
            };
            Ok((hash, source))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(Error::NotFound {
            app: app.clone(),
            generation,
        }),
        Err(e) => Err(e.into()),
    }
}

/// The bundle hash a generation's history row carries: the definition that
/// was current when it was written.
pub fn bundle_hash_at(db: &Db, app: &AppName, generation: Generation) -> Result<String, Error> {
    definition_at(db, app, generation).map(|(hash, _)| hash)
}

/// Reconstruct the AppDef as it was at a specific generation by loading the
/// bundle active at that generation and evaluating it with the parameter
/// map at that generation.
// spec: GEN#reconstruction
pub fn reconstruct_app_def(
    db: &Db,
    app: &AppName,
    generation: Generation,
    limits: &crate::ScriptLimits,
    cipher: &Cipher,
) -> Result<App, Error> {
    if get(db, app, generation)?.is_none() {
        return Err(Error::NotFound {
            app: app.clone(),
            generation,
        });
    }
    let (hash, _) = definition_at(db, app, generation)?;
    let bundle = load_bundle(db, &hash)?;
    let params = param_map_at(db, app, generation, cipher)?;
    let (evaled, script_error) = apps::evaluate(app, &bundle, &params, limits);
    if let Some(e) = script_error {
        return Err(Error::Script(e));
    }
    Ok(evaled)
}

/// Delete all generation history and orphaned bundles for an app.
/// Called as part of deregistration.
// spec: GEN#lineage
pub fn delete_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM generations WHERE app = ?1", [app])?;
    gc_bundles(db)
}

#[cfg(test)]
mod tests;
