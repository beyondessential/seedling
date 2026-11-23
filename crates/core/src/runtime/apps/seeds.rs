use rusqlite::OptionalExtension;
use seedling_protocol::names::AppName;

use crate::runtime::{
    db::Db,
    secrets::Cipher,
    seeds::{AppSeed, Seed},
};

fn sealing_failed(e: crate::runtime::secrets::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

// spec: SEC#app-seeds
// spec: SEC#app-seed-rotation
/// Record `seed` as the app's, replacing any seed it had.
pub fn store_app_seed(
    db: &Db,
    cipher: &Cipher,
    app_name: &AppName,
    seed: &Seed,
) -> rusqlite::Result<()> {
    let ciphertext = seed.seal(cipher).map_err(sealing_failed)?;
    db.conn.execute(
        // spec: INFRA#database
        "INSERT INTO app_seeds (app_name, ciphertext) VALUES (?1, ?2) \
         ON CONFLICT(app_name) DO UPDATE SET ciphertext = excluded.ciphertext",
        rusqlite::params![app_name, ciphertext],
    )?;
    Ok(())
}

// spec: SEC#app-seeds
/// The app's seed as the runtime can hold it.
///
/// A registered app without a recorded seed reads as unreadable, never as one
/// to be created: every registered app is given its seed when it registers.
pub fn load_app_seed(db: &Db, cipher: &Cipher, app_name: &AppName) -> rusqlite::Result<AppSeed> {
    let ciphertext: Option<Vec<u8>> = db
        .conn
        .query_row(
            "SELECT ciphertext FROM app_seeds WHERE app_name = ?1",
            [app_name],
            |row| row.get(0),
        )
        .optional()?;
    let seed = match ciphertext {
        Some(ciphertext) => AppSeed::open(cipher, &ciphertext),
        None => AppSeed::Unreadable("the app has no recorded seed".into()),
    };
    if let AppSeed::Unreadable(reason) = &seed {
        tracing::error!(app = %app_name, %reason, "app seed cannot be read");
    }
    Ok(seed)
}

// spec: SEC#app-seeds
pub fn delete_app_seed(db: &Db, app_name: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM app_seeds WHERE app_name = ?1", [app_name])?;
    Ok(())
}

// spec: SEC#app-seeds
/// Give every registered app without a recorded seed a fresh one, once, for
/// the apps registered before seeds existed; does nothing after that.
///
/// A row that exists is never touched, readable or not.
pub fn backfill_app_seeds(db: &Db, cipher: &Cipher) -> rusqlite::Result<()> {
    db.atomically(|db| {
        let owed = db
            .conn
            .query_row("SELECT 1 FROM app_seed_backfill", [], |_| Ok(()))
            .optional()?
            .is_some();
        if !owed {
            return Ok(());
        }
        let unseeded: Vec<AppName> = db
            .conn
            .prepare(
                "SELECT name FROM registered_apps \
                 WHERE name NOT IN (SELECT app_name FROM app_seeds)",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for app_name in &unseeded {
            let seed =
                Seed::draw().map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            store_app_seed(db, cipher, app_name, &seed)?;
        }
        db.conn.execute("DELETE FROM app_seed_backfill", [])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests;
