//! Assembling a definition folder's included files from elsewhere in the
//! repository it sits in.

use std::{
    collections::BTreeMap,
    io::ErrorKind,
    ops::Bound,
    path::{Component, Path, PathBuf},
};

use seedling_bundle::{METADATA_FILE, included, normalise_path};

use super::tree::{Budget, Kind, Tree, VCS_METADATA_DIRS, locate, read_folder};

/// What an include boundary is, for telling the operator a source lies
/// outside it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Bounds<'a> {
    /// A repository on disk, at this path.
    Repository(&'a Path),
    /// The definition folder itself, which is in no repository.
    Folder,
    /// A repository downloaded as an archive.
    Archive,
}

impl std::fmt::Display for Bounds<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(at) => write!(f, "the repository at {}", at.display()),
            Self::Folder => f.write_str("the definition folder, which is in no repository"),
            Self::Archive => f.write_str("the repository"),
        }
    }
}

/// The repository a folder on disk sits in, if any: the nearest directory at
/// or above it holding a `.git` or `.jj` entry. Where there is one it is the
/// folder's include boundary, and where there is none the folder itself is.
///
/// `folder` is canonical, so that its ancestors are the directories it
/// really sits in.
// spec: CLI#include-assembly
pub(super) fn repository_of(folder: &Path) -> Result<Option<PathBuf>, String> {
    for dir in folder.ancestors() {
        for marker in [".git", ".jj"] {
            let at = dir.join(marker);
            match at.symlink_metadata() {
                Ok(_) => return Ok(Some(dir.to_owned())),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot read {}: {e}", at.display())),
            }
        }
    }
    Ok(None)
}

/// Place every file the folder's `include` table names into `files`, which
/// holds what the folder itself yielded.
///
/// `tree` is rooted at the include boundary, which `bounds` says what it is,
/// and `folder` is the definition folder's path within it.
// spec: CLI#include-assembly
pub(super) fn assemble(
    files: &mut BTreeMap<String, Vec<u8>>,
    tree: &impl Tree,
    folder: &str,
    bounds: Bounds<'_>,
    budget: &mut Budget,
) -> Result<(), String> {
    let Some(metadata) = files.get(METADATA_FILE) else {
        return Ok(());
    };
    let table = included(metadata).map_err(|e| e.to_string())?;
    for (key, value) in &table {
        let at = |msg: String| format!("include {key:?} from {value:?}: {msg}");
        let source = source_path(folder, value);
        let has_copy = at_or_beneath(files, key).next().is_some();
        // A source matching the folder's copy adds nothing to the bundle, so
        // the copy's share of the budget is lent to reading it, and the
        // budget is as it was wherever the copy is what the bundle keeps.
        let before = *budget;
        budget.give_back(at_or_beneath(files, key));
        let yielded = match &source {
            Some(path) => read_source(tree, path, budget).map_err(at)?,
            None => None,
        };
        let Some(yielded) = yielded else {
            // spec: CLI#include-assembly
            // An exported bundle carries its included files, and its
            // sources may well not exist wherever it was exported to.
            if has_copy {
                *budget = before;
                continue;
            }
            return Err(at(match source {
                None => format!("lies outside {bounds}"),
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
        } else if at_or_beneath(files, key).eq(placed.iter()) {
            *budget = before;
        } else {
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
pub(super) fn relative(folder: &Path, boundary: &Path) -> Result<String, String> {
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

/// The source paths the include table in `metadata` names for a definition
/// folder at `within`, leaving out those out of bounds, which are never read.
// spec: CLI#include-assembly
pub(super) fn source_paths(within: &str, metadata: &[u8]) -> Result<Vec<String>, String> {
    Ok(included(metadata)
        .map_err(|e| e.to_string())?
        .values()
        .filter_map(|value| source_path(within, value))
        .collect())
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

/// Read the source at `path`: a file as one entry at the empty path, a
/// directory as a definition folder, or `None` where nothing is there.
// spec: CLI#include-assembly
fn read_source(
    tree: &impl Tree,
    path: &str,
    budget: &mut Budget,
) -> Result<Option<BTreeMap<String, Vec<u8>>>, String> {
    match locate(tree, path)? {
        None => Ok(None),
        Some(Kind::File) => {
            budget.take_file(&tree.show(path), 0)?;
            let contents = tree.read(path, budget)?;
            Ok(Some(BTreeMap::from([(String::new(), contents)])))
        }
        Some(Kind::Dir) => {
            let found = read_folder(tree, path, budget)?;
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
        Some(Kind::Other) => Err(format!("{path} is not a regular file or directory")),
    }
}
