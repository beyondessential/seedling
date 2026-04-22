use std::{collections::BTreeMap, fmt::Write as FmtWrite};

use secrecy::{ExposeSecret, SecretString};
use seedling_protocol::names::{AppName, ParamName};
use sha2::{Digest, Sha256};

use crate::{
    defs::app::App,
    runtime::{apps, db::Db, secrets::Cipher},
};

// r[impl generation.definition]
pub type Generation = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Register,
    ScriptUpdate,
    ParamSet,
    ParamUnset,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::ScriptUpdate => "script_update",
            Self::ParamSet => "param_set",
            Self::ParamUnset => "param_unset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "register" => Some(Self::Register),
            "script_update" => Some(Self::ScriptUpdate),
            "param_set" => Some(Self::ParamSet),
            "param_unset" => Some(Self::ParamUnset),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Succeeded,
    Failed,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
// r[impl generation.history]
pub struct HistoryEntry {
    pub generation: Generation,
    pub created_at: String,
    pub kind: Kind,
    pub param_name: Option<String>,
    pub previous_value: Option<String>,
    pub new_value: Option<String>,
    /// True when the previous value was stored encrypted (redact in responses).
    pub previous_value_redacted: bool,
    /// True when the new value was stored encrypted (redact in responses).
    pub new_value_redacted: bool,
    pub script_hash: String,
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
    MissingScript(String),
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
            Self::MissingScript(hash) => write!(f, "script body not found for hash {hash}"),
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

fn hex_of(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        write!(s, "{b:02x}").expect("write to String is infallible");
    }
    s
}

fn hash_script(script: &str) -> String {
    hex_of(&Sha256::digest(script.as_bytes()))
}

// r[impl generation.script-storage]
fn store_script(db: &Db, script: &str) -> rusqlite::Result<String> {
    let hash = hash_script(script);
    db.conn.execute(
        "INSERT OR IGNORE INTO script_bodies (hash, body) VALUES (?1, ?2)",
        rusqlite::params![hash, script],
    )?;
    Ok(hash)
}

// r[impl generation.script-storage]
pub fn script_body(db: &Db, hash: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT body FROM script_bodies WHERE hash = ?1")?;
    match stmt.query_row([hash], |row| row.get::<_, String>(0)) {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
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

// r[impl generation.monotonic]
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

// r[impl generation.bumps]
fn insert_register_or_update(
    db: &Db,
    app: &AppName,
    kind: Kind,
    script_hash: &str,
) -> rusqlite::Result<Generation> {
    let gen_n = next_generation_for(db, app)?;
    db.conn.execute(
        "INSERT INTO generations
            (app, generation, created_at, kind, script_hash)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![app, gen_n as i64, now(), kind.as_str(), script_hash],
    )?;
    db.conn.execute(
        "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
        rusqlite::params![gen_n as i64, app],
    )?;
    Ok(gen_n)
}

/// Bump the generation for the initial registration of an app.
/// Stores the script body content-addressed and writes a `Register` history entry.
pub fn bump_register(db: &Db, app: &AppName, script: &str) -> rusqlite::Result<Generation> {
    let hash = store_script(db, script)?;
    insert_register_or_update(db, app, Kind::Register, &hash)
}

/// Bump the generation for a script update. The new script is stored and a
/// `ScriptUpdate` entry is written. Idempotent storage: identical script content
/// reuses the existing `script_bodies` row.
pub fn bump_script_update(db: &Db, app: &AppName, script: &str) -> rusqlite::Result<Generation> {
    let hash = store_script(db, script)?;
    insert_register_or_update(db, app, Kind::ScriptUpdate, &hash)
}

fn current_script_hash(db: &Db, app: &AppName) -> rusqlite::Result<String> {
    let mut stmt = db.conn.prepare(
        "SELECT script_hash FROM generations
         WHERE app = ?1
         ORDER BY generation DESC
         LIMIT 1",
    )?;
    stmt.query_row([app], |row| row.get::<_, String>(0))
}

/// Bump the generation for a parameter set (transitioning to `Some(new)`).
/// The previous value (`None` for `None → Some`) is recorded for history.
// r[impl secret.history]
pub fn bump_param_set(
    db: &Db,
    app: &AppName,
    name: &ParamName,
    previous: Option<&str>,
    new_value: &str,
    cipher: &Cipher,
    is_secret: bool,
) -> rusqlite::Result<Generation> {
    let hash = current_script_hash(db, app)?;
    let gen_n = next_generation_for(db, app)?;
    if is_secret {
        let prev_ct = previous
            .map(|p| {
                let s = SecretString::new(p.to_owned().into());
                cipher
                    .encrypt(&s)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })
            .transpose()?;
        let new_ct = {
            let s = SecretString::new(new_value.to_owned().into());
            cipher
                .encrypt(&s)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
        };
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, param_name,
                 previous_value_ciphertext, new_value_ciphertext, script_hash)
             VALUES (?1, ?2, ?3, 'param_set', ?4, ?5, ?6, ?7)",
            rusqlite::params![app, gen_n as i64, now(), name, prev_ct, new_ct, hash],
        )?;
    } else {
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, param_name,
                 previous_value, new_value, script_hash)
             VALUES (?1, ?2, ?3, 'param_set', ?4, ?5, ?6, ?7)",
            rusqlite::params![app, gen_n as i64, now(), name, previous, new_value, hash],
        )?;
    }
    db.conn.execute(
        "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
        rusqlite::params![gen_n as i64, app],
    )?;
    Ok(gen_n)
}

/// Bump the generation for a parameter unset (transitioning to `None`).
// r[impl secret.history]
pub fn bump_param_unset(
    db: &Db,
    app: &AppName,
    name: &ParamName,
    previous: &str,
    cipher: &Cipher,
    is_secret: bool,
) -> rusqlite::Result<Generation> {
    let hash = current_script_hash(db, app)?;
    let gen_n = next_generation_for(db, app)?;
    if is_secret {
        let prev_ct = {
            let s = SecretString::new(previous.to_owned().into());
            cipher
                .encrypt(&s)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
        };
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, param_name,
                 previous_value_ciphertext, script_hash)
             VALUES (?1, ?2, ?3, 'param_unset', ?4, ?5, ?6)",
            rusqlite::params![app, gen_n as i64, now(), name, prev_ct, hash],
        )?;
    } else {
        db.conn.execute(
            "INSERT INTO generations
                (app, generation, created_at, kind, param_name,
                 previous_value, new_value, script_hash)
             VALUES (?1, ?2, ?3, 'param_unset', ?4, ?5, NULL, ?6)",
            rusqlite::params![app, gen_n as i64, now(), name, previous, hash],
        )?;
    }
    db.conn.execute(
        "UPDATE registered_apps SET current_generation = ?1 WHERE name = ?2",
        rusqlite::params![gen_n as i64, app],
    )?;
    Ok(gen_n)
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
            "SELECT generation, created_at, kind, param_name, previous_value,
                    new_value, script_hash, operation_id, outcome, outcome_error,
                    previous_value_ciphertext, new_value_ciphertext
             FROM generations
             WHERE app = ?1 AND generation < ?2
             ORDER BY generation DESC
             LIMIT ?3",
        )?;
        rows_to_entries(stmt.query(rusqlite::params![app, before as i64, limit])?)?
    } else {
        let mut stmt = db.conn.prepare(
            "SELECT generation, created_at, kind, param_name, previous_value,
                    new_value, script_hash, operation_id, outcome, outcome_error,
                    previous_value_ciphertext, new_value_ciphertext
             FROM generations
             WHERE app = ?1
             ORDER BY generation DESC
             LIMIT ?2",
        )?;
        rows_to_entries(stmt.query(rusqlite::params![app, limit])?)?
    };
    Ok(entries)
}

fn rows_to_entries(mut rows: rusqlite::Rows<'_>) -> rusqlite::Result<Vec<HistoryEntry>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let kind_str: String = row.get(2)?;
        let outcome_str: Option<String> = row.get(8)?;
        let prev_ct: Option<Vec<u8>> = row.get(10)?;
        let new_ct: Option<Vec<u8>> = row.get(11)?;
        out.push(HistoryEntry {
            generation: row.get::<_, i64>(0)? as Generation,
            created_at: row.get(1)?,
            kind: Kind::parse(&kind_str).unwrap_or(Kind::Register),
            param_name: row.get(3)?,
            previous_value: row.get(4)?,
            new_value: row.get(5)?,
            previous_value_redacted: prev_ct.is_some(),
            new_value_redacted: new_ct.is_some(),
            script_hash: row.get(6)?,
            operation_id: row.get(7)?,
            outcome: outcome_str.as_deref().and_then(Outcome::parse),
            outcome_error: row.get(9)?,
        });
    }
    Ok(out)
}

/// Look up a single generation entry.
pub fn get(
    db: &Db,
    app: &AppName,
    generation: Generation,
) -> rusqlite::Result<Option<HistoryEntry>> {
    let mut stmt = db.conn.prepare(
        "SELECT generation, created_at, kind, param_name, previous_value,
                new_value, script_hash, operation_id, outcome, outcome_error,
                previous_value_ciphertext, new_value_ciphertext
         FROM generations
         WHERE app = ?1 AND generation = ?2",
    )?;
    let mut entries = rows_to_entries(stmt.query(rusqlite::params![app, generation as i64])?)?;
    Ok(entries.pop())
}

/// Build the parameter map at a specific generation by walking history.
/// For each parameter, the most recent ParamSet/ParamUnset entry at or before
/// `generation` is taken; ParamUnset (or no entry) yields None.
// r[impl secret.history]
pub fn param_map_at(
    db: &Db,
    app: &AppName,
    generation: Generation,
    cipher: &Cipher,
) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut stmt = db.conn.prepare(
        "SELECT param_name, kind, new_value, new_value_ciphertext
         FROM generations
         WHERE app = ?1
           AND generation <= ?2
           AND kind IN ('param_set', 'param_unset')
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
        let kind: String = row.get(1)?;
        if kind == "param_set" {
            let plaintext: Option<String> = row.get(2)?;
            let ciphertext: Option<Vec<u8>> = row.get(3)?;
            let value = if let Some(ct) = ciphertext {
                match cipher.decrypt(&ct) {
                    Ok(s) => Some(s.expose_secret().to_owned()),
                    Err(e) => {
                        tracing::error!(app = %app, param = %name, "failed to decrypt param history for reconstruction: {e}");
                        None
                    }
                }
            } else {
                plaintext
            };
            if let Some(v) = value {
                map.insert(name, v);
            }
        }
    }
    Ok(map)
}

/// Look up the script hash active at a specific generation: the script_hash
/// of the most recent Register/ScriptUpdate at or before that generation.
// r[impl generation.previous]
pub fn script_hash_at(db: &Db, app: &AppName, generation: Generation) -> Result<String, Error> {
    let mut stmt = db.conn.prepare(
        "SELECT script_hash
         FROM generations
         WHERE app = ?1 AND generation <= ?2
         ORDER BY generation DESC
         LIMIT 1",
    )?;
    match stmt.query_row(rusqlite::params![app, generation as i64], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(h) => Ok(h),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(Error::NotFound {
            app: app.clone(),
            generation,
        }),
        Err(e) => Err(e.into()),
    }
}

/// Reconstruct the AppDef as it was at a specific generation by loading the
/// script body active at that generation and evaluating it with the parameter
/// map at that generation.
// r[impl generation.reconstruction]
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
    let hash = script_hash_at(db, app, generation)?;
    let script = script_body(db, &hash)?.ok_or_else(|| Error::MissingScript(hash.clone()))?;
    let params = param_map_at(db, app, generation, cipher)?;
    let (evaled, script_error) = apps::evaluate_script(app, &script, &params, limits);
    if let Some(e) = script_error {
        return Err(Error::Script(e));
    }
    Ok(evaled)
}

/// Delete all generation history and orphaned script bodies for an app.
/// Called as part of deregistration.
// r[impl generation.deregister]
pub fn delete_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM generations WHERE app = ?1", [app])?;
    // Garbage-collect any script bodies no longer referenced by any generation.
    db.conn.execute(
        "DELETE FROM script_bodies
         WHERE hash NOT IN (SELECT DISTINCT script_hash FROM generations)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
