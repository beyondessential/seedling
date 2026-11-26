//! Reading a definition's files from its source: a folder on disk with what
//! it includes from its repository, a lone file, or a folder of a repository
//! downloaded as an archive.

use std::{collections::BTreeMap, path::Path};

use semver::Version;

use crate::{BundleError, METADATA_FILE, normalise_path};
use assembly::{Bounds, assemble, hold_to_version, relative, repository_of, source_paths};
use tree::{Archive, Budget, Disk, Kind, ReadError, locate, read_file, read_folder};

mod assembly;
mod tree;

/// Why a definition source could not be read and assembled.
#[derive(Debug)]
pub enum SourceError {
    /// The metadata file breaks a bundle rule, or declares a Seedling
    /// version requirement the definition was held to and does not meet.
    Bundle(BundleError),
    /// A key of the metadata file's `include` table could not be assembled.
    Include(String),
    /// Any other reason.
    Other(String),
}

impl From<String> for SourceError {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}

impl From<ReadError> for SourceError {
    fn from(e: ReadError) -> Self {
        match e {
            // spec: CLI#check-results
            // Reading stops at a bundle limit before the bundle exists, so
            // the refused file has no place in it.
            ReadError::OverLimit(message) => Self::Bundle(BundleError::Invalid {
                message,
                place: None,
            }),
            ReadError::Other(e) => Self::Other(e),
        }
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundle(e) => e.fmt(f),
            Self::Include(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for SourceError {}

/// The files read from a definition folder, with the bundle files placed
/// from an include source each mapped to that source's path relative to the
/// definition folder.
#[derive(Debug, Default)]
pub struct Assembled {
    pub files: BTreeMap<String, Vec<u8>>,
    pub sources: BTreeMap<String, String>,
}

/// Read a definition folder on disk and assemble what it includes from the
/// repository it sits in.
///
/// Where `running` is given, the definition is held to it before its include
/// table is read, as a definition check holds it.
// spec: CLI#include-assembly
pub fn read_folder_definition(
    path: &Path,
    running: Option<&Version>,
) -> Result<Assembled, SourceError> {
    let folder = path
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let repository = repository_of(&folder)?;
    let (boundary, bounds) = match &repository {
        Some(root) => (root.as_path(), Bounds::Repository(root)),
        None => (folder.as_path(), Bounds::Folder),
    };
    let tree = Disk::new(boundary);
    let within = relative(&folder, boundary)?;
    let mut budget = Budget::full();
    let mut files = read_folder(&tree, &within, &mut budget)?;
    if let Some(metadata) = files.get(METADATA_FILE) {
        hold_to_version(running, metadata)?;
    }
    let sources = assemble(&mut files, &tree, &within, bounds, &mut budget)?;
    Ok(Assembled { files, sources })
}

/// Read a lone file as a bundle holding it under its own name, with a
/// metadata file naming it as the only script file.
// spec: CLI#definition-sources
pub fn read_lone_file(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, SourceError> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?
        .to_str()
        .ok_or_else(|| format!("{}: the file's name is not valid UTF-8", path.display()))?;
    if name == METADATA_FILE {
        return Err(format!(
            "{}: a lone {METADATA_FILE} is not a definition; push its folder instead",
            path.display()
        )
        .into());
    }
    let contents = read_file(path, &mut Budget::full())?;
    let metadata = toml::Table::from_iter([(
        "script".to_owned(),
        toml::Value::Array(vec![toml::Value::String(name.to_owned())]),
    )]);
    Ok(BTreeMap::from([
        (name.to_owned(), contents),
        (METADATA_FILE.to_owned(), metadata.to_string().into_bytes()),
    ]))
}

/// Read the definition at `folder` out of a GitHub tarball, with what
/// its include table names from elsewhere in the repository.
///
/// The repository is read in memory, and only the folder and its sources
/// are held, so a repository far larger than any definition costs no more
/// than the definition does.
///
/// Where `running` is given, the definition is held to it before its include
/// table is read, as a definition check holds it.
// spec: CLI#definition-sources
// spec: CLI#include-assembly
pub fn read_archive_definition(
    tarball: &[u8],
    folder: &str,
    running: Option<&Version>,
) -> Result<Assembled, SourceError> {
    // The folder comes from the URL, so it is held to the repository as
    // an archive entry is.
    let folder = normalise_path(folder)
        .map_err(|e| format!("the URL's folder lies outside the repository: {e}"))?;
    let mut archive = Archive::scan(tarball, &folder)?;
    let mut wanted = vec![folder.clone()];
    if let Some(metadata) = archive.metadata(&folder) {
        hold_to_version(running, metadata)?;
        wanted.extend(source_paths(&folder, metadata).map_err(SourceError::Bundle)?);
    }
    archive.load(tarball, &wanted)?;
    if locate(&archive, &folder)? != Some(Kind::Dir) {
        return Ok(Assembled::default());
    }
    let mut budget = Budget::full();
    let mut files = read_folder(&archive, &folder, &mut budget)?;
    let sources = assemble(&mut files, &archive, &folder, Bounds::Archive, &mut budget)?;
    Ok(Assembled { files, sources })
}

#[cfg(test)]
mod tests;
