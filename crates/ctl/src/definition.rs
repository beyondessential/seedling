//! Reading an app definition from wherever the operator points at it, and
//! turning it into the fields a definition request carries.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use seedling_bundle::{Bundle, METADATA_FILE, VERSIONS_ANNOTATION};
use serde_json::{Map, Value, json};

use assembly::{Unwritten, assemble, boundary_of};

mod assembly;

/// Where a definition comes from.
// spec: CLI#definition-sources
#[derive(clap::Args, Debug, Clone, Default)]
pub(super) struct DefinitionArgs {
    /// A script file, a definition folder, or a GitHub URL naming a folder
    /// at a branch, tag, or commit
    pub source: Option<String>,
    /// Have the daemon fetch the definition from this OCI reference
    #[arg(long = "ref", value_name = "REFERENCE", conflicts_with = "source")]
    pub reference: Option<String>,
}

/// A definition resolved on this side, before it is sent.
#[derive(Debug, PartialEq)]
pub(super) enum Definition {
    Bundle {
        files: BTreeMap<String, Vec<u8>>,
        origin: Option<(String, String)>,
    },
    Reference(String),
}

/// The field names a request carries a definition under.
///
/// Each request shape that takes a definition names one set, so the mapping
/// is enumerated here rather than derived from another shape's keys.
// spec: CLI#definition-sources
#[derive(Clone, Copy)]
pub(super) struct DefinitionKeys {
    pub bundle: &'static str,
    pub reference: &'static str,
    /// `None` where the request has nowhere to report an origin.
    pub origin: Option<&'static str>,
}

impl DefinitionKeys {
    /// `/apps/create` and `/apps/update`.
    pub const APP: Self = Self {
        bundle: "bundle",
        reference: "reference",
        origin: Some("origin"),
    };
    /// `/templates/create` and `/templates/update`.
    pub const TEMPLATE: Self = Self {
        bundle: "bundle",
        reference: "reference",
        origin: Some("origin"),
    };
    /// The definition `/apps/plan` is asked to compare against, which it
    /// never installs and so records no origin for.
    pub const PROPOSED: Self = Self {
        bundle: "proposed_bundle",
        reference: "proposed_reference",
        origin: None,
    };
}

impl Definition {
    /// The request fields for this definition, under `keys`.
    pub fn fields(&self, keys: DefinitionKeys) -> Map<String, Value> {
        let mut out = Map::new();
        match self {
            Self::Bundle { files, origin } => {
                let encoded: Map<String, Value> = files
                    .iter()
                    .map(|(p, c)| (p.clone(), json!(BASE64.encode(c))))
                    .collect();
                out.insert(keys.bundle.to_owned(), Value::Object(encoded));
                if let (Some((url, revision)), Some(key)) = (origin, keys.origin) {
                    out.insert(key.to_owned(), json!({ "url": url, "revision": revision }));
                }
            }
            Self::Reference(r) => {
                out.insert(keys.reference.to_owned(), json!(r));
            }
        }
        out
    }
}

impl DefinitionArgs {
    /// Resolve the definition, or `None` when none was given.
    pub async fn resolve(&self) -> Result<Option<Definition>, String> {
        if let Some(r) = &self.reference {
            return Ok(Some(Definition::Reference(r.clone())));
        }
        let Some(source) = &self.source else {
            return Ok(None);
        };
        if let Some(url) = GithubUrl::parse(source) {
            return github::download(&url).await.map(Some);
        }
        let path = PathBuf::from(source);
        let meta = std::fs::metadata(&path).map_err(|e| format!("cannot read {source}: {e}"))?;
        if meta.is_dir() {
            Ok(Some(Definition::Bundle {
                files: read_assembled_folder(&path)?,
                origin: None,
            }))
        } else {
            Ok(Some(Definition::Bundle {
                files: read_lone_file(&path)?,
                origin: None,
            }))
        }
    }
}

/// Read a definition folder on disk and assemble what it includes from the
/// repository it sits in.
// spec: CLI#include-assembly
pub(super) fn read_assembled_folder(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = read_folder(path)?;
    let folder = path
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    assemble(
        &mut files,
        &folder,
        &boundary_of(&folder)?,
        &Unwritten::new(),
    )?;
    Ok(files)
}

/// Read a lone file as a bundle holding it under its own name, with a
/// metadata file naming it as the only script file.
// spec: CLI#definition-sources
pub(super) fn read_lone_file(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
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
    let contents =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let metadata = toml::Table::from_iter([(
        "script".to_owned(),
        toml::Value::Array(vec![toml::Value::String(name.to_owned())]),
    )]);
    Ok(BTreeMap::from([
        (name.to_owned(), contents),
        (METADATA_FILE.to_owned(), metadata.to_string().into_bytes()),
    ]))
}

#[derive(clap::Subcommand, Debug)]
pub(super) enum DefinitionsCommand {
    /// Write a definition as the layer of a definition artefact, for pushing
    /// to a registry with an OCI tool. Prints the version annotation to give
    /// the artefact, where the definition declares one
    Pack {
        /// A script file, a definition folder, or a GitHub URL naming a folder
        /// at a branch, tag, or commit
        source: String,
        /// The gzipped tar to write, which must not exist yet
        file: PathBuf,
    },
}

pub(super) async fn dispatch(command: &DefinitionsCommand) {
    match command {
        DefinitionsCommand::Pack { source, file } => match pack(source, file).await {
            Ok(Some(requirement)) => println!("{VERSIONS_ANNOTATION}={requirement}"),
            Ok(None) => {}
            Err(e) => {
                tracing::error!("{e}");
                std::process::exit(1);
            }
        },
    }
}

/// Read and assemble `source` as a push would, hold it to the bundle rules,
/// and write it to `out` as a definition layer. Returns the bundle's declared
/// Seedling version requirement.
// spec: CLI#definition-packing
pub(super) async fn pack(source: &str, out: &Path) -> Result<Option<String>, String> {
    if out.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", out.display()));
    }
    let args = DefinitionArgs {
        source: Some(source.to_owned()),
        reference: None,
    };
    let Some(Definition::Bundle { files, .. }) = args.resolve().await? else {
        return Err(format!("{source} is not a definition source to pack"));
    };
    let bundle = Bundle::from_files(files).map_err(|e| e.to_string())?;
    bundle.script().map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    file.write_all(&bundle.to_tar_gz())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(bundle.seedling_versions().map(|r| r.to_string()))
}

/// Resolve `args` or exit with the error.
pub(super) async fn resolve_or_exit(args: &DefinitionArgs) -> Option<Definition> {
    args.resolve().await.unwrap_or_else(|e| {
        tracing::error!("{e}");
        std::process::exit(1);
    })
}

/// Directories version-control systems keep their metadata in.
const VCS_METADATA_DIRS: &[&str] = &[
    ".git", ".jj", ".hg", ".sl", ".svn", ".bzr", ".pijul", "_darcs", "CVS",
];

/// Whether `entry` is version-control metadata: one of the directories above,
/// or the `.git` file a linked work tree keeps in its place.
fn is_vcs_metadata(entry: &ignore::DirEntry) -> bool {
    let Some(name) = entry.file_name().to_str() else {
        return false;
    };
    name == ".git"
        || (entry.file_type().is_some_and(|t| t.is_dir()) && VCS_METADATA_DIRS.contains(&name))
}

/// Read a definition folder: every regular file, leaving out version-control
/// metadata and what `.seedignore` excludes. Anything else that is not left
/// out is an error, so nothing is sent.
// spec: CLI#definition-folders
pub(super) fn read_folder(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .add_custom_ignore_filename(".seedignore")
        .filter_entry(|e| !is_vcs_metadata(e))
        .build();
    let mut files = BTreeMap::new();
    for entry in walker {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", root.display()))?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("walked paths are beneath the root");
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            continue;
        }
        if !file_type.is_file() {
            return Err(format!(
                "{} is not a regular file; definitions may only hold files and folders",
                path.display()
            ));
        }
        let rel = rel
            .to_str()
            .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))?
            .to_owned();
        let contents =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        files.insert(rel, contents);
    }
    Ok(files)
}

/// A GitHub URL naming a folder: `https://github.com/<owner>/<repo>/tree/<rest>`,
/// where `<rest>` is a ref followed by a path and the split between them is
/// only known once the ref is resolved.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GithubUrl {
    pub url: String,
    pub owner: String,
    pub repo: String,
    pub rest: Vec<String>,
}

impl GithubUrl {
    pub fn parse(raw: &str) -> Option<Self> {
        let path = raw
            .strip_prefix("https://github.com/")
            .or_else(|| raw.strip_prefix("http://github.com/"))?;
        let mut parts = path.trim_end_matches('/').split('/');
        let owner = parts.next()?.to_owned();
        let repo = parts.next()?.to_owned();
        if parts.next()? != "tree" {
            return None;
        }
        let rest: Vec<String> = parts.map(str::to_owned).collect();
        if rest.is_empty() || owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some(Self {
            url: raw.to_owned(),
            owner,
            repo,
            rest,
        })
    }

    /// Each way of splitting the rest into a ref and a folder, longest ref
    /// last, the order GitHub itself tries them in being unknowable here.
    pub fn splits(&self) -> impl Iterator<Item = (String, String)> + '_ {
        (1..=self.rest.len()).map(|n| (self.rest[..n].join("/"), self.rest[n..].join("/")))
    }
}

mod github {
    use std::io::Read;

    use super::{Definition, GithubUrl, Unwritten, assemble, read_folder};

    fn client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(concat!("seedling-ctl/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("cannot build an HTTP client: {e}"))
    }

    fn authorise(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match std::env::var("GITHUB_TOKEN") {
            Ok(token) if !token.is_empty() => req.bearer_auth(token),
            _ => req,
        }
    }

    /// Resolve the URL's ref to a commit, download that commit, and read the
    /// folder out of it, recording the URL and commit as the origin.
    // spec: CLI#definition-sources
    pub(super) async fn download(url: &GithubUrl) -> Result<Definition, String> {
        let client = client()?;
        let mut resolved = None;
        for (git_ref, folder) in url.splits() {
            let api = format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                url.owner, url.repo, git_ref
            );
            let resp = authorise(client.get(&api))
                .header("Accept", "application/vnd.github.sha")
                .send()
                .await
                .map_err(|e| format!("cannot reach GitHub: {e}"))?;
            if resp.status().is_success() {
                let sha = resp
                    .text()
                    .await
                    .map_err(|e| format!("cannot read GitHub's response: {e}"))?;
                resolved = Some((sha.trim().to_owned(), folder));
                break;
            }
            if resp.status() != reqwest::StatusCode::NOT_FOUND
                && resp.status() != reqwest::StatusCode::UNPROCESSABLE_ENTITY
            {
                return Err(format!("GitHub refused {api}: {}", resp.status()));
            }
        }
        let (sha, folder) =
            resolved.ok_or_else(|| format!("{} names no branch, tag, or commit", url.url))?;

        let tarball = format!(
            "https://api.github.com/repos/{}/{}/tarball/{sha}",
            url.owner, url.repo
        );
        let resp = authorise(client.get(&tarball))
            .send()
            .await
            .map_err(|e| format!("cannot download {tarball}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GitHub refused {tarball}: {}", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("cannot download {tarball}: {e}"))?;

        let dir =
            tempfile::tempdir().map_err(|e| format!("cannot make a temporary folder: {e}"))?;
        let unwritten = extract_repository(&bytes, dir.path())?;
        let files = read_repository_folder(dir.path(), &folder, &unwritten)?;
        if files.is_empty() {
            return Err(format!("{} holds no files", url.url));
        }
        Ok(Definition::Bundle {
            files,
            origin: Some((url.url.clone(), sha)),
        })
    }

    /// Read `folder` out of an extracted repository and assemble what it
    /// includes from the rest of it.
    // spec: CLI#definition-sources
    // spec: CLI#include-assembly
    pub(super) fn read_repository_folder(
        root: &std::path::Path,
        folder: &str,
        unwritten: &Unwritten,
    ) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
        let beneath = format!("{folder}/");
        if let Some(special) = unwritten.iter().find(|u| {
            folder.is_empty()
                || *u == folder
                || u.starts_with(&beneath)
                || folder.starts_with(&format!("{u}/"))
        }) {
            return Err(format!(
                "{special} is not a regular file; definitions may only hold files and folders"
            ));
        }
        let path = root.join(folder);
        if !path.is_dir() {
            return Ok(Default::default());
        }
        let mut files = read_folder(&path)?;
        assemble(&mut files, &path, root, unwritten)?;
        Ok(files)
    }

    /// Unpack a GitHub tarball into `dest`, dropping the one top-level
    /// directory GitHub prefixes every entry with.
    ///
    /// An entry that is neither a file nor a directory is recorded rather
    /// than written: a symlink written out would let a later entry be written
    /// through it, and it only matters if the definition reads it. They are
    /// all recorded before anything is written, so an entry beneath one is
    /// refused whichever comes first in the archive.
    // spec: CLI#definition-sources
    pub(super) fn extract_repository(
        tarball: &[u8],
        dest: &std::path::Path,
    ) -> Result<Unwritten, String> {
        let bad = |e: std::io::Error| format!("the downloaded archive is unreadable: {e}");
        let archive = || tar::Archive::new(flate2::read::GzDecoder::new(tarball));

        let mut unwritten = Unwritten::new();
        for entry in archive().entries().map_err(bad)? {
            let entry = entry.map_err(bad)?;
            let Some(name) = entry_name(&entry.path().map_err(bad)?)? else {
                continue;
            };
            if !matches!(
                entry.header().entry_type(),
                tar::EntryType::Directory
                    | tar::EntryType::Regular
                    | tar::EntryType::Continuous
                    | tar::EntryType::XGlobalHeader
                    | tar::EntryType::XHeader
            ) {
                unwritten.insert(name);
            }
        }

        let mut archive = archive();
        for entry in archive.entries().map_err(bad)? {
            let mut entry = entry.map_err(bad)?;
            let Some(name) = entry_name(&entry.path().map_err(bad)?)? else {
                continue;
            };
            if let Some(special) =
                std::iter::successors(name.rsplit_once('/'), |(parent, _)| parent.rsplit_once('/'))
                    .map(|(parent, _)| parent)
                    .find(|p| unwritten.contains(*p))
            {
                return Err(format!(
                    "the downloaded archive holds {name} beneath {special}, which is not a directory"
                ));
            }
            let target = dest.join(&name);
            match entry.header().entry_type() {
                tar::EntryType::Directory => {
                    std::fs::create_dir_all(&target).map_err(bad)?;
                }
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(bad)?;
                    }
                    let mut contents = Vec::new();
                    entry.read_to_end(&mut contents).map_err(bad)?;
                    std::fs::write(&target, contents).map_err(bad)?;
                }
                _ => {}
            }
        }
        Ok(unwritten)
    }

    /// An archive entry's path within the repository, `/`-separated, or
    /// `None` for GitHub's prefix directory itself.
    fn entry_name(path: &std::path::Path) -> Result<Option<String>, String> {
        // Every entry is written beneath the extraction root, so nothing but
        // plain names may reach the join: an archive is not a trusted source
        // of paths, whatever produced it. The prefix directory is checked
        // too, so that a leading `..` is refused rather than dropped.
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "the downloaded archive holds an entry outside the repository: {}",
                path.display()
            ));
        }
        let mut components = path.components();
        components.next();
        let rel = components.as_path();
        if rel.as_os_str().is_empty() {
            return Ok(None);
        }
        rel.to_str().map(|s| Some(s.to_owned())).ok_or_else(|| {
            format!(
                "the downloaded archive holds a non-UTF-8 path: {}",
                rel.display()
            )
        })
    }
}

/// The `params` object of a parameter change: each `name=value` of `set`
/// sets a parameter to the string `value`, and each name of `unset` unsets one.
///
/// Refuses a `set` argument without `=` and a parameter changed more than
/// once, whether by repeating it or by both setting and unsetting it. Whether
/// anything is changed is for the caller to decide.
// spec: CLI#parameter-changes
pub(super) fn param_changes(
    set: &[String],
    unset: &[String],
) -> Result<Map<String, Value>, String> {
    let mut changes = Map::new();
    let mut add = |name: &str, value: Value| match changes.insert(name.to_owned(), value) {
        Some(_) => Err(format!("parameter {name:?} is changed more than once")),
        None => Ok(()),
    };
    for pair in set {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("a parameter to set takes name=value, got {pair:?}"))?;
        add(name, json!(value))?;
    }
    for name in unset {
        add(name, Value::Null)?;
    }
    Ok(changes)
}

/// Write a bundle from `/apps/bundle` into a new folder.
// spec: CLI#definition-export
pub(super) fn export(bundle: &Map<String, Value>, folder: &Path) -> Result<(), String> {
    if folder.exists() {
        let mut entries = std::fs::read_dir(folder)
            .map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "{} already exists and is not empty",
                folder.display()
            ));
        }
    }
    let mut decoded = Vec::with_capacity(bundle.len());
    for (path, value) in bundle {
        let encoded = value
            .as_str()
            .ok_or_else(|| format!("the daemon sent {path:?} in an unexpected form"))?;
        let contents = BASE64
            .decode(encoded)
            .map_err(|e| format!("the daemon sent {path:?} undecodable: {e}"))?;
        // Paths come from a bundle the daemon validated, but they are
        // joined onto a local folder, so they are checked again here.
        if path.starts_with('/') || path.split('/').any(|s| s == ".." || s.is_empty()) {
            return Err(format!("the daemon sent an unsafe path {path:?}"));
        }
        decoded.push((folder.join(path), contents));
    }
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    for (target, contents) in decoded {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&target, contents)
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
