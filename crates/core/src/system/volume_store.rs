use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::runtime::identity::VolumeName;

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
        if !legacy_path.exists() {
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
        if !path.exists() {
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
        if !src.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("volume {display_name} does not exist"),
            ));
        }

        let held_dir = self.held_dir();
        tokio::fs::create_dir_all(&held_dir).await?;

        let id = uuid::Uuid::new_v4().to_string();
        let dest = held_dir.join(&id);
        tokio::fs::rename(&src, &dest).await?;

        let meta = HeldVolumeMeta {
            id: id.clone(),
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
    pub fn held_path(&self, id: &str) -> Option<PathBuf> {
        let held_dir = self.held_dir();
        let data_path = held_dir.join(id);
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

    pub async fn confirm_delete_held(&self, id: &str) -> std::io::Result<()> {
        let held_dir = self.held_dir();
        let data_path = held_dir.join(id);
        let meta_path = held_dir.join(format!("{id}.meta.json"));

        if !meta_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no held volume with id {id}"),
            ));
        }

        // Remove the data (could be a BTRFS subvolume or plain directory).
        if data_path.exists() {
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

    // r[impl volume.site.lifecycle]
    pub async fn create_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        if self.use_btrfs {
            btrfs_create_subvolume(&path).await
        } else {
            tokio::fs::create_dir_all(&path).await
        }
    }

    pub async fn remove_site(&self, name: &str) -> std::io::Result<()> {
        let path = self.site_path(name);
        if !path.exists() {
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
    pub id: String,
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
