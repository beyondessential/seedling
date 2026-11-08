//! Host filesystem paths the daemon confines to a root it owns.
//!
//! One rule, in two shapes. A path the daemon resolves is resolved under
//! the root at every component, not only at the last: `openat2` with
//! `RESOLVE_BENEATH` makes the kernel atomically reject any component
//! that would escape `root` via `..`, a symlink, or a mount-point
//! crossing. Requires Linux >= 5.6.
//!
//! A path the daemon *is* the owner of — a volume's own directory, as
//! against a path inside one — is not resolved at all: it is required to
//! be a real directory. A symlink there is somebody else's storage
//! wearing the name, so an operation on it is refused rather than
//! followed.

use std::{
    io::Write,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    path::{Component, Path, PathBuf},
};

use rustix::fs::{self, Mode, OFlags, ResolveFlags, mkdirat};
use snafu::{IntoError, Snafu};

/// Default file mode for written files: owner rw, group r (0640).
const FILE_MODE: Mode = Mode::from_raw_mode(0o640);

/// Default directory mode for created parent dirs: owner rwx, group rx (0750).
const DIR_MODE: Mode = Mode::from_raw_mode(0o750);

/// Error returned by confined write operations.
#[derive(Debug, Snafu)]
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

/// Write `contents` to `rel_path` beneath `root`, creating parent
/// directories as needed. Both directory creation and the final file
/// open are kernel-confined via `openat2(RESOLVE_BENEATH)`.
///
/// Each parent is created relative to a descriptor the kernel resolved
/// from `root` for the prefix above it, so the walk reaches exactly what
/// the final open would and a symlink part-way along the path can no
/// more place a directory outside `root` than it can place the file.
///
/// `rel_path` is interpreted relative to `root` — any leading `/` is
/// stripped. An empty path (or one that resolves to just the root) is
/// rejected.
///
/// Written files get mode 0640; created directories get mode 0750.
///
/// This is a blocking function; see [`write_async`] for the async wrapper.
pub fn write(root: &Path, rel_path: &str, contents: &[u8]) -> Result<(), ConfinedWriteError> {
    let rel = rel_path.trim_start_matches('/');
    if rel.is_empty() {
        return EscapeSnafu {
            path: root.join(rel_path),
        }
        .fail();
    }

    let dir_fd = fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| {
        IoSnafu {
            path: root.to_path_buf(),
        }
        .into_error(std::io::Error::from(e))
    })?;

    // r[impl storage.confinement] — create the parents one segment at a
    // time, each relative to a descriptor the kernel resolved beneath the
    // root for the prefix above it.
    let rel_obj = Path::new(rel);
    if let Some(parent) = rel_obj.parent() {
        let mut accum = PathBuf::new();
        let mut cursor = dir_fd.try_clone().map_err(|e| {
            IoSnafu {
                path: root.to_path_buf(),
            }
            .into_error(e)
        })?;
        for component in parent.components() {
            match component {
                Component::Normal(seg) => {
                    // A single segment, so `mkdirat` has nothing to resolve
                    // through: it creates in the directory the cursor holds
                    // open, which is beneath the root by construction.
                    match mkdirat(cursor.as_fd(), seg, DIR_MODE) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(e) => {
                            return Err(IoSnafu {
                                path: root.join(&accum).join(seg),
                            }
                            .into_error(std::io::Error::from(e)));
                        }
                    }
                    accum.push(seg);
                }
                Component::CurDir => continue,
                // Carried rather than resolved here: the kernel decides
                // where it lands, including where the prefix above it went
                // through a symlink, and refuses it if it leaves the root.
                Component::ParentDir => accum.push(".."),
                Component::RootDir | Component::Prefix(_) => {
                    return EscapeSnafu {
                        path: root.join(rel),
                    }
                    .fail();
                }
            }
            cursor = open_beneath(dir_fd.as_fd(), &accum, root)?;
        }
    }

    // Open (or create) the target file with RESOLVE_BENEATH.
    let fd = fs::openat2(
        dir_fd.as_fd(),
        rel,
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::CLOEXEC,
        FILE_MODE,
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|e| resolve_error(e, root.join(rel)))?;

    let mut file = std::fs::File::from(fd);
    file.write_all(contents).map_err(|e| {
        IoSnafu {
            path: root.join(rel),
        }
        .into_error(e)
    })?;

    Ok(())
}

/// Open the directory at `rel` — a path relative to `dir_fd` — under the
/// same confinement the final open uses, so the walk and the open agree
/// on where the path lands.
fn open_beneath(
    dir_fd: BorrowedFd<'_>,
    rel: &Path,
    root: &Path,
) -> Result<OwnedFd, ConfinedWriteError> {
    fs::openat2(
        dir_fd,
        rel,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|e| resolve_error(e, root.join(rel)))
}

/// Classify what the kernel said about a confined resolution.
fn resolve_error(e: rustix::io::Errno, path: PathBuf) -> ConfinedWriteError {
    // EXDEV is returned when RESOLVE_BENEATH detects an escape.
    if e == rustix::io::Errno::XDEV {
        return EscapeSnafu { path }.build();
    }
    IoSnafu { path }.into_error(std::io::Error::from(e))
}

/// Whether `path` is a directory the daemon may act on as its own
/// storage, `false` when nothing is there at all.
///
/// A symlink is refused rather than followed: whatever it points at is
/// somebody else's, so creating, moving or deleting through it would act
/// outside the root that owns the name. So is any other kind of file,
/// which is not storage the daemon wrote.
// r[impl storage.confinement]
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

/// Async wrapper around [`write`] that runs the blocking syscalls on
/// Tokio's blocking thread pool.
pub async fn write_async(
    root: &Path,
    rel_path: &str,
    contents: &[u8],
) -> Result<(), ConfinedWriteError> {
    let root = root.to_path_buf();
    let rel_path = rel_path.to_owned();
    let contents = contents.to_owned();

    tokio::task::spawn_blocking(move || write(&root, &rel_path, &contents))
        .await
        .unwrap_or_else(|e| {
            Err(IoSnafu {
                path: PathBuf::from("<task join error>"),
            }
            .into_error(std::io::Error::other(e)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_rel_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = write(dir.path(), "", b"data").unwrap_err();
        assert!(matches!(err, ConfinedWriteError::Escape { .. }));
    }

    #[test]
    fn rejects_root_only_slash() {
        let dir = tempfile::tempdir().unwrap();
        let err = write(dir.path(), "/", b"data").unwrap_err();
        assert!(matches!(err, ConfinedWriteError::Escape { .. }));
    }

    #[test]
    fn writes_simple_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "hello.txt", b"world").unwrap();
        let contents = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert_eq!(contents, "world");
    }

    #[test]
    fn writes_with_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "/hello.txt", b"world").unwrap();
        let contents = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert_eq!(contents, "world");
    }

    #[test]
    fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/b/c/file.txt", b"nested").unwrap();
        let contents = std::fs::read_to_string(dir.path().join("a/b/c/file.txt")).unwrap();
        assert_eq!(contents, "nested");
    }

    #[test]
    fn rejects_dotdot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let err = write(dir.path(), "../escape.txt", b"evil").unwrap_err();
        assert!(matches!(err, ConfinedWriteError::Escape { .. }));
    }

    #[test]
    fn rejects_mid_path_dotdot_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let err = write(dir.path(), "sub/../../escape.txt", b"evil").unwrap_err();
        assert!(matches!(err, ConfinedWriteError::Escape { .. }));
    }

    #[test]
    fn rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), dir.path().join("link")).unwrap();
        let err = write(dir.path(), "link/file.txt", b"evil").unwrap_err();
        // NO_MAGICLINKS + BENEATH blocks symlink traversal outside root.
        assert!(matches!(
            err,
            ConfinedWriteError::Escape { .. } | ConfinedWriteError::Io { .. }
        ));
    }

    // r[verify storage.confinement]
    #[test]
    fn a_symlinked_parent_creates_nothing_outside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("data")).unwrap();

        let err = write(dir.path(), "data/newdir/f.txt", b"evil").unwrap_err();

        assert!(matches!(err, ConfinedWriteError::Escape { .. }));
        assert!(
            !outside.path().join("newdir").exists(),
            "the parent walk must not create a directory the open would refuse to write in"
        );
    }

    // r[verify storage.confinement]
    #[test]
    fn dotdot_inside_the_root_lands_where_the_open_would() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "sub/../sib/f.txt", b"inside").unwrap();
        let contents = std::fs::read_to_string(dir.path().join("sib/f.txt")).unwrap();
        assert_eq!(contents, "inside");
    }

    // r[verify storage.confinement]
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

    #[test]
    fn file_permissions_are_0640() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "secret.conf", b"key=value").unwrap();
        let meta = std::fs::metadata(dir.path().join("secret.conf")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "f.txt", b"first").unwrap();
        write(dir.path(), "f.txt", b"second").unwrap();
        let contents = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
        assert_eq!(contents, "second");
    }

    #[tokio::test]
    async fn async_write_works() {
        let dir = tempfile::tempdir().unwrap();
        write_async(dir.path(), "/a/b.txt", b"async").await.unwrap();
        let contents = std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap();
        assert_eq!(contents, "async");
    }
}
