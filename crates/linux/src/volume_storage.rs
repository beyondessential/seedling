//! Volume storage on Linux: BTRFS subvolumes when the data directory is on
//! BTRFS, the snapshotting volume store, and plain directories otherwise.

use std::path::Path;

use futures_util::FutureExt;
use seedling_core::system::{
    BoxFuture,
    volume_store::{ConfinedWriteError, VolumeStorage},
};

/// Volumes as BTRFS subvolumes or plain directories.
pub struct HostVolumeStorage {
    btrfs: bool,
}

impl HostVolumeStorage {
    /// Storage that creates subvolumes when `btrfs` is set, as it is when the
    /// data directory is on BTRFS, and plain directories otherwise.
    pub fn new(btrfs: bool) -> Self {
        Self { btrfs }
    }
}

impl VolumeStorage for HostVolumeStorage {
    // spec: LINUX#volume-storage
    fn snapshotting(&self) -> bool {
        self.btrfs
    }

    // spec: LINUX#volume-storage
    fn create<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, std::io::Result<()>> {
        async move {
            if self.btrfs {
                btrfs_create_subvolume(path).await
            } else {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::create_dir(path).await
            }
        }
        .boxed()
    }

    // spec: LINUX#volume-storage
    fn remove<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, std::io::Result<()>> {
        async move {
            // Always check whether the path is actually a BTRFS subvolume,
            // whatever this storage creates. A volume created on BTRFS must be
            // removed by deleting the subvolume even if seedling was restarted
            // with --without-btrfs.
            if is_btrfs_subvolume(path).await {
                btrfs_delete_subvolume(path).await
            } else {
                tokio::fs::remove_dir_all(path).await
            }
        }
        .boxed()
    }

    fn is_snapshottable<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, bool> {
        is_btrfs_subvolume(path).boxed()
    }

    // spec: LINUX#volume-storage
    fn snapshot<'a>(
        &'a self,
        source: &'a Path,
        dest: &'a Path,
        writable: bool,
    ) -> BoxFuture<'a, std::io::Result<()>> {
        async move {
            let mut args = vec!["subvolume", "snapshot"];
            if !writable {
                args.push("-r");
            }
            let output = tokio::process::Command::new("btrfs")
                .args(args)
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
        .boxed()
    }

    // spec: ACTU#storage-confinement
    fn write_confined<'a>(
        &'a self,
        root: &'a Path,
        rel_path: &'a str,
        contents: &'a [u8],
    ) -> BoxFuture<'a, Result<(), ConfinedWriteError>> {
        seedling_core::system::confined::write_async(root, rel_path, contents).boxed()
    }
}

/// Whether `path` is on a BTRFS filesystem, as the data directory must be to
/// be on the snapshotting volume store.
// spec: LINUX#volume-storage
pub fn is_btrfs(path: &Path) -> std::io::Result<bool> {
    const BTRFS_SUPER_MAGIC: u64 = 0x9123683E;
    let stat = rustix::fs::statfs(path).map_err(std::io::Error::from)?;
    Ok(stat.f_type as u64 == BTRFS_SUPER_MAGIC)
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

#[cfg(test)]
mod tests {
    use seedling_core::{runtime::identity::VolumeName, system::volume_store::VolumeStore};

    use super::*;

    fn store(data: &Path) -> VolumeStore {
        VolumeStore::new(data, std::sync::Arc::new(HostVolumeStorage::new(false))).unwrap()
    }

    // spec: ACTU#volume-creation-and-seeding
    #[tokio::test]
    async fn plain_directories_report_which_create_made_the_volume() {
        let data = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let name = VolumeName::for_app("app", "data");

        assert!(store.create(&name).await.unwrap());
        assert!(!store.create(&name).await.unwrap());
        assert!(store.is_backend_match(&name).await);
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn a_seed_write_that_escapes_the_volume_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let store = store(data.path());
        let name = VolumeName::for_app("app", "data");
        store.create(&name).await.unwrap();

        let err = store
            .write_confined(&store.path(&name), "../escape", b"elsewhere")
            .await
            .unwrap_err();

        assert!(matches!(err, ConfinedWriteError::Escape { .. }), "{err}");
    }
}
