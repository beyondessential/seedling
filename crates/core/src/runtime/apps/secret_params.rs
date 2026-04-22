use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use secrecy::{ExposeSecret, SecretString};
use seedling_protocol::names::{AppName, ParamName};

use crate::runtime::{db::Db, secrets::Cipher};

// r[impl secret.storage]
pub fn load_secret_params_for_app(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
) -> rusqlite::Result<BTreeMap<String, String>> {
    let mut stmt = db.conn.prepare(
        "SELECT param_name, ciphertext FROM secret_params WHERE app_name = ?1 ORDER BY param_name",
    )?;
    let rows: Vec<(String, Vec<u8>)> = stmt
        .query_map([app_name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut map = BTreeMap::new();
    for (name, ct) in rows {
        match cipher.decrypt(&ct) {
            Ok(s) => {
                map.insert(name, s.expose_secret().to_owned());
            }
            Err(e) => {
                tracing::error!(app = %app_name, param = %name, "failed to decrypt secret param: {e}");
            }
        }
    }
    Ok(map)
}

// r[impl secret.storage]
pub fn upsert_secret_param(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
    param_name: &ParamName,
    value: &SecretString,
) -> rusqlite::Result<()> {
    let ct = cipher
        .encrypt(value)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    db.conn.execute(
        "INSERT OR REPLACE INTO secret_params (app_name, param_name, ciphertext) VALUES (?1, ?2, ?3)",
        rusqlite::params![app_name, param_name, ct],
    )?;
    Ok(())
}

pub fn delete_one_secret_param(
    db: &Db,
    app_name: &AppName,
    param_name: &ParamName,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM secret_params WHERE app_name = ?1 AND param_name = ?2",
        rusqlite::params![app_name, param_name],
    )?;
    Ok(())
}

pub fn delete_app_secret_params(db: &Db, app_name: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM secret_params WHERE app_name = ?1", [app_name])?;
    Ok(())
}

// r[impl secret.migration]
pub fn migrate_to_secret(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
    param_name: &ParamName,
) -> rusqlite::Result<()> {
    let plaintext: Option<String> = db
        .conn
        .query_row(
            "SELECT value FROM params WHERE app_name = ?1 AND param_name = ?2",
            rusqlite::params![app_name, param_name],
            |row| row.get::<_, String>(0),
        )
        .optional()?;

    if let Some(v) = plaintext {
        let secret = SecretString::new(v.into());
        upsert_secret_param(db, cipher, app_name, param_name, &secret)?;
        db.conn.execute(
            "DELETE FROM params WHERE app_name = ?1 AND param_name = ?2",
            rusqlite::params![app_name, param_name],
        )?;
    }
    Ok(())
}
