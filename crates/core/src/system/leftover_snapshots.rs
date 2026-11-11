//! Snapshots left on disk by backup runs of earlier releases.
//!
//! A backup run took a read-only snapshot of its source volume as site-volume
//! storage under a [`SNAPSHOT_NAME_PREFIX`] name and removed it once the run
//! finished. A daemon killed in between left the snapshot behind with no
//! site-volume record, so nothing else would ever list or remove it.

use std::collections::HashMap;

use seedling_protocol::names::SiteVolumeName;

use crate::{
    runtime::{db::DbHandle, site_volumes},
    system::volume_store::VolumeStore,
};

/// Site-volume name prefix backup runs took their snapshots under.
pub const SNAPSHOT_NAME_PREFIX: &str = "backup-snap-";

/// Why a snapshot-prefixed path is not a leftover to remove.
///
/// A definite "the operator owns this" and a "we could not tell" both keep the
/// storage, but they are not the same answer and must not be reported as one.
enum Keep {
    Registered,
    UnparseableName(String),
    OwnershipUnknown(String),
}

/// Remove every snapshot-prefixed site-volume path that no site volume is
/// recorded for.
// spec: SITE#leftover-backup-snapshots
pub async fn sweep(db: &DbHandle, store: &VolumeStore) {
    let candidates = match store.list_sites_with_prefix(SNAPSHOT_NAME_PREFIX) {
        Ok(candidates) => candidates,
        Err(e) => {
            tracing::warn!("failed to scan for leftover backup snapshots: {e}");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }

    // spec: IDN#reserved-names — a site-volume record is what says an
    // operator owns this name. Creation rejects the prefix, but that cannot
    // help a volume created before the check existed.
    let lookup = candidates.clone();
    let keeps: HashMap<String, Keep> = db.call(move |conn| {
        lookup
            .into_iter()
            .filter_map(|name| {
                let reason = match SiteVolumeName::new(&name) {
                    Ok(parsed) => match site_volumes::get(conn, &parsed) {
                        Ok(Some(_)) => Keep::Registered,
                        Ok(None) => return None,
                        // A destructive sweep must be conservative: "we could
                        // not tell" is not "not owned".
                        Err(e) => Keep::OwnershipUnknown(e.to_string()),
                    },
                    // Also a "could not tell": a name Seedling could not have
                    // created is not a snapshot of ours to delete.
                    Err(e) => Keep::UnparseableName(e.to_string()),
                };
                Some((name, reason))
            })
            .collect()
    });

    for name in candidates {
        match keeps.get(&name) {
            None => {}
            Some(Keep::Registered) => {
                tracing::info!(
                    volume = %name,
                    "a registered site volume uses the backup snapshot prefix; leaving it alone"
                );
                continue;
            }
            Some(Keep::UnparseableName(e)) => {
                tracing::warn!(
                    volume = %name,
                    "a snapshot-prefixed path is not a valid site volume name, so it is not a \
                     snapshot of ours; leaving it alone: {e}"
                );
                continue;
            }
            Some(Keep::OwnershipUnknown(e)) => {
                tracing::warn!(
                    volume = %name,
                    "could not determine ownership of a snapshot-prefixed site volume; leaving \
                     it alone: {e}"
                );
                continue;
            }
        }
        tracing::info!(snapshot = %name, "removing leftover backup snapshot");
        if let Err(e) = store.remove_site(&name).await {
            tracing::warn!(snapshot = %name, "failed to remove leftover backup snapshot: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    // spec: SITE#leftover-backup-snapshots
    // spec: IDN#reserved-names
    #[tokio::test]
    async fn removes_only_unrecorded_snapshot_storage() {
        let dir = tempfile::tempdir().unwrap();
        let store = VolumeStore::new(dir.path(), false).unwrap();
        let db = DbHandle::open_in_memory().unwrap();

        for name in [
            "backup-snap-leftover",
            "backup-snap-recorded",
            "backup-snap-trailing-",
            "archive",
        ] {
            store.create_site(name).await.unwrap();
        }
        db.call(|db| {
            site_volumes::create(
                db,
                &SiteVolumeDef {
                    name: SiteVolumeName::new("backup-snap-recorded").unwrap(),
                    kind: SiteVolumeKind::Managed,
                    created_at: "2026-01-01T00:00:00Z".into(),
                },
            )
        })
        .unwrap();

        sweep(&db, &store).await;

        assert!(!store.site_exists("backup-snap-leftover"));
        assert!(store.site_exists("backup-snap-recorded"));
        assert!(store.site_exists("backup-snap-trailing-"));
        assert!(store.site_exists("archive"));
    }
}
