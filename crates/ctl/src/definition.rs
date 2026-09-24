//! Reading an app definition from wherever the operator points at it, and
//! turning it into the fields a definition request carries.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Map, Value, json};

/// Where a definition comes from.
// i[impl ctl.definition.source]
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
    Script(String),
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
// i[impl ctl.definition.source]
#[derive(Clone, Copy)]
pub(super) struct DefinitionKeys {
    pub script: &'static str,
    pub bundle: &'static str,
    pub reference: &'static str,
    /// `None` where the request has nowhere to report an origin.
    pub origin: Option<&'static str>,
}

impl DefinitionKeys {
    /// `/apps/create` and `/apps/update`.
    pub const APP: Self = Self {
        script: "script",
        bundle: "bundle",
        reference: "reference",
        origin: Some("origin"),
    };
    /// `/templates/create` and `/templates/update`.
    pub const TEMPLATE: Self = Self {
        script: "body",
        bundle: "bundle",
        reference: "reference",
        origin: Some("origin"),
    };
    /// The definition `/apps/plan` is asked to compare against, which it
    /// never installs and so records no origin for.
    pub const PROPOSED: Self = Self {
        script: "proposed_script",
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
            Self::Script(text) => {
                out.insert(keys.script.to_owned(), json!(text));
            }
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

    /// The script text, when it can be seen without the daemon.
    pub fn script_text(&self) -> Option<String> {
        match self {
            Self::Script(text) => Some(text.clone()),
            Self::Bundle { files, .. } => Some(
                files
                    .values()
                    .filter_map(|c| std::str::from_utf8(c).ok())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Self::Reference(_) => None,
        }
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
                files: read_folder(&path)?,
                origin: None,
            }))
        } else {
            std::fs::read_to_string(&path)
                .map(|t| Some(Definition::Script(t)))
                .map_err(|e| format!("cannot read {source}: {e}"))
        }
    }
}

/// Resolve `args` or exit with the error.
pub(super) async fn resolve_or_exit(args: &DefinitionArgs) -> Option<Definition> {
    args.resolve().await.unwrap_or_else(|e| {
        tracing::error!("{e}");
        std::process::exit(1);
    })
}

/// Read a definition folder: every regular file, leaving out version-control
/// metadata and what `.seedignore` excludes. Anything else that is not left
/// out is an error, so nothing is sent.
// i[impl ctl.definition.folder]
pub(super) fn read_folder(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .add_custom_ignore_filename(".seedignore")
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".jj")))
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

    use super::{Definition, GithubUrl, read_folder};

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
    // i[impl ctl.definition.source]
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
        extract_folder(&bytes, &folder, dir.path())?;
        let files = read_folder(dir.path())?;
        if files.is_empty() {
            return Err(format!("{} holds no files", url.url));
        }
        Ok(Definition::Bundle {
            files,
            origin: Some((url.url.clone(), sha)),
        })
    }

    /// Unpack the entries of a GitHub tarball beneath `folder` into `dest`.
    /// GitHub prefixes every entry with one top-level directory, which is
    /// dropped.
    pub(super) fn extract_folder(
        tarball: &[u8],
        folder: &str,
        dest: &std::path::Path,
    ) -> Result<(), String> {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
        let bad = |e: std::io::Error| format!("the downloaded archive is unreadable: {e}");
        for entry in archive.entries().map_err(bad)? {
            let mut entry = entry.map_err(bad)?;
            let path = entry.path().map_err(bad)?.into_owned();
            let mut components = path.components();
            components.next();
            let inner = components.as_path();
            let Ok(rel) = inner.strip_prefix(folder) else {
                continue;
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            // Every entry is written beneath `dest`, so nothing but plain
            // names may reach the join: an archive is not a trusted source
            // of paths, whatever produced it.
            if rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(format!(
                    "the downloaded archive holds an entry outside the folder: {}",
                    rel.display()
                ));
            }
            let target = dest.join(rel);
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
                tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => {}
                // A symlink names its own target and a hardlink resolves one
                // relative to where it lands, so neither is reproduced: the
                // folder rules refuse them anyway, and this reports them
                // without ever writing one.
                other => {
                    return Err(format!(
                        "{} is not a regular file; definitions may only hold files and folders ({other:?})",
                        rel.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The parameter change `ctl apps update` carries alongside a definition.
// i[impl ctl.definition.param]
pub(super) fn param_change(
    set: Option<&str>,
    unset: Option<&str>,
) -> Result<Option<Value>, String> {
    match (set, unset) {
        (Some(_), Some(_)) => Err("--set and --unset cannot be combined".to_owned()),
        (Some(pair), None) => {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("--set takes name=value, got {pair:?}"))?;
            Ok(Some(json!({ "name": name, "value": value })))
        }
        (None, Some(name)) => Ok(Some(json!({ "name": name, "value": Value::Null }))),
        (None, None) => Ok(None),
    }
}

/// Write a bundle from `/apps/bundle` into a new folder.
// i[impl ctl.definition.export]
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
