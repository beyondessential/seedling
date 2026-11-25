//! Reading a definition's files from its source: a folder on disk with what
//! it includes from its repository, a lone file, or a folder of a repository
//! downloaded as an archive.

use std::{collections::BTreeMap, path::Path};

use crate::{METADATA_FILE, normalise_path};
use assembly::{Bounds, assemble, relative, repository_of, source_paths};
use tree::{Archive, Budget, Disk, Kind, locate, read_file, read_folder};

mod assembly;
mod tree;

/// Read a definition folder on disk and assemble what it includes from the
/// repository it sits in.
// spec: CLI#include-assembly
pub fn read_folder_definition(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
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
    assemble(&mut files, &tree, &within, bounds, &mut budget)?;
    Ok(files)
}

/// Read a lone file as a bundle holding it under its own name, with a
/// metadata file naming it as the only script file.
// spec: CLI#definition-sources
pub fn read_lone_file(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?
        .to_str()
        .ok_or_else(|| format!("{}: the file's name is not valid UTF-8", path.display()))?;
    if name == METADATA_FILE {
        return Err(format!(
            "{}: a lone {METADATA_FILE} is not a definition; push its folder instead",
            path.display()
        ));
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
// spec: CLI#definition-sources
// spec: CLI#include-assembly
pub fn read_archive_definition(
    tarball: &[u8],
    folder: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    // The folder comes from the URL, so it is held to the repository as
    // an archive entry is.
    let folder = normalise_path(folder)
        .map_err(|e| format!("the URL's folder lies outside the repository: {e}"))?;
    let mut archive = Archive::scan(tarball, &folder)?;
    let mut wanted = vec![folder.clone()];
    if let Some(metadata) = archive.metadata(&folder) {
        wanted.extend(source_paths(&folder, metadata)?);
    }
    archive.load(tarball, &wanted)?;
    if locate(&archive, &folder)? != Some(Kind::Dir) {
        return Ok(BTreeMap::new());
    }
    let mut budget = Budget::full();
    let mut files = read_folder(&archive, &folder, &mut budget)?;
    assemble(&mut files, &archive, &folder, Bounds::Archive, &mut budget)?;
    Ok(files)
}

#[cfg(test)]
mod tests;
