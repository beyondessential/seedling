//! Assembling a definition folder's included files from elsewhere in the
//! repository it sits in.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    ops::Bound,
    path::{Component, Path, PathBuf},
};

use seedling_bundle::{METADATA_FILE, included, normalise_path};

use super::{VCS_METADATA_DIRS, read_folder};

/// The `/`-separated paths, from the include boundary, of archive entries
/// that were neither files nor directories and so were never written out.
pub(super) type Unwritten = BTreeSet<String>;

/// The include boundary of a folder on disk: the nearest directory at or
/// above it holding a `.git` or `.jj` entry, or the folder itself.
///
/// `folder` is canonical, so that its ancestors are the directories it
/// really sits in.
// spec: CLI#include-assembly
pub(super) fn boundary_of(folder: &Path) -> Result<PathBuf, String> {
    for dir in folder.ancestors() {
        for marker in [".git", ".jj"] {
            let at = dir.join(marker);
            match at.symlink_metadata() {
                Ok(_) => return Ok(dir.to_owned()),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot read {}: {e}", at.display())),
            }
        }
    }
    Ok(folder.to_owned())
}

/// Place every file the folder's `include` table names into `files`, which
/// holds what the folder itself yielded.
///
/// `folder` lies within `boundary`; `unwritten` names what an archive held
/// there that is not on disk.
// spec: CLI#include-assembly
pub(super) fn assemble(
    files: &mut BTreeMap<String, Vec<u8>>,
    folder: &Path,
    boundary: &Path,
    unwritten: &Unwritten,
) -> Result<(), String> {
    let Some(metadata) = files.get(METADATA_FILE) else {
        return Ok(());
    };
    let table = included(metadata).map_err(|e| e.to_string())?;
    let within = relative(folder, boundary)?;
    for (key, value) in &table {
        let at = |msg: String| format!("include {key:?} from {value:?}: {msg}");
        let source = source_path(&within, value);
        let yielded = match &source {
            Some(path) => read_source(boundary, path, unwritten).map_err(at)?,
            None => None,
        };
        let has_copy = at_or_beneath(files, key).next().is_some();
        let Some(yielded) = yielded else {
            // spec: CLI#include-assembly
            // An exported bundle carries its included files, and its
            // sources may well not exist wherever it was exported to.
            if has_copy {
                continue;
            }
            return Err(at(match source {
                None => format!("lies outside {}", boundary.display()),
                Some(_) => "names nothing, and the folder holds no copy".to_owned(),
            }));
        };
        let placed: BTreeMap<String, Vec<u8>> = yielded
            .into_iter()
            .map(|(rel, contents)| {
                let path = if rel.is_empty() {
                    key.clone()
                } else {
                    format!("{key}/{rel}")
                };
                (path, contents)
            })
            .collect();
        if !has_copy {
            files.extend(placed);
        } else if !at_or_beneath(files, key).eq(placed.iter()) {
            return Err(at(
                "the folder already holds different files there; remove them or bring them in line"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

/// The files at `key` or beneath it, in path order, found by range rather
/// than by scanning every file.
fn at_or_beneath<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    key: &str,
) -> impl Iterator<Item = (&'a String, &'a Vec<u8>)> {
    let beneath = format!("{key}/");
    let exact = files.get_key_value(key);
    let under = files
        .range::<str, _>((Bound::Included(beneath.as_str()), Bound::Unbounded))
        .take_while(move |(path, _)| path.starts_with(&beneath));
    exact.into_iter().chain(under)
}

/// `folder`'s path within `boundary`, `/`-separated.
fn relative(folder: &Path, boundary: &Path) -> Result<String, String> {
    let within = folder.strip_prefix(boundary).map_err(|_| {
        format!(
            "{} does not lie within {}",
            folder.display(),
            boundary.display()
        )
    })?;
    within
        .components()
        .map(|c| match c {
            Component::Normal(name) => name
                .to_str()
                .ok_or_else(|| format!("{} is not a UTF-8 path", folder.display())),
            _ => Err(format!("{} is not a plain path", folder.display())),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

/// A key's source path from the boundary, or `None` where it is out of
/// bounds and so must not be read.
// spec: CLI#include-assembly
fn source_path(within: &str, value: &str) -> Option<String> {
    if value.starts_with('/') || value.contains('\0') {
        return None;
    }
    let joined = if within.is_empty() {
        value.to_owned()
    } else {
        format!("{within}/{value}")
    };
    let path = normalise_path(&joined).ok()?;
    if path.split('/').any(|s| VCS_METADATA_DIRS.contains(&s)) {
        return None;
    }
    Some(path)
}

/// Read the source at `path` beneath `boundary`: a file as one entry at the
/// empty path, a directory as a definition folder, or `None` where nothing
/// is there.
// spec: CLI#include-assembly
fn read_source(
    boundary: &Path,
    path: &str,
    unwritten: &Unwritten,
) -> Result<Option<BTreeMap<String, Vec<u8>>>, String> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut at = boundary.to_owned();
    let mut prefix = String::new();
    for (i, segment) in segments.iter().enumerate() {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        at.push(segment);
        if unwritten.contains(&prefix) {
            return Err(format!("{prefix} is not a regular file or directory"));
        }
        let meta = match at.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot read {}: {e}", at.display())),
        };
        if meta.file_type().is_symlink() {
            return Err(format!("{prefix} is a symlink"));
        }
        let last = i + 1 == segments.len();
        if !last && !meta.is_dir() {
            return Ok(None);
        }
    }
    let meta = at
        .symlink_metadata()
        .map_err(|e| format!("cannot read {}: {e}", at.display()))?;
    if meta.is_file() {
        let contents =
            std::fs::read(&at).map_err(|e| format!("cannot read {}: {e}", at.display()))?;
        return Ok(Some(BTreeMap::from([(String::new(), contents)])));
    }
    if !meta.is_dir() {
        return Err(format!("{path} is not a regular file or directory"));
    }
    let beneath = format!("{prefix}/");
    if let Some(special) = unwritten
        .iter()
        .find(|u| prefix.is_empty() || u.starts_with(&beneath))
    {
        return Err(format!("{special} is not a regular file or directory"));
    }
    let found = read_folder(&at)?;
    if found.is_empty() {
        return Err(format!("{path} holds no files"));
    }
    if let Some(nested) = found
        .keys()
        .find(|p| p.rsplit('/').next() == Some(METADATA_FILE))
    {
        return Err(format!(
            "{path} holds {nested}, which would not be read; only the definition folder's own {METADATA_FILE} is"
        ));
    }
    Ok(Some(found))
}
