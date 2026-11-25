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

use assembly::{Bounds, assemble, relative, repository_of, source_paths};
use tree::{Budget, Disk, read_file, read_folder};

mod assembly;
mod tree;

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
        bundle: Box<Bundle>,
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
            Self::Bundle { bundle, origin } => {
                out.insert(keys.bundle.to_owned(), Value::Object(bundle.to_wire()));
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
        let (files, origin) = if let Some(url) = GithubUrl::parse(source) {
            let (files, origin) = github::download(&url).await?;
            (files, Some(origin))
        } else {
            let path = PathBuf::from(source);
            let meta =
                std::fs::metadata(&path).map_err(|e| format!("cannot read {source}: {e}"))?;
            let files = if meta.is_dir() {
                read_assembled_folder(&path)?
            } else {
                read_lone_file(&path)?
            };
            (files, None)
        };
        // Only the rules a server holds every stored definition to, as it
        // applies them: the metadata fields and script files are judged by
        // the Seedling a definition is installed on, since a template may be
        // stored for a later Seedling whose metadata this CLI cannot read.
        let bundle = Bundle::from_files(files).map_err(|e| e.to_string())?;
        Ok(Some(Definition::Bundle {
            bundle: Box::new(bundle),
            origin,
        }))
    }
}

/// Read a definition folder on disk and assemble what it includes from the
/// repository it sits in.
// spec: CLI#include-assembly
pub(super) fn read_assembled_folder(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
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
    let Some(Definition::Bundle { bundle, .. }) = args.resolve().await? else {
        return Err(format!("{source} is not a definition source to pack"));
    };
    bundle.script().map_err(|e| e.to_string())?;
    // Written beside `out` and moved into place whole, so a failed write
    // leaves nothing for the next attempt to refuse as already existing.
    let dir = match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut staged = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("cannot create a file beside {}: {e}", out.display()))?;
    staged
        .write_all(&bundle.to_tar_gz())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    staged
        .persist_noclobber(out)
        .map_err(|e| format!("cannot create {}: {}", out.display(), e.error))?;
    Ok(bundle.seedling_versions().map(|r| r.to_string()))
}

/// Resolve `args` or exit with the error.
pub(super) async fn resolve_or_exit(args: &DefinitionArgs) -> Option<Definition> {
    args.resolve().await.unwrap_or_else(|e| {
        tracing::error!("{e}");
        std::process::exit(1);
    })
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
    use std::collections::BTreeMap;

    use super::{
        Bounds, Budget, GithubUrl, assemble, read_folder, source_paths,
        tree::{Archive, Kind, locate},
    };

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
    /// folder out of it, with the URL and commit to record as its origin.
    // spec: CLI#definition-sources
    pub(super) async fn download(
        url: &GithubUrl,
    ) -> Result<(BTreeMap<String, Vec<u8>>, (String, String)), String> {
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

        let files = read_definition(&bytes, &folder)?;
        if files.is_empty() {
            return Err(format!("{} holds no files", url.url));
        }
        Ok((files, (url.url.clone(), sha)))
    }

    /// Read the definition at `folder` out of a GitHub tarball, with what
    /// its include table names from elsewhere in the repository.
    ///
    /// The repository is read in memory, and only the folder and its sources
    /// are held, so a repository far larger than any definition costs no more
    /// than the definition does.
    // spec: CLI#definition-sources
    // spec: CLI#include-assembly
    pub(super) fn read_definition(
        tarball: &[u8],
        folder: &str,
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        // The folder comes from the URL, so it is held to the repository as
        // an archive entry is.
        let folder = seedling_bundle::normalise_path(folder)
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
