//! The storage of each anonymous volume created for a pod instance.
//!
//! An anonymous volume goes when the pod instance it was created for stops.
//! A pod an operation created can be stopped after its definition is gone,
//! from its recorded identity alone, so the storage of each of its anonymous
//! volumes is recorded here before it is created, and the record removed
//! once the storage is.

use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::runtime::{
    db::Db,
    identity::{InstanceId, ResourceInstance},
};

/// Where one anonymous volume's storage is: under its name, in the tmpfs
/// location or with the container runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnonymousVolume {
    pub name: String,
    pub tmpfs: bool,
}

/// Record `volumes` as created for `instance`, before their storage is.
pub fn record(
    db: &Db,
    instance: &ResourceInstance,
    volumes: &[AnonymousVolume],
) -> rusqlite::Result<()> {
    db.atomically(|db| {
        for volume in volumes {
            db.conn.execute(
                "INSERT INTO anonymous_volumes (instance_id, app, volume, tmpfs)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (instance_id, volume, tmpfs) DO NOTHING",
                params![
                    instance.id.to_hex(),
                    instance.app,
                    volume.name,
                    volume.tmpfs
                ],
            )?;
        }
        Ok(())
    })
}

/// The anonymous volumes recorded as created for `instance`.
pub fn recorded_for(db: &Db, instance: InstanceId) -> rusqlite::Result<Vec<AnonymousVolume>> {
    let mut stmt = db.conn.prepare(
        "SELECT volume, tmpfs FROM anonymous_volumes
         WHERE instance_id = ?1
         ORDER BY volume, tmpfs",
    )?;
    stmt.query_map(params![instance.to_hex()], |row| {
        Ok(AnonymousVolume {
            name: row.get(0)?,
            tmpfs: row.get(1)?,
        })
    })?
    .collect()
}

/// Whether a pod instance other than `instance` is recorded as having
/// `volume` created for it, so that its storage is still that instance's.
pub fn held_by_another(
    db: &Db,
    instance: InstanceId,
    volume: &AnonymousVolume,
) -> rusqlite::Result<bool> {
    db.conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM anonymous_volumes
             WHERE volume = ?1 AND tmpfs = ?2 AND instance_id != ?3
         )",
        params![volume.name, volume.tmpfs, instance.to_hex()],
        |row| row.get(0),
    )
}

/// Drop the record of `volume` for `instance`, once its storage is gone.
pub fn forget(db: &Db, instance: InstanceId, volume: &AnonymousVolume) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM anonymous_volumes
         WHERE instance_id = ?1 AND volume = ?2 AND tmpfs = ?3",
        params![instance.to_hex(), volume.name, volume.tmpfs],
    )?;
    Ok(())
}

/// Drop every record of `app`'s, with its recorded instances.
pub fn delete_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM anonymous_volumes WHERE app = ?1", params![app])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;

    fn pod(app: &str) -> ResourceInstance {
        ResourceInstance::new_anonymous(AppName::new(app).unwrap(), ResourceKind::Job)
    }

    fn volume(name: &str, tmpfs: bool) -> AnonymousVolume {
        AnonymousVolume {
            name: name.to_owned(),
            tmpfs,
        }
    }

    // spec: ACTU#deployment-and-job-instances
    #[test]
    fn a_volume_is_held_by_each_instance_recorded_with_it_until_forgotten() {
        let db = Db::open_in_memory().unwrap();
        let a = pod("app");
        let b = pod("app");
        let shared = volume("seedling-anon-1-vol-0", false);
        let scratch = volume("seedling-anon-1-vol-1", true);
        record(&db, &a, &[shared.clone(), scratch.clone()]).unwrap();
        record(&db, &a, std::slice::from_ref(&shared)).unwrap();
        record(&db, &b, std::slice::from_ref(&shared)).unwrap();

        assert_eq!(
            recorded_for(&db, a.id).unwrap(),
            vec![shared.clone(), scratch.clone()]
        );
        assert!(held_by_another(&db, a.id, &shared).unwrap());
        assert!(!held_by_another(&db, a.id, &scratch).unwrap());
        assert!(
            !held_by_another(&db, a.id, &volume(&shared.name, true)).unwrap(),
            "the same name at the other storage address is other storage"
        );

        forget(&db, b.id, &shared).unwrap();
        assert!(!held_by_another(&db, a.id, &shared).unwrap());
        assert_eq!(recorded_for(&db, b.id).unwrap(), Vec::new());
    }

    // spec: TRD#deregistration
    #[test]
    fn deleting_an_apps_records_leaves_the_other_apps() {
        let db = Db::open_in_memory().unwrap();
        let ours = pod("app");
        let theirs = pod("app-db");
        let vol = volume("seedling-anon-1-vol-0", false);
        record(&db, &ours, std::slice::from_ref(&vol)).unwrap();
        record(&db, &theirs, std::slice::from_ref(&vol)).unwrap();

        delete_for_app(&db, &AppName::new("app").unwrap()).unwrap();

        assert_eq!(recorded_for(&db, ours.id).unwrap(), Vec::new());
        assert_eq!(recorded_for(&db, theirs.id).unwrap(), vec![vol]);
    }
}
