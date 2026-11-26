//! Reading an app definition from wherever the operator points at it, and
//! turning it into the fields a definition request carries.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use seedling_bundle::{
    Bundle, VERSIONS_ANNOTATION,
    source::{read_folder_definition, read_lone_file},
};
use serde_json::{Map, Value, json};

use github::GithubUrl;

mod check;
mod github;

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
            let (assembled, origin) = github::download(&url, None)
                .await
                .map_err(|e| e.to_string())?;
            (assembled.files, Some(origin))
        } else {
            let path = PathBuf::from(source);
            let meta =
                std::fs::metadata(&path).map_err(|e| format!("cannot read {source}: {e}"))?;
            let files = if meta.is_dir() {
                read_folder_definition(&path, None)
                    .map_err(|e| e.to_string())?
                    .files
            } else {
                read_lone_file(&path).map_err(|e| e.to_string())?
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
    /// Check a definition without a server: its Seedling version
    /// requirement, the bundle rules, its evaluation, and any sample
    /// parameter values. Exits 1 when a check fails and 2 on a bad option
    Check(check::CheckArgs),
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
        DefinitionsCommand::Check(args) => std::process::exit(check::run(args).await),
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
