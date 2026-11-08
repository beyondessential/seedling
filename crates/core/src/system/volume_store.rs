use std::path::{Path, PathBuf};

use seedling_protocol::names::HeldVolumeId;
use serde::{Deserialize, Serialize};

use crate::{runtime::identity::VolumeName, system::confined::owned_dir_async};

/// Manages host-backed storage for named volumes.
///
/// Named non-tmpfs volumes are stored as directories (or BTRFS subvolumes)
/// under `{data_dir}/volumes/`. They are bind-mounted into containers rather
/// than using podman-managed volumes.
pub struct VolumeStore {
    volumes_dir: PathBuf,
    use_btrfs: bool,
}

impl VolumeStore {
    pub fn new(data_dir: &Path, use_btrfs: bool) -> std::io::Result<Self> {
        let volumes_dir = data_dir.join("volumes");
        std::fs::create_dir_all(&volumes_dir)?;
        Ok(Self {
            volumes_dir,
            use_btrfs,
        })
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

    // r[impl actuate.volume.btrfs]
    pub async fn create(&self, name: &VolumeName) -> std::io::Result<()> {
        let path = self.path(name);
        // r[impl storage.confinement]
        owned_dir_async(&path).await?;
        if self.use_btrfs {
            btrfs_create_subvolume(&path).await
        } else {
            tokio::fs::create_dir_all(&path).await
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
    /// nothing to migrate. If the canonical path already exists, it is
    /// removed (when empty) or held (when not) before the legacy path is
    /// renamed in place so nothing is silently dropped.
    pub async fn migrate_legacy(
        &self,
        legacy: &str,
        canonical: &VolumeName,
        app: &str,
    ) -> std::io::Result<bool> {
        let legacy_path = self.volumes_dir.join(legacy);
        // r[impl storage.confinement] — the migration moves the storage, so
        // a symlink wearing the legacy name is refused rather than rehomed.
        if !owned_dir_async(&legacy_path).await? {
            return Ok(false);
        }
        let canonical_path = self.path(canonical);
        if legacy_path == canonical_path {
            return Ok(false);
        }

        if canonical_path.exists() {
            let is_empty = match tokio::fs::read_dir(&canonical_path).await {
                Ok(mut rd) => rd.next_entry().await?.is_none(),
                Err(e) => return Err(e),
            };
            if is_empty {
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
        // r[impl storage.confinement] — a symlink here is refused rather
        // than unlinked, so a volume whose storage was relocated by hand
        // is never reported as deleted while its data stays where it is.
        if !owned_dir_async(&path).await? {
            return Ok(());
        }
        // Always check whether the path is actually a BTRFS subvolume,
        // regardless of the current use_btrfs setting. A volume created
        // under BTRFS mode must be removed with `btrfs subvolume delete`
        // even if seedling was restarted with --without-btrfs.
        if is_btrfs_subvolume(&path).await {
            btrfs_delete_subvolume(&path).await
        } else {
            tokio::fs::remove_dir_all(&path).await
        }
    }

    fn held_dir(&self) -> PathBuf {
        self.volumes_dir
            .parent()
            .expect("volumes_dir has a parent")
            .join("held-volumes")
    }

    // r[impl actuate.volume.hold]
    pub async fn hold(
        &self,
        name: &VolumeName,
        app: &str,
        reason: &str,
    ) -> std::io::Result<HeldVolumeMeta> {
        let s = name.as_str();
        self.hold_inner(&self.path(name), s, s, app, reason).await
    }

    /// Hold a managed or snapshot site volume for operator review.
    ///
    /// The on-disk path `site-{name}` is moved into the held volumes
    /// directory; the resulting record carries the literal app name `_site`
    /// so UIs can distinguish site-origin holds from app-origin ones.
    // r[impl actuate.volume.hold]
    pub async fn hold_site(&self, name: &str, reason: &str) -> std::io::Result<HeldVolumeMeta> {
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
    ) -> std::io::Result<HeldVolumeMeta> {
        // r[impl storage.confinement] — holding moves the storage aside,
        // and moving a symlink would record a held volume with none of the
        // data behind it.
        if !owned_dir_async(src).await? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("volume {display_name} does not exist"),
            ));
        }

        let held_dir = self.held_dir();
        tokio::fs::create_dir_all(&held_dir).await?;

        let id = HeldVolumeId::generate();
        let dest = held_dir.join(id.to_string());
        tokio::fs::rename(&src, &dest).await?;

        let meta = HeldVolumeMeta {
            id,
            app: app.to_owned(),
            volume_name: volume_name.to_owned(),
            display_name: display_name.to_owned(),
            reason: reason.to_owned(),
            held_at: jiff::Timestamp::now().to_string(),
            path: dest,
        };

        let meta_path = held_dir.join(format!("{id}.meta.json"));
        let json = serde_json::to_string_pretty(&meta).map_err(std::io::Error::other)?;
        tokio::fs::write(&meta_path, json).await?;

        tracing::info!(
            app = app,
            volume = volume_name,
            held_id = %id,
            reason = reason,
            "volume held for operator review"
        );

        Ok(meta)
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
                let data = std::fs::read_to_string(&path)?;
                if let Ok(meta) = serde_json::from_str::<HeldVolumeMeta>(&data) {
                    results.push(meta);
                }
            }
        }
        Ok(results)
    }

    /// Move a held volume's data into a fresh managed site volume named
    /// `target_name`. Errors if the held id is unknown, if the held data
    /// directory is missing, or if a site volume with the target name is
    /// already present on disk. The caller is responsible for inserting the
    /// `site_volumes` row and emitting the corresponding events.
    // r[impl actuate.volume.hold.restore]
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
        // r[impl storage.confinement]
        if !owned_dir_async(&data_path).await? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("held volume {id} has no backing data directory"),
            ));
        }

        let dest = self.site_path(target_name);
        // r[impl storage.confinement]
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
        tokio::fs::remove_file(&meta_path).await?;

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

        // Remove the data (could be a BTRFS subvolume or plain directory).
        // r[impl storage.confinement]
        if owned_dir_async(&data_path).await? {
            if is_btrfs_subvolume(&data_path).await {
                btrfs_delete_subvolume(&data_path).await?;
            } else {
                tokio::fs::remove_dir_all(&data_path).await?;
            }
        }

        tokio::fs::remove_file(&meta_path).await?;

        tracing::info!(held_id = %id, "held volume deleted by operator");
        Ok(())
    }

    pub async fn is_backend_match(&self, name: &VolumeName) -> bool {
        let path = self.path(name);
        if !path.exists() {
            return true;
        }
        let is_subvol = is_btrfs_subvolume(&path).await;
        is_subvol == self.use_btrfs
    }

    pub fn site_path(&self, name: &str) -> PathBuf {
        self.volumes_dir.join(format!("site-{name}"))
    }

    pub fn site_exists(&self, name: &str) -> bool {
        self.site_path(name).exists()
    }

    /// Return the names (without the `site-` prefix) of every site volume
    /// whose name starts with `prefix`.
    pub fn list_sites_with_prefix(&self, prefix: &str) -> std::io::Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.volumes_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&self.volumes_dir)? {
            let entry = entry?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(rest) = file_name.strip_prefix("site-") else {
                continue;
            };
            if rest.starts_with(prefix) {
                out.push(rest.to_owned());
            }
        }
        Ok(out)
    }

    // r[impl volume.site.lifecycle]
    pub async fn create_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        // r[impl storage.confinement]
        owned_dir_async(&path).await?;
        if self.use_btrfs {
            btrfs_create_subvolume(&path).await
        } else {
            tokio::fs::create_dir_all(&path).await
        }
    }

    pub async fn remove_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        // r[impl storage.confinement]
        if !owned_dir_async(&path).await? {
            return Ok(());
        }
        if is_btrfs_subvolume(&path).await {
            btrfs_delete_subvolume(&path).await
        } else {
            tokio::fs::remove_dir_all(&path).await
        }
    }

    // r[impl volume.site.snapshot]
    /// Create a read-only BTRFS snapshot at `site-{name}` from `source_path`.
    /// Errors if BTRFS is not in use.
    pub async fn snapshot_site(
        &self,
        name: &str,
        source_path: &std::path::Path,
    ) -> std::io::Result<()> {
        if !self.use_btrfs {
            return Err(std::io::Error::other(
                "snapshotting requires BTRFS; restart without --without-btrfs",
            ));
        }
        if !source_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("source path {} does not exist", source_path.display()),
            ));
        }
        let dest = self.site_path(name);
        btrfs_snapshot_readonly(source_path, &dest).await
    }

    // r[impl volume.site.promote]
    /// Create a writable BTRFS snapshot at `site-{name}` from `source_path`.
    /// Used to promote a read-only snapshot site volume into a fresh
    /// read-write managed site volume. Errors if BTRFS is not in use.
    pub async fn promote_site_snapshot(
        &self,
        name: &str,
        source_path: &std::path::Path,
    ) -> std::io::Result<()> {
        if !self.use_btrfs {
            return Err(std::io::Error::other(
                "promoting a snapshot requires BTRFS; restart without --without-btrfs",
            ));
        }
        if !source_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("source path {} does not exist", source_path.display()),
            ));
        }
        let dest = self.site_path(name);
        btrfs_snapshot_writable(source_path, &dest).await
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

async fn is_btrfs_subvolume(path: &Path) -> bool {
    // `btrfs subvolume show` exits 0 only for subvolumes.
    tokio::process::Command::new("btrfs")
        .args(["subvolume", "show"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

// r[impl startup.btrfs]
pub fn is_btrfs(path: &Path) -> std::io::Result<bool> {
    const BTRFS_SUPER_MAGIC: u64 = 0x9123683E;
    let stat = rustix::fs::statfs(path).map_err(std::io::Error::from)?;
    Ok(stat.f_type as u64 == BTRFS_SUPER_MAGIC)
}

async fn btrfs_create_subvolume(path: &Path) -> std::io::Result<()> {
    let output = tokio::process::Command::new("btrfs")
        .args(["subvolume", "create"])
        .arg(path)
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "btrfs subvolume create failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

async fn btrfs_delete_subvolume(path: &Path) -> std::io::Result<()> {
    let output = tokio::process::Command::new("btrfs")
        .args(["subvolume", "delete"])
        .arg(path)
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "btrfs subvolume delete failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

async fn btrfs_snapshot_readonly(source: &Path, dest: &Path) -> std::io::Result<()> {
    let output = tokio::process::Command::new("btrfs")
        .args(["subvolume", "snapshot", "-r"])
        .arg(source)
        .arg(dest)
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "btrfs subvolume snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

async fn btrfs_snapshot_writable(source: &Path, dest: &Path) -> std::io::Result<()> {
    let output = tokio::process::Command::new("btrfs")
        .args(["subvolume", "snapshot"])
        .arg(source)
        .arg(dest)
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "btrfs subvolume snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, VolumeStore) {
        let data = tempfile::tempdir().unwrap();
        let store = VolumeStore::new(data.path(), false).unwrap();
        (data, store)
    }

    /// A volume directory that is really a symlink — storage an operator
    /// relocated by hand, or a link somebody else placed.
    fn symlinked_volume(store: &VolumeStore, name: &VolumeName, target: &Path) {
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(target.join("data.db"), b"the volume's contents").unwrap();
        std::os::unix::fs::symlink(target, store.path(name)).unwrap();
    }

    // r[verify storage.confinement]
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

    // r[verify storage.confinement]
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

    // r[verify storage.confinement]
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

    // r[verify storage.confinement]
    #[tokio::test]
    async fn removing_a_volume_that_is_not_there_is_still_nothing_to_do() {
        let (_data, store) = store();
        let name = VolumeName::for_app("app", "data");
        store.remove(&name).await.unwrap();
        store.remove_site("never-made").await.unwrap();
    }
}
