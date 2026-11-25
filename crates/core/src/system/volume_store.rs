use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use seedling_protocol::names::HeldVolumeId;
use serde::{Deserialize, Serialize};
use snafu::Snafu;

use crate::{runtime::identity::VolumeName, system::BoxFuture};

/// Error returned by confined write operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ConfinedWriteError {
    #[snafu(display("path escapes confined root: {path:?}"))]
    Escape {
        path: PathBuf,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("confined write to {path:?}: {source}"))]
    Io {
        path: PathBuf,
        source: std::io::Error,
        backtrace: snafu::Backtrace,
    },
}

/// How the host stores volumes: as snapshottable volumes when the data
/// directory is on the host's snapshotting volume store, and as plain
/// directories otherwise.
// spec: ACTU#volume-storage
pub trait VolumeStorage: Send + Sync + 'static {
    /// Whether the data directory is on the snapshotting volume store.
    fn snapshotting(&self) -> bool;

    /// Create a volume's storage at `path`: a snapshottable volume when the
    /// data directory is on the snapshotting volume store, and a directory
    /// otherwise. Fails when anything already stands at `path`.
    fn create<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, std::io::Result<()>>;

    /// Remove the storage at `path`, whichever way it was created.
    fn remove<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, std::io::Result<()>>;

    /// Whether the storage at `path` is a snapshottable volume.
    fn is_snapshottable<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, bool>;

    /// Snapshot the snapshottable volume at `source` to `dest`, read-only or
    /// writable.
    fn snapshot<'a>(
        &'a self,
        source: &'a Path,
        dest: &'a Path,
        writable: bool,
    ) -> BoxFuture<'a, std::io::Result<()>>;

    /// Write `contents` to `rel_path` beneath `root`, creating parent
    /// directories as needed, and refusing a path that would leave `root`
    /// through any of its components.
    // spec: ACTU#storage-confinement
    fn write_confined<'a>(
        &'a self,
        root: &'a Path,
        rel_path: &'a str,
        contents: &'a [u8],
    ) -> BoxFuture<'a, Result<(), ConfinedWriteError>>;
}

/// Whether `path` is a directory the daemon may act on as its own
/// storage, `false` when nothing is there at all.
///
/// A path the daemon *is* the owner of, such as a volume's own directory as
/// against a path inside one, is not resolved at all. A symlink is refused
/// rather than followed: whatever it points at is somebody else's, so
/// creating, moving or deleting through it would act outside the root that
/// owns the name. So is any other kind of file, which is not storage the
/// daemon wrote.
// spec: ACTU#storage-confinement
pub fn owned_dir(path: &Path) -> std::io::Result<bool> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        return Ok(true);
    }
    let kind = if meta.is_symlink() {
        "a symlink"
    } else {
        "not a directory"
    };
    Err(std::io::Error::other(format!(
        "{} is {kind}, so it is not storage seedling owns; \
         resolve it by hand before asking seedling to act on it",
        path.display()
    )))
}

/// Async wrapper around [`owned_dir`].
pub async fn owned_dir_async(path: &Path) -> std::io::Result<bool> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || owned_dir(&path))
        .await
        .map_err(std::io::Error::other)?
}

/// Manages host-backed storage for named volumes.
///
/// Named non-tmpfs volumes are stored under `{data_dir}/volumes/`, as the
/// host's [`VolumeStorage`] makes them. They are bind-mounted into containers
/// rather than using volumes the container runtime manages.
pub struct VolumeStore {
    volumes_dir: PathBuf,
    storage: Arc<dyn VolumeStorage>,
    /// Held while a site volume's name is found free and then taken.
    // spec: SITE#site-volume-names
    site_name_claim: parking_lot::Mutex<()>,
}

impl VolumeStore {
    pub fn new(data_dir: &Path, storage: Arc<dyn VolumeStorage>) -> std::io::Result<Self> {
        let volumes_dir = data_dir.join("volumes");
        std::fs::create_dir_all(&volumes_dir)?;
        Ok(Self {
            volumes_dir,
            storage,
            site_name_claim: parking_lot::Mutex::new(()),
        })
    }

    /// Claim the right to decide whether a site volume name is free.
    ///
    /// Taking a site volume name is check-then-act: the name is found free,
    /// then storage is made and a record written. Operator commands are
    /// served concurrently, so the guard must be held across the whole of
    /// that, not around the look alone. One lock covers every name: these
    /// are rare operator commands, and a per-name map would have to be
    /// reaped to avoid growing with every name ever used.
    // spec: SITE#site-volume-names
    pub fn claim_site_name(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.site_name_claim.lock()
    }

    pub fn volumes_dir(&self) -> &Path {
        &self.volumes_dir
    }

    pub fn path(&self, name: &VolumeName) -> PathBuf {
        self.volumes_dir.join(name.as_str())
    }

    pub fn exists(&self, name: &VolumeName) -> bool {
        self.path(name).exists()
    }

    /// Create a named volume's storage, reporting whether this call is the
    /// one that created it.
    ///
    /// `Ok(false)` means the storage was already there — either standing from
    /// an earlier tick, or created by a concurrent caller between this one's
    /// look and its own create. The two are the same answer to the caller:
    /// somebody else owns seeding it.
    // spec: ACTU#volume-storage
    // spec: ACTU#volume-creation-and-seeding
    pub async fn create(&self, name: &VolumeName) -> std::io::Result<bool> {
        let path = self.path(name);
        // spec: ACTU#storage-confinement
        if owned_dir_async(&path).await? {
            return Ok(false);
        }
        // Creating refuses a name already in use, and need give no
        // machine-readable way to say so. Ask the filesystem afterwards
        // instead: storage standing at the name means the race was lost, not
        // that the create was wrong.
        match self.storage.create(&path).await {
            Ok(()) => Ok(true),
            Err(e) => match owned_dir_async(&path).await {
                Ok(true) => Ok(false),
                _ => Err(e),
            },
        }
    }

    /// Rename a named app volume's on-disk path from `legacy` to `canonical`.
    ///
    /// Earlier builds created the persistent bind-mount directory at
    /// `<app>-<name>` while the Volume actuator and registry recorded the
    /// canonical display name as `<app>-volume-<name>`. Unifying the two
    /// requires rehoming data that containers wrote to the legacy path.
    ///
    /// Returns `Ok(true)` if a rename happened, `Ok(false)` if there was
    /// nothing to migrate. If a directory already stands at the canonical
    /// path it is removed (when empty) or held (when not) before the legacy
    /// path is renamed in place, so nothing is silently dropped. Anything
    /// else at that path is storage the runtime does not own, and the
    /// migration is refused with an error naming it rather than acting on
    /// it; both paths are left as they are for an operator to resolve.
    pub async fn migrate_legacy(
        &self,
        legacy: &str,
        canonical: &VolumeName,
        app: &str,
    ) -> std::io::Result<bool> {
        let legacy_path = self.volumes_dir.join(legacy);
        // spec: ACTU#storage-confinement — the migration moves the storage, so
        // a symlink wearing the legacy name is refused rather than rehomed.
        if !owned_dir_async(&legacy_path).await? {
            return Ok(false);
        }
        let canonical_path = self.path(canonical);
        if legacy_path == canonical_path {
            return Ok(false);
        }

        // spec: ACTU#storage-confinement — classify the canonical path before
        // deciding what to do with it. A regular file or a symlink wearing
        // the name is refused with an error naming it, rather than reaching
        // a directory read that fails with a bare NotADirectory and aborts
        // the migration with nothing said about what an operator should do.
        if owned_dir_async(&canonical_path).await? {
            let mut rd = tokio::fs::read_dir(&canonical_path).await?;
            if rd.next_entry().await?.is_none() {
                self.remove(canonical).await?;
            } else {
                self.hold_inner(
                    &canonical_path,
                    canonical.as_str(),
                    canonical.as_str(),
                    app,
                    "canonical volume path pre-existed during legacy-path migration",
                )
                .await?;
            }
        }

        tokio::fs::rename(&legacy_path, &canonical_path).await?;
        tracing::info!(
            from = %legacy,
            to = %canonical,
            "migrated legacy app volume path"
        );
        Ok(true)
    }

    pub async fn remove(&self, name: &VolumeName) -> std::io::Result<()> {
        let path = self.path(name);
        // spec: ACTU#storage-confinement — a symlink here is refused rather
        // than unlinked, so a volume whose storage was relocated by hand
        // is never reported as deleted while its data stays where it is.
        if !owned_dir_async(&path).await? {
            return Ok(());
        }
        self.storage.remove(&path).await
    }

    fn held_dir(&self) -> PathBuf {
        self.volumes_dir
            .parent()
            .expect("volumes_dir has a parent")
            .join("held-volumes")
    }

    /// Hold a named app volume for operator review.
    ///
    /// `Ok(None)` means there was no storage to hold, which is not a failure:
    /// nothing is preserved because there is nothing to preserve. A hold that
    /// could not be carried out is an error, and the two must stay distinct —
    /// callers hold in order to remove something, and removing on the strength
    /// of a hold that did not happen orphans the data.
    // spec: ACTU#held-volumes
    pub async fn hold(
        &self,
        name: &VolumeName,
        app: &str,
        reason: &str,
    ) -> std::io::Result<Option<HeldVolumeMeta>> {
        let s = name.as_str();
        self.hold_inner(&self.path(name), s, s, app, reason).await
    }

    /// Hold a managed or snapshot site volume for operator review.
    ///
    /// The on-disk path `site-{name}` is moved into the held volumes
    /// directory; the resulting record carries the literal app name `_site`
    /// so UIs can distinguish site-origin holds from app-origin ones.
    ///
    /// `Ok(None)` means there was no storage to hold; see [`Self::hold`].
    // spec: ACTU#held-volumes
    pub async fn hold_site(
        &self,
        name: &str,
        reason: &str,
    ) -> std::io::Result<Option<HeldVolumeMeta>> {
        let src = self.site_path(name);
        let display = format!("site-{name}");
        self.hold_inner(&src, &display, name, "_site", reason).await
    }

    async fn hold_inner(
        &self,
        src: &Path,
        display_name: &str,
        volume_name: &str,
        app: &str,
        reason: &str,
    ) -> std::io::Result<Option<HeldVolumeMeta>> {
        // spec: ACTU#storage-confinement — holding moves the storage aside,
        // and moving a symlink would record a held volume with none of the
        // data behind it. That is an error and stops the caller; storage
        // that is simply not there is not, and lets the caller carry on.
        // spec: ACTU#held-volumes
        if !owned_dir_async(src).await? {
            tracing::info!(
                app = app,
                volume = volume_name,
                "nothing to hold: the volume has no storage behind it"
            );
            return Ok(None);
        }

        let held_dir = self.held_dir();
        tokio::fs::create_dir_all(&held_dir).await?;

        let id = HeldVolumeId::generate();
        let dest = held_dir.join(id.to_string());

        let meta = HeldVolumeMeta {
            id,
            app: app.to_owned(),
            volume_name: volume_name.to_owned(),
            display_name: display_name.to_owned(),
            reason: reason.to_owned(),
            held_at: jiff::Timestamp::now().to_string(),
            path: dest.clone(),
        };

        // spec: ACTU#held-volumes
        // The record is committed before the data moves, so data is never
        // moved aside with nothing to say where it went; if the move then
        // fails, the record goes again.
        let meta_path = held_dir.join(format!("{id}.meta.json"));
        let json = serde_json::to_string_pretty(&meta).map_err(std::io::Error::other)?;
        tokio::fs::write(&meta_path, json).await?;
        if let Err(e) = tokio::fs::rename(&src, &dest).await {
            let _ = tokio::fs::remove_file(&meta_path).await;
            return Err(e);
        }

        tracing::info!(
            app = app,
            volume = volume_name,
            held_id = %id,
            reason = reason,
            "volume held for operator review"
        );

        Ok(Some(meta))
    }

    /// Host path for a held volume's data directory.
    ///
    /// Returns `None` if no held volume with the given id is registered
    /// (either the id is bogus or the volume's meta.json has been
    /// hand-removed). The caller should use this as the definitive source
    /// of truth rather than constructing the path themselves.
    pub fn held_path(&self, id: &HeldVolumeId) -> Option<PathBuf> {
        let held_dir = self.held_dir();
        let data_path = held_dir.join(id.to_string());
        let meta_path = held_dir.join(format!("{id}.meta.json"));
        if !meta_path.exists() || !data_path.exists() {
            return None;
        }
        Some(data_path)
    }

    /// Every held volume, ordered by app, volume name and then id.
    ///
    /// A record that cannot be read or parsed is an error: leaving it out
    /// would show the operator a host with less held on it than there is.
    // spec: VOLM#listing-held-volumes
    pub fn list_held(&self) -> std::io::Result<Vec<HeldVolumeMeta>> {
        let held_dir = self.held_dir();
        if !held_dir.exists() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for entry in std::fs::read_dir(&held_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                results.push(read_held_meta(&path)?);
            }
        }
        results.sort_by(|a, b| {
            (&a.app, &a.volume_name, a.id.to_string()).cmp(&(
                &b.app,
                &b.volume_name,
                b.id.to_string(),
            ))
        });
        Ok(results)
    }

    /// The held volume with this id, or `None` when no record exists for it.
    ///
    /// A record that exists and cannot be read is an error, not `None`.
    // spec: VOLM#deleting-a-held-volume
    pub fn held_meta(&self, id: &HeldVolumeId) -> std::io::Result<Option<HeldVolumeMeta>> {
        let meta_path = self.held_dir().join(format!("{id}.meta.json"));
        match std::fs::symlink_metadata(&meta_path) {
            Ok(_) => read_held_meta(&meta_path).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Move a held volume's data into a fresh managed site volume named
    /// `target_name`. Errors if the held id is unknown, if the held data
    /// directory is missing, or if a site volume with the target name is
    /// already present on disk. The caller is responsible for inserting the
    /// `site_volumes` row and emitting the corresponding events.
    // spec: ACTU#held-volumes
    pub async fn restore_held_to_site(
        &self,
        id: &HeldVolumeId,
        target_name: &str,
    ) -> std::io::Result<HeldVolumeMeta> {
        let held_dir = self.held_dir();
        let data_path = held_dir.join(id.to_string());
        let meta_path = held_dir.join(format!("{id}.meta.json"));

        if !meta_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no held volume with id {id}"),
            ));
        }
        // spec: ACTU#storage-confinement
        if !owned_dir_async(&data_path).await? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("held volume {id} has no backing data directory"),
            ));
        }

        let dest = self.site_path(target_name);
        // spec: ACTU#storage-confinement
        if owned_dir_async(&dest).await? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("site volume {target_name:?} already exists on disk"),
            ));
        }

        let meta_json = tokio::fs::read_to_string(&meta_path).await?;
        let meta: HeldVolumeMeta =
            serde_json::from_str(&meta_json).map_err(std::io::Error::other)?;

        tokio::fs::rename(&data_path, &dest).await?;
        // spec: VOLM#requests
        // A restore that fails leaves the volume held, so the data goes back
        // where it was rather than standing under a name with no record.
        if let Err(e) = tokio::fs::remove_file(&meta_path).await {
            if let Err(undo) = tokio::fs::rename(&dest, &data_path).await {
                tracing::error!(
                    held_id = %id,
                    error = %undo,
                    "could not move a held volume back after its restore failed"
                );
            }
            return Err(e);
        }

        tracing::info!(
            held_id = %id,
            site_name = target_name,
            "held volume restored to site volume"
        );
        Ok(meta)
    }

    pub async fn confirm_delete_held(&self, id: &HeldVolumeId) -> std::io::Result<()> {
        let held_dir = self.held_dir();
        let data_path = held_dir.join(id.to_string());
        let meta_path = held_dir.join(format!("{id}.meta.json"));

        if !meta_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no held volume with id {id}"),
            ));
        }

        // spec: ACTU#storage-confinement
        if owned_dir_async(&data_path).await? {
            self.storage.remove(&data_path).await?;
        }

        tokio::fs::remove_file(&meta_path).await?;

        tracing::info!(held_id = %id, "held volume deleted by operator");
        Ok(())
    }

    /// Whether the volume's storage is of the kind the data directory's
    /// backend creates: a snapshottable volume on the snapshotting volume
    /// store, and a plain directory otherwise.
    // spec: OBS#volumes
    pub async fn is_backend_match(&self, name: &VolumeName) -> bool {
        let path = self.path(name);
        if !path.exists() {
            return true;
        }
        self.storage.is_snapshottable(&path).await == self.storage.snapshotting()
    }

    /// Whether the data directory is on the snapshotting volume store.
    // spec: ACTU#volume-storage
    pub fn snapshotting(&self) -> bool {
        self.storage.snapshotting()
    }

    /// Whether the storage at `path` is a snapshottable volume.
    pub async fn is_snapshottable(&self, path: &Path) -> bool {
        self.storage.is_snapshottable(path).await
    }

    /// Write `contents` to `rel_path` beneath `root`, a volume's storage,
    /// refusing a path that would leave it.
    // spec: ACTU#storage-confinement
    pub async fn write_confined(
        &self,
        root: &Path,
        rel_path: &str,
        contents: &[u8],
    ) -> Result<(), ConfinedWriteError> {
        self.storage.write_confined(root, rel_path, contents).await
    }

    pub fn site_path(&self, name: &str) -> PathBuf {
        self.volumes_dir.join(format!("site-{name}"))
    }

    /// Whether anything at all stands at this site volume's path: a
    /// directory, a file, or a symlink whose target is gone. The question
    /// callers ask is whether the name is free, so a name occupied by
    /// something the runtime would refuse to act on is not free either.
    // spec: SITE#site-volume-names
    pub fn site_exists(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.site_path(name)).is_ok()
    }

    // spec: SITE#site-volumes
    pub async fn create_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        // spec: ACTU#storage-confinement
        owned_dir_async(&path).await?;
        self.storage.create(&path).await
    }

    pub async fn remove_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        // spec: ACTU#storage-confinement
        if !owned_dir_async(&path).await? {
            return Ok(());
        }
        self.storage.remove(&path).await
    }

    // spec: SITE#site-volume-snapshots
    /// Create a read-only snapshot at `site-{name}` from `source_path`.
    /// Errors if `source_path` is not a snapshottable volume.
    pub async fn snapshot_site(
        &self,
        name: &str,
        source_path: &std::path::Path,
    ) -> std::io::Result<()> {
        if !source_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("source path {} does not exist", source_path.display()),
            ));
        }
        let dest = self.site_path(name);
        self.storage.snapshot(source_path, &dest, false).await
    }

    // spec: SITE#snapshot-promotion
    /// Create a writable snapshot at `site-{name}` from `source_path`.
    /// Used to promote a read-only snapshot site volume into a fresh
    /// read-write managed site volume. Errors when the data directory is not
    /// on the snapshotting volume store.
    pub async fn promote_site_snapshot(
        &self,
        name: &str,
        source_path: &std::path::Path,
    ) -> std::io::Result<()> {
        if !self.storage.snapshotting() {
            return Err(std::io::Error::other(
                "promoting a snapshot needs the data directory on the snapshotting volume store",
            ));
        }
        if !source_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("source path {} does not exist", source_path.display()),
            ));
        }
        let dest = self.site_path(name);
        self.storage.snapshot(source_path, &dest, true).await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeldVolumeMeta {
    pub id: HeldVolumeId,
    pub app: String,
    pub volume_name: String,
    pub display_name: String,
    pub reason: String,
    pub held_at: String,
    pub path: PathBuf,
}

fn read_held_meta(path: &Path) -> std::io::Result<HeldVolumeMeta> {
    let data = std::fs::read_to_string(path)?;
    serde_json::from_str(&data).map_err(|e| {
        std::io::Error::other(format!(
            "held volume record {} cannot be read: {e}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, VolumeStore) {
        let data = tempfile::tempdir().unwrap();
        let store = VolumeStore::new(
            data.path(),
            Arc::new(crate::system::stub::StubVolumeStorage),
        )
        .unwrap();
        (data, store)
    }

    /// A volume directory that is really a symlink — storage an operator
    /// relocated by hand, or a link somebody else placed.
    #[cfg(unix)]
    fn symlinked_volume(store: &VolumeStore, name: &VolumeName, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(target.join("data.db"), b"the volume's contents").unwrap();
        std::os::unix::fs::symlink(target, store.path(name)).unwrap();
    }

    // spec: ACTU#held-volumes
    #[tokio::test]
    async fn holding_a_volume_records_it_with_its_data_beside_the_record() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        std::fs::create_dir_all(store.path(&name)).unwrap();
        std::fs::write(store.path(&name).join("data.db"), b"contents").unwrap();

        let meta = store
            .hold(&name, "app", "uninstall")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(store.list_held().unwrap().len(), 1);
        assert!(meta.path.join("data.db").exists());
    }

    // spec: ACTU#held-volumes
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hold_whose_move_fails_leaves_no_record_and_the_data_where_it_was() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let (data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let volume = store.path(&name);
        std::fs::create_dir_all(&volume).unwrap();
        std::fs::write(volume.join("data.db"), b"contents").unwrap();
        let parent = volume.parent().unwrap().to_owned();
        if std::fs::metadata(data.path()).unwrap().uid() == 0 {
            return; // root ignores the permission that makes the move fail
        }
        // A directory that cannot be written cannot have the volume moved
        // out of it, though the record, in a directory of its own, can be.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();

        let result = store.hold(&name, "app", "uninstall").await;

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            result.is_err(),
            "the hold fails, so the removal does not go ahead"
        );
        assert!(
            store.list_held().unwrap().is_empty(),
            "no record is left behind"
        );
        assert!(volume.join("data.db").exists(), "the data is where it was");
    }

    // spec: ACTU#storage-confinement
    #[cfg(unix)]
    #[tokio::test]
    async fn removing_a_symlinked_volume_is_refused_rather_than_reported_done() {
        let (_data, store) = store();
        let outside = tempfile::tempdir().unwrap();
        let name = VolumeName::for_app("app", "data");
        symlinked_volume(&store, &name, &outside.path().join("storage"));

        let err = store.remove(&name).await.unwrap_err();

        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(
            outside.path().join("storage/data.db").exists(),
            "the data must be left alone"
        );
        assert!(
            std::fs::symlink_metadata(store.path(&name)).is_ok(),
            "refusing leaves the link in place for the operator to resolve"
        );
    }

    // spec: ACTU#storage-confinement
    #[cfg(unix)]
    #[tokio::test]
    async fn removing_a_site_volume_that_is_a_symlink_is_refused() {
        let (_data, store) = store();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("storage");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("data.db"), b"the volume's contents").unwrap();
        std::os::unix::fs::symlink(&target, store.site_path("backups")).unwrap();

        let err = store.remove_site("backups").await.unwrap_err();

        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(
            target.join("data.db").exists(),
            "the data must be left alone"
        );
    }

    // spec: ACTU#storage-confinement
    #[cfg(unix)]
    #[tokio::test]
    async fn holding_a_symlinked_volume_is_refused_rather_than_recorded_empty() {
        let (_data, store) = store();
        let outside = tempfile::tempdir().unwrap();
        let name = VolumeName::for_app("app", "data");
        symlinked_volume(&store, &name, &outside.path().join("storage"));

        let err = store
            .hold(&name, "app", "backend changed")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(
            store.list_held().unwrap().is_empty(),
            "a hold that moved only a link would record a volume with no data behind it"
        );
    }

    /// The migration moves storage, so a canonical path that is not a
    /// directory is refused rather than renamed over or read as empty.
    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn migrating_onto_a_canonical_path_that_is_a_file_is_refused_by_name() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let legacy = store.volumes_dir().join("app-data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("data.db"), b"the volume's contents").unwrap();
        std::fs::write(store.path(&name), b"not a directory").unwrap();

        let err = store
            .migrate_legacy("app-data", &name, "app")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("not a directory"), "{err}");
        assert!(
            err.to_string().contains("app-volume-data"),
            "the error must name the path: {err}"
        );
        assert!(
            legacy.join("data.db").exists(),
            "both paths must be left as they are"
        );
        assert!(store.list_held().unwrap().is_empty());
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn migrating_onto_an_empty_canonical_path_replaces_it() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let legacy = store.volumes_dir().join("app-data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("data.db"), b"the volume's contents").unwrap();
        std::fs::create_dir_all(store.path(&name)).unwrap();

        assert!(
            store
                .migrate_legacy("app-data", &name, "app")
                .await
                .unwrap()
        );

        assert!(store.path(&name).join("data.db").exists());
        assert!(!legacy.exists());
        assert!(store.list_held().unwrap().is_empty());
    }

    // spec: ACTU#held-volumes
    #[tokio::test]
    async fn migrating_onto_a_canonical_path_holding_data_holds_it_first() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        let legacy = store.volumes_dir().join("app-data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("data.db"), b"the volume's contents").unwrap();
        std::fs::create_dir_all(store.path(&name)).unwrap();
        std::fs::write(store.path(&name).join("other.db"), b"somebody else's").unwrap();

        assert!(
            store
                .migrate_legacy("app-data", &name, "app")
                .await
                .unwrap()
        );

        let held = store.list_held().unwrap();
        assert_eq!(held.len(), 1);
        assert!(held[0].path.join("other.db").exists());
        assert!(store.path(&name).join("data.db").exists());
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn removing_a_volume_that_is_not_there_is_still_nothing_to_do() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        store.remove(&name).await.unwrap();
        store.remove_site("never-made").await.unwrap();
    }

    // spec: ACTU#storage-confinement
    #[cfg(unix)]
    #[test]
    fn owned_dir_refuses_a_symlink_and_reports_an_absent_path() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        assert!(!owned_dir(&dir.path().join("nothing")).unwrap());

        std::fs::create_dir(dir.path().join("real")).unwrap();
        assert!(owned_dir(&dir.path().join("real")).unwrap());

        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let err = owned_dir(&dir.path().join("link")).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
    }
}
