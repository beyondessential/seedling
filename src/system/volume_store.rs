use std::path::{Path, PathBuf};

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

    pub fn path(&self, name: &str) -> PathBuf {
        self.volumes_dir.join(name)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    // r[impl actuate.volume.btrfs]
    pub async fn create(&self, name: &str) -> std::io::Result<()> {
        let path = self.path(name);
        if self.use_btrfs {
            btrfs_create_subvolume(&path).await
        } else {
            tokio::fs::create_dir_all(&path).await
        }
    }

    pub async fn remove(&self, name: &str) -> std::io::Result<()> {
        let path = self.path(name);
        if !path.exists() {
            return Ok(());
        }
        if self.use_btrfs {
            btrfs_delete_subvolume(&path).await
        } else {
            tokio::fs::remove_dir_all(&path).await
        }
    }
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
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "btrfs subvolume create failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
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
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "btrfs subvolume delete failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}
