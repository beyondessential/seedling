//! The modes of files only the daemon's own user may read, and of the
//! directories that hold them.
//!
//! Unix permission bits throughout: the daemon runs on Unix hosts only, so the
//! other platforms the crate builds for never reach these.

use std::{
    fs::{File, Metadata, OpenOptions},
    io,
    path::Path,
};

/// The mode of `path` when its group or others have any access to it, which a
/// private file must not give them; `None` when it is private.
// spec: INFRA#file-permissions
#[cfg(unix)]
pub(crate) fn exposed_mode(path: &Path) -> io::Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;
    let mode = path.metadata()?.permissions().mode() & 0o777;
    Ok((mode & 0o077 != 0).then_some(mode))
}

#[cfg(not(unix))]
pub(crate) fn exposed_mode(_path: &Path) -> io::Result<Option<u32>> {
    todo!("private file modes on a platform without Unix permissions")
}

/// Give the directory at `path` to its owner, and let its group read it.
#[cfg(unix)]
pub(crate) fn restrict_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o750))
}

#[cfg(not(unix))]
pub(crate) fn restrict_dir(_path: &Path) -> io::Result<()> {
    todo!("private directory modes on a platform without Unix permissions")
}

/// Open `path` for appending, creating it readable and writable by its owner
/// alone when it does not exist.
#[cfg(unix)]
pub(crate) fn open_private_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
pub(crate) fn open_private_append(_path: &Path) -> io::Result<File> {
    todo!("private file modes on a platform without Unix permissions")
}

/// What identifies the file `meta` describes, so a file renamed away and
/// replaced at the same path can be told from the one opened there.
#[cfg(unix)]
pub(crate) fn file_identity(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
pub(crate) fn file_identity(_meta: &Metadata) -> u64 {
    todo!("file identity on a platform without inodes")
}
