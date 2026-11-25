//! Reading an app definition from wherever the operator points at it, and
//! turning it into the fields a definition request carries.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use seedling_bundle::{
    BUNDLE_FILE_LIMIT, BUNDLE_SIZE_LIMIT, Bundle, METADATA_FILE, VERSIONS_ANNOTATION,
};
use serde_json::{Map, Value, json};

use assembly::{Bounds, Unwritten, assemble, repository_of, source_paths};

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
    let mut budget = Budget::full();
    let mut files = read_folder(path, &mut budget)?;
    let folder = path
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let repository = repository_of(&folder)?;
    let (boundary, bounds) = match &repository {
        Some(root) => (root.as_path(), Bounds::Repository(root)),
        None => (folder.as_path(), Bounds::Folder),
    };
    assemble(
        &mut files,
        &folder,
        boundary,
        bounds,
        &Unwritten::default(),
        &mut budget,
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
    let contents = read_bounded(path, &mut Budget::full())?;
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
pub(super) fn read_folder(
    root: &Path,
    budget: &mut Budget,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
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
        budget.take_file(path, rel.len())?;
        let contents = read_bounded(path, budget)?;
        files.insert(rel, contents);
    }
    Ok(files)
}

/// What is left of the bundle limits while a definition is read, shared by
/// the folder and every source it includes.
///
/// Reading stops once a definition could no longer make a bundle a server
/// holds, so a folder far larger than any bundle, such as a whole repository
/// named by mistake, is refused before it is held in memory. The count is a
/// bound, not the rule: the bundle rules judge the assembled files.
#[derive(Debug, Clone, Copy)]
pub(super) struct Budget {
    files: usize,
    bytes: usize,
}

impl Budget {
    pub fn full() -> Self {
        Self {
            files: BUNDLE_FILE_LIMIT,
            bytes: BUNDLE_SIZE_LIMIT,
        }
    }

    /// Take one file, with a path `path_len` bytes long, from the budget.
    pub fn take_file(&mut self, path: &Path, path_len: usize) -> Result<(), String> {
        self.files = self.files.checked_sub(1).ok_or_else(|| {
            format!(
                "{} takes the definition past the {BUNDLE_FILE_LIMIT} files it may hold",
                path.display()
            )
        })?;
        self.take_bytes(path, path_len)
    }

    fn take_bytes(&mut self, path: &Path, len: usize) -> Result<(), String> {
        self.bytes = self.bytes.checked_sub(len).ok_or_else(|| {
            format!(
                "{} takes the definition past its {BUNDLE_SIZE_LIMIT}-byte size limit",
                path.display()
            )
        })?;
        Ok(())
    }

    /// Read `from` to its end, taking what it holds from the budget and
    /// refusing it as soon as it passes what is left.
    fn read(&mut self, path: &Path, from: impl Read) -> Result<Vec<u8>, String> {
        let mut contents = Vec::new();
        from.take(self.bytes as u64 + 1)
            .read_to_end(&mut contents)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        self.take_bytes(path, contents.len())?;
        Ok(contents)
    }

    /// Return files already taken, as when files read from a source turn
    /// out to be the ones a folder already holds.
    pub fn give_back<'a>(&mut self, files: impl Iterator<Item = (&'a String, &'a Vec<u8>)>) {
        for (path, contents) in files {
            self.files += 1;
            self.bytes += path.len() + contents.len();
        }
    }
}

/// Read a file, taking its contents from `budget`, and refusing it as soon as
/// it passes what is left.
pub(super) fn read_bounded(path: &Path, budget: &mut Budget) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    budget.read(path, file)
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

    use seedling_bundle::METADATA_FILE;

    use super::{
        Bounds, Budget, GithubUrl, Unwritten, assemble, read_bounded, read_folder, source_paths,
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
    ) -> Result<
        (
            std::collections::BTreeMap<String, Vec<u8>>,
            (String, String),
        ),
        String,
    > {
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
        let files = extract_definition(&bytes, dir.path(), &folder)?;
        if files.is_empty() {
            return Err(format!("{} holds no files", url.url));
        }
        Ok((files, (url.url.clone(), sha)))
    }

    /// Read the definition at `folder` out of a GitHub tarball, unpacking it
    /// into `root` with the sources its include table names, and assemble it.
    ///
    /// Only the folder and its sources are written out, so a repository far
    /// larger than any definition costs no more than the definition does.
    // spec: CLI#definition-sources
    // spec: CLI#include-assembly
    pub(super) fn extract_definition(
        tarball: &[u8],
        root: &std::path::Path,
        folder: &str,
    ) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
        // The folder comes from the URL, and is joined onto the extraction
        // root, so it is held to the repository as an archive entry is.
        let folder = seedling_bundle::normalise_path(folder)
            .map_err(|e| format!("the URL's folder lies outside the repository: {e}"))?;
        let unwritten = record_links(tarball)?;
        extract(tarball, root, &unwritten, std::slice::from_ref(&folder))?;
        let metadata = root.join(&folder).join(METADATA_FILE);
        let sources = match metadata.symlink_metadata() {
            Ok(meta) if meta.is_file() => {
                source_paths(&folder, &read_bounded(&metadata, &mut Budget::full())?)?
            }
            _ => Vec::new(),
        };
        extract(tarball, root, &unwritten, &sources)?;
        read_repository_folder(root, &folder, &unwritten)
    }

    /// Read `folder` out of an extracted repository and assemble what it
    /// includes from the rest of it.
    fn read_repository_folder(
        root: &std::path::Path,
        folder: &str,
        unwritten: &Unwritten,
    ) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
        if let Some(special) = unwritten
            .reached_by(folder)
            .or_else(|| unwritten.within(folder))
        {
            return Err(format!(
                "{special} is not a regular file; definitions may only hold files and folders"
            ));
        }
        let path = root.join(folder);
        if !path.is_dir() {
            return Ok(Default::default());
        }
        let mut budget = Budget::full();
        let mut files = read_folder(&path, &mut budget)?;
        assemble(
            &mut files,
            &path,
            root,
            Bounds::Archive,
            unwritten,
            &mut budget,
        )?;
        Ok(files)
    }

    /// Record every entry of a GitHub tarball that is neither a file nor a
    /// directory, checking every entry's path on the way.
    ///
    /// Such an entry is never written: a symlink written out would let a
    /// later entry be written through it, and it only matters if the
    /// definition reads it. They are all recorded before anything is written,
    /// so an entry at or beneath one is refused whichever comes first.
    fn record_links(tarball: &[u8]) -> Result<Unwritten, String> {
        let mut unwritten = Unwritten::default();
        for entry in archive(tarball).entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let Some(name) = entry_name(&entry.path().map_err(unreadable)?)? else {
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
                unwritten.record(name);
            }
        }
        Ok(unwritten)
    }

    /// Unpack the files and directories of a GitHub tarball at or beneath
    /// each of `wanted` into `dest`, dropping the one top-level directory
    /// GitHub prefixes every entry with. What is written is held to the
    /// bundle limits.
    // spec: CLI#definition-sources
    fn extract(
        tarball: &[u8],
        dest: &std::path::Path,
        unwritten: &Unwritten,
        wanted: &[String],
    ) -> Result<(), String> {
        let mut budget = Budget::full();
        for entry in archive(tarball).entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let Some(name) = entry_name(&entry.path().map_err(unreadable)?)? else {
                continue;
            };
            let kind = entry.header().entry_type();
            let written = matches!(
                kind,
                tar::EntryType::Directory | tar::EntryType::Regular | tar::EntryType::Continuous
            );
            if let Some(special) = unwritten.reached_by(&name).filter(|_| written) {
                return Err(format!(
                    "the downloaded archive holds {name} at or beneath {special}, which is not a file or directory"
                ));
            }
            if !wanted
                .iter()
                .any(|w| w.is_empty() || name == *w || name.starts_with(&format!("{w}/")))
            {
                continue;
            }
            let target = dest.join(&name);
            match kind {
                tar::EntryType::Directory => {
                    std::fs::create_dir_all(&target).map_err(unreadable)?;
                }
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(unreadable)?;
                    }
                    let at = std::path::Path::new(&name);
                    budget.take_file(at, name.len())?;
                    let contents = budget.read(at, entry)?;
                    std::fs::write(&target, contents).map_err(unreadable)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn archive(tarball: &[u8]) -> tar::Archive<flate2::read::GzDecoder<&[u8]>> {
        tar::Archive::new(flate2::read::GzDecoder::new(tarball))
    }

    fn unreadable(e: std::io::Error) -> String {
        format!("the downloaded archive is unreadable: {e}")
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
