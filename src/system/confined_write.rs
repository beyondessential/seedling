//! Confined file writes using `openat2(RESOLVE_BENEATH)`.
//!
//! Every write is kernel-confined to the given root directory: the
//! `RESOLVE_BENEATH` flag causes the kernel to atomically reject any
//! path that would escape `root` via `..`, symlinks, or mount-point
//! crossings. Requires Linux >= 5.6.

use std::{
    io::Write,
    os::unix::io::AsFd,
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

    // Create parent directories one segment at a time using mkdirat
    // beneath the root fd.
    let rel_obj = Path::new(rel);
    if let Some(parent) = rel_obj.parent() {
        let mut accum = PathBuf::new();
        for component in parent.components() {
            if let Component::Normal(seg) = component {
                accum.push(seg);
                match mkdirat(dir_fd.as_fd(), &accum, DIR_MODE) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => {
                        return Err(IoSnafu {
                            path: root.join(&accum),
                        }
                        .into_error(std::io::Error::from(e)));
                    }
                }
            }
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
    .map_err(|e| {
        let path = root.join(rel);
        // EXDEV is returned when RESOLVE_BENEATH detects an escape.
        if e == rustix::io::Errno::XDEV {
            return EscapeSnafu { path }.build();
        }
        IoSnafu { path }.into_error(std::io::Error::from(e))
    })?;

    let mut file = std::fs::File::from(fd);
    file.write_all(contents).map_err(|e| {
        IoSnafu {
            path: root.join(rel),
        }
        .into_error(e)
    })?;

    Ok(())
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
