//! App definition bundles: the bundle rules, the content hash, and the forms
//! a bundle travels in.

use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    ops::Bound,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use seedling_protocol::error::{ErrorCode, OiError};
use semver::Version;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use version::VersionRequirement;

#[cfg(feature = "source")]
pub mod source;
mod version;

/// Upper bound on a bundle's total file size.
///
/// Pushed bundles arrive base64-encoded inside one control-stream request, and
/// `/apps/bundle` returns them the same way, so the cap has to leave the
/// encoded form inside the 4 MiB request and response limits.
pub const BUNDLE_SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// Upper bound on the number of files in a bundle.
///
/// The size limit alone does not bound intake: a pushed map of empty files,
/// or a tar of empty-file headers, costs almost nothing towards it while
/// still making the runtime hold and walk one entry each.
pub const BUNDLE_FILE_LIMIT: usize = 4096;

pub const METADATA_FILE: &str = "seedling.toml";

/// The artifact type of a definition artefact, an OCI manifest holding a
/// bundle as its single layer.
// spec: DEFN#fetching
pub const ARTIFACT_TYPE: &str = "application/vnd.bes.seedling.definition.v1";

/// The media type of a definition artefact's layer, the form
/// [`Bundle::to_tar_gz`] writes and [`Bundle::from_tar_gz`] reads.
// spec: DEFN#fetching
pub const LAYER_MEDIA_TYPE: &str = "application/vnd.bes.seedling.definition.v1.tar+gzip";

/// The annotation carrying a definition's Seedling version requirement on its
/// manifest or index entry.
// spec: DEFN#fetching
pub const VERSIONS_ANNOTATION: &str = "vnd.bes.seedling.versions";
pub const DEFAULT_SCRIPT: &str = "app.seed.rhai";

/// Why a bundle cannot be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    // spec: DEFN#bundle-rules
    Invalid {
        message: String,
        /// Where in the bundle the broken rule lies, where it lies in one
        /// place.
        place: Option<Place>,
    },
    // spec: DEFN#seedling-version-support
    Unsupported {
        requirement: String,
        running: Version,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { message, .. } => f.write_str(message),
            Self::Unsupported {
                requirement,
                running,
            } => write!(
                f,
                "definition requires Seedling {requirement}, but this is Seedling {running}"
            ),
        }
    }
}

impl std::error::Error for BundleError {}

impl From<BundleError> for OiError {
    fn from(e: BundleError) -> Self {
        let code = match &e {
            BundleError::Invalid { .. } => ErrorCode::BundleInvalid,
            BundleError::Unsupported { .. } => ErrorCode::UnsupportedSeedling,
        };
        OiError::new(code, e.to_string())
    }
}

/// Where in a bundle a broken rule lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// The metadata file, at this line where the error has one.
    MetadataFile { line: Option<usize> },
    /// The file at this bundle path.
    Path(String),
}

fn invalid(msg: impl Into<String>) -> BundleError {
    BundleError::Invalid {
        message: msg.into(),
        place: None,
    }
}

fn in_metadata(msg: impl Into<String>) -> BundleError {
    BundleError::Invalid {
        message: msg.into(),
        place: Some(Place::MetadataFile { line: None }),
    }
}

fn at_path(path: &str, msg: impl Into<String>) -> BundleError {
    BundleError::Invalid {
        message: msg.into(),
        place: Some(Place::Path(path.to_owned())),
    }
}

/// Normalise a bundle path: relative, `/`-separated, `.` and `..` resolved
/// lexically, without touching any filesystem.
// spec: DEFN#bundle-rules
// spec: BND#bundle-files
pub fn normalise_path(raw: &str) -> Result<String, String> {
    if raw.contains('\0') {
        return Err(format!("path {raw:?} contains a null byte"));
    }
    if raw.starts_with('/') {
        return Err(format!("path {raw:?} is absolute"));
    }
    let mut parts: Vec<&str> = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("path {raw:?} escapes the bundle root"));
                }
            }
            other => parts.push(other),
        }
    }
    Ok(parts.join("/"))
}

/// The script files' contents joined in order, with enough bookkeeping to
/// report an error's position in the file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    text: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    file: String,
    /// First line of this file within the concatenation, 1-based.
    first_line: usize,
    lines: usize,
}

impl Script {
    // spec: BND#script-formation
    fn concatenate<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut text = String::new();
        let mut segments = Vec::new();
        let mut next_line = 1;
        for (file, contents) in files {
            // Each file starts on a line of its own, so the line table stays
            // exact; a lone script is returned exactly as written.
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            // An empty file contributes no text and no separator, so it
            // takes no lines: charging it one would shift every file after
            // it out of step with the concatenation.
            let lines = contents.lines().count();
            text.push_str(contents);
            segments.push(Segment {
                file: file.to_owned(),
                first_line: next_line,
                lines,
            });
            next_line += lines;
        }
        Self { text, segments }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.segments.iter().map(|s| s.file.as_str())
    }

    /// The file and in-file line of a line of the concatenation.
    pub fn locate(&self, line: usize) -> Option<(&str, usize)> {
        self.segments
            .iter()
            .find(|s| line >= s.first_line && line < s.first_line + s.lines)
            .map(|s| (s.file.as_str(), line - s.first_line + 1))
    }

    /// Rewrite every `line N, position M` in an evaluation error to name the
    /// script file and the line within it.
    ///
    /// Rhai reports positions against the text it was given, and nests them
    /// for errors raised inside called functions, so the rewrite applies to
    /// each occurrence rather than only the outermost.
    // spec: BND#script-formation
    pub fn remap_error(&self, message: &str) -> String {
        const NEEDLE: &str = "line ";
        let mut out = String::with_capacity(message.len());
        let mut rest = message;
        while let Some(idx) = rest.find(NEEDLE) {
            out.push_str(&rest[..idx]);
            let after = &rest[idx + NEEDLE.len()..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            let located = (digits > 0)
                .then(|| after[..digits].parse::<usize>().ok())
                .flatten()
                .filter(|_| after[digits..].starts_with(", position "))
                .and_then(|line| self.locate(line));
            match located {
                Some((file, line)) => {
                    out.push_str(file);
                    out.push_str(" line ");
                    out.push_str(&line.to_string());
                    rest = &after[digits..];
                }
                None => {
                    out.push_str(NEEDLE);
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// An app definition bundle: a tree of files, validated against the bundle
/// rules, with its content hash and declared Seedling version requirement.
///
/// The requirement is read on its own before the rest of the metadata, so a
/// bundle written for a newer Seedling can still be held and reported as
/// unsupported; the strict reading of everything else lives in `script`.
/// The metadata table is read on its own too, so that a field or script file
/// this Seedling cannot read does not hide a table it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    files: BTreeMap<String, Bytes>,
    hash: String,
    seedling: Option<VersionRequirement>,
    metadata: Result<Map<String, Value>, BundleError>,
    script: Result<Script, BundleError>,
}

/// What a stored bundle's metadata file declares, read without rebuilding
/// the bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declared {
    pub seedling_versions: Option<VersionRequirement>,
    pub metadata: Map<String, Value>,
}

impl Default for Bundle {
    fn default() -> Self {
        Self::empty()
    }
}

impl Bundle {
    /// Validate a set of files as a bundle.
    ///
    /// Only the rules that make a bundle unholdable fail here: size, paths,
    /// and a `seedling` field that cannot be read. The rest of the metadata
    /// and the script files are checked by [`Self::script`], since a bundle
    /// for a newer Seedling may define fields this one does not know.
    // spec: DEFN#bundle-rules
    pub fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, BundleError> {
        Self::build(files, true)
    }

    /// Rebuild a bundle the runtime already holds. The size limit governs
    /// what may be installed, not what was installed under an earlier limit,
    /// so it is not reapplied: a stored definition must always load.
    fn from_stored(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, BundleError> {
        Self::build(files, false)
    }

    fn build(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
        enforce_limit: bool,
    ) -> Result<Self, BundleError> {
        let mut out: BTreeMap<String, Bytes> = BTreeMap::new();
        let mut total = 0usize;
        for (raw, contents) in files {
            let path = normalise_path(&raw).map_err(invalid)?;
            if path.is_empty() {
                return Err(invalid(format!("path {raw:?} names the bundle root")));
            }
            total = total
                .saturating_add(path.len())
                .saturating_add(contents.len());
            if enforce_limit && total > BUNDLE_SIZE_LIMIT {
                return Err(at_path(
                    &path,
                    format!("bundle exceeds the {BUNDLE_SIZE_LIMIT}-byte size limit at {path:?}"),
                ));
            }
            if enforce_limit && out.len() >= BUNDLE_FILE_LIMIT {
                return Err(at_path(
                    &path,
                    format!(
                        "bundle holds more than the {BUNDLE_FILE_LIMIT}-file limit at {path:?}"
                    ),
                ));
            }
            if let Some(dir) = overlapping(&out, &path) {
                return Err(at_path(
                    &path,
                    format!("path {path:?} conflicts with {dir:?}: one is inside the other"),
                ));
            }
            if out.insert(path.clone(), Bytes::from(contents)).is_some() {
                return Err(at_path(&path, format!("path {path:?} appears twice")));
            }
        }
        let table = read_metadata_file(out.get(METADATA_FILE).map(|b| b.as_ref()))?;
        let seedling = read_requirement(&table)?;
        let metadata = read_metadata(&table);
        let script = read_script(&out, table, &metadata);
        let hash = content_hash(&out);
        Ok(Self {
            files: out,
            hash,
            seedling,
            metadata,
            script,
        })
    }

    /// A bundle holding no files, for an `App` not evaluated from any
    /// definition.
    pub fn empty() -> Self {
        Self::from_files(std::iter::empty()).expect("an empty bundle breaks no rule")
    }

    /// A lone script, as a bundle holding only `app.seed.rhai`.
    // spec: BND#script-formation
    pub fn from_script(text: &str) -> Result<Self, BundleError> {
        Self::from_files([(DEFAULT_SCRIPT.to_owned(), text.as_bytes().to_vec())])
    }

    /// A script stored before definitions were bundles, as the one-file
    /// bundle it now is.
    pub fn from_stored_script(text: &str) -> Self {
        Self::from_stored([(DEFAULT_SCRIPT.to_owned(), text.as_bytes().to_vec())])
            .expect("a single script file breaks no bundle rule but size")
    }

    /// Decode the wire form: an object map of path to base64 contents.
    // spec: DEFN#definition-sources
    pub fn from_wire(
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, BundleError> {
        let mut files = Vec::with_capacity(map.len());
        for (path, value) in map {
            let encoded = value
                .as_str()
                .ok_or_else(|| invalid(format!("contents of {path:?} must be a base64 string")))?;
            let bytes = BASE64
                .decode(encoded)
                .map_err(|e| invalid(format!("contents of {path:?} are not valid base64: {e}")))?;
            files.push((path.clone(), bytes));
        }
        Self::from_files(files)
    }

    /// Decode a definition artefact's layer: a gzip-compressed tar whose root
    /// is the bundle root. Only regular files and directories are accepted.
    // spec: DEFN#fetching
    // spec: DEFN#bundle-rules
    pub fn from_tar_gz(data: &[u8]) -> Result<Self, BundleError> {
        let decoder = flate2::read::GzDecoder::new(data);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|e| invalid(format!("bundle archive is unreadable: {e}")))?;
        let mut files = Vec::new();
        let mut total = 0usize;
        let mut others = 0usize;
        for entry in entries {
            let mut entry =
                entry.map_err(|e| invalid(format!("bundle archive is unreadable: {e}")))?;
            let path = entry
                .path()
                .map_err(|e| invalid(format!("bundle archive has an unreadable path: {e}")))?
                .to_string_lossy()
                .into_owned();
            // Headers cost nothing to decompress and little to hold, so the
            // byte cap alone would let a small blob expand into millions of
            // entries; the count is capped as the entries are read. The file
            // limit counts files, as the bundle rules do, and the entries
            // that hold no file are capped as many again.
            match entry.header().entry_type() {
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    if files.len() >= BUNDLE_FILE_LIMIT {
                        return Err(invalid(format!(
                            "bundle archive holds more than the {BUNDLE_FILE_LIMIT}-file limit"
                        )));
                    }
                }
                tar::EntryType::Directory | tar::EntryType::XGlobalHeader => {
                    others += 1;
                    if others > BUNDLE_FILE_LIMIT {
                        return Err(invalid(format!(
                            "bundle archive holds more than {BUNDLE_FILE_LIMIT} entries that are not files"
                        )));
                    }
                    if entry.header().entry_type() == tar::EntryType::Directory {
                        normalise_path(&path).map_err(invalid)?;
                    }
                    continue;
                }
                other => {
                    return Err(invalid(format!(
                        "{path:?} is not a regular file or directory ({other:?})"
                    )));
                }
            }
            let remaining = BUNDLE_SIZE_LIMIT.saturating_sub(total);
            let mut contents = Vec::new();
            (&mut entry)
                .take(remaining as u64 + 1)
                .read_to_end(&mut contents)
                .map_err(|e| invalid(format!("bundle archive is unreadable at {path:?}: {e}")))?;
            total += contents.len();
            if total > BUNDLE_SIZE_LIMIT {
                return Err(invalid(format!(
                    "bundle exceeds the {BUNDLE_SIZE_LIMIT}-byte size limit at {path:?}"
                )));
            }
            files.push((path, contents));
        }
        Self::from_files(files)
    }

    /// Encode the bundle as a definition artefact's layer, the form
    /// [`Self::from_tar_gz`] reads.
    ///
    /// Every header field that could vary between runs is fixed and files go
    /// in path order, so the same bundle always gives the same bytes and so
    /// the same digest. Only files are written: a directory is implied by
    /// what it holds, and an entry for it would spend the archive's
    /// allowance of entries that hold no file.
    // spec: CLI#definition-packing
    pub fn to_tar_gz(&self) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents) in &self.files {
            append_file(&mut builder, path, contents);
        }
        let tar = builder
            .into_inner()
            .expect("writing a tar to memory is infallible");
        let mut gz = flate2::GzBuilder::new().mtime(0).write(
            Vec::with_capacity(tar.len()),
            flate2::Compression::default(),
        );
        gz.write_all(&tar)
            .expect("compressing to memory is infallible");
        gz.finish().expect("compressing to memory is infallible")
    }

    /// This bundle with the text of its only script file replaced, every
    /// other file carried over unchanged.
    pub fn with_script(&self, text: &str) -> Result<Self, BundleError> {
        let script = self.script()?;
        let mut files = script.files();
        let (Some(only), None) = (files.next(), files.next()) else {
            return Err(invalid(
                "the definition's script spans several files; replace the bundle instead",
            ));
        };
        let only = only.to_owned();
        Self::from_files(self.files.iter().map(|(path, contents)| {
            let contents = if *path == only {
                text.as_bytes().to_vec()
            } else {
                contents.to_vec()
            };
            (path.clone(), contents)
        }))
    }

    pub fn to_wire(&self) -> serde_json::Map<String, serde_json::Value> {
        self.files
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    serde_json::Value::String(BASE64.encode(bytes)),
                )
            })
            .collect()
    }

    /// The storage encoding, which is also what the content hash is taken
    /// over; see [`content_hash`].
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_encoding(&self.files)
    }

    pub fn from_canonical_bytes(data: &[u8]) -> Result<Self, BundleError> {
        let mut rest = data
            .strip_prefix(CANONICAL_MAGIC)
            .ok_or_else(|| invalid("stored bundle has an unknown encoding"))?;
        let mut files = Vec::new();
        while !rest.is_empty() {
            let path = take_chunk(&mut rest)?;
            let contents = take_chunk(&mut rest)?;
            let path = String::from_utf8(path.to_vec())
                .map_err(|_| invalid("stored bundle has a non-UTF-8 path"))?;
            files.push((path, contents.to_vec()));
        }
        Self::from_stored(files)
    }

    /// The Seedling version requirement and metadata table a stored bundle
    /// declares, read straight out of the canonical encoding.
    ///
    /// A generation history page reports both for every definition it lists;
    /// rebuilding each bundle to read them would hash megabytes per row for
    /// values that are one file long.
    // spec: DEFN#provenance-reporting
    pub fn declared_of_canonical(data: &[u8]) -> Result<Declared, BundleError> {
        let table = read_metadata_file(metadata_file_of_canonical(data)?)?;
        Ok(Declared {
            seedling_versions: read_requirement(&table)?,
            metadata: read_metadata(&table)?,
        })
    }

    // spec: DEFN#content-hash-computation
    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn files(&self) -> &BTreeMap<String, Bytes> {
        &self.files
    }

    pub fn file(&self, path: &str) -> Option<&Bytes> {
        self.files.get(path)
    }

    pub fn seedling_versions(&self) -> Option<&VersionRequirement> {
        self.seedling.as_ref()
    }

    pub fn supports(&self, version: &Version) -> bool {
        self.seedling
            .as_ref()
            .is_none_or(|req| req.matches(version))
    }

    /// Refuse the bundle when `version` does not satisfy its requirement.
    // spec: DEFN#seedling-version-support
    pub fn check_supported(&self, version: &Version) -> Result<(), BundleError> {
        match &self.seedling {
            Some(req) if !req.matches(version) => Err(BundleError::Unsupported {
                requirement: req.to_string(),
                running: version.clone(),
            }),
            _ => Ok(()),
        }
    }

    /// The concatenated script, or why the metadata or script files break
    /// the bundle rules.
    pub fn script(&self) -> Result<&Script, BundleError> {
        self.script.as_ref().map_err(Clone::clone)
    }

    /// The metadata table, or why it breaks the bundle rules.
    // spec: BND#metadata-table
    pub fn metadata(&self) -> Result<&Map<String, Value>, BundleError> {
        self.metadata.as_ref().map_err(Clone::clone)
    }

    /// Everything a definition must pass before it is evaluated for an app:
    /// the version requirement first, then the rest of the metadata.
    pub fn check_installable(&self, version: &Version) -> Result<&Script, BundleError> {
        self.check_supported(version)?;
        self.script()
    }
}

/// Append one file with every varying header field fixed.
fn append_file(builder: &mut tar::Builder<Vec<u8>>, path: &str, contents: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    builder
        .append_data(&mut header, path, contents)
        .expect("writing a tar to memory is infallible");
}

/// The accepted path that `path` is inside, or that is inside `path`.
///
/// Neither relationship needs a scan of what has been accepted: every file
/// beneath `path` sorts contiguously from the prefix `path/`, and every
/// directory `path` sits under is one of its own ancestors.
fn overlapping<'a, V>(out: &'a BTreeMap<String, V>, path: &str) -> Option<&'a String> {
    let beneath = format!("{path}/");
    let from_beneath = (Bound::Included(beneath.as_str()), Bound::Unbounded);
    if let Some((key, _)) = out.range::<str, _>(from_beneath).next()
        && key.starts_with(&beneath)
    {
        return Some(key);
    }
    let mut rest = path;
    while let Some((ancestor, _)) = rest.rsplit_once('/') {
        if let Some((key, _)) = out.get_key_value(ancestor) {
            return Some(key);
        }
        rest = ancestor;
    }
    None
}

/// The metadata file parsed as TOML, or an empty table where there is none.
// spec: BND#metadata-file
fn read_metadata_file(file: Option<&[u8]>) -> Result<toml::Table, BundleError> {
    let Some(bytes) = file else {
        return Ok(toml::Table::new());
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| in_metadata(format!("{METADATA_FILE} is not valid UTF-8")))?;
    toml::from_str(text).map_err(|e| BundleError::Invalid {
        message: format!("{METADATA_FILE} cannot be parsed: {e}"),
        place: Some(Place::MetadataFile {
            line: e
                .span()
                .map(|span| text[..span.start].matches('\n').count() + 1),
        }),
    })
}

// spec: BND#metadata-file
fn read_requirement(table: &toml::Table) -> Result<Option<VersionRequirement>, BundleError> {
    match table.get("seedling") {
        None => Ok(None),
        Some(toml::Value::String(s)) => VersionRequirement::parse(s)
            .map(Some)
            .map_err(|e| in_metadata(format!("{METADATA_FILE} field `seedling`: {e}"))),
        Some(other) => Err(in_metadata(format!(
            "{METADATA_FILE} field `seedling` must be a string, got {}",
            other.type_str()
        ))),
    }
}

/// The metadata file's contents, read out of a canonical encoding.
fn metadata_file_of_canonical(data: &[u8]) -> Result<Option<&[u8]>, BundleError> {
    let mut rest = data
        .strip_prefix(CANONICAL_MAGIC)
        .ok_or_else(|| invalid("stored bundle has an unknown encoding"))?;
    while !rest.is_empty() {
        let path = take_chunk(&mut rest)?;
        let contents = take_chunk(&mut rest)?;
        match path.cmp(METADATA_FILE.as_bytes()) {
            std::cmp::Ordering::Less => {}
            // Files are encoded in path order, so it is not here.
            std::cmp::Ordering::Greater => break,
            std::cmp::Ordering::Equal => return Ok(Some(contents)),
        }
    }
    Ok(None)
}

// spec: BND#metadata-table
fn read_metadata(table: &toml::Table) -> Result<Map<String, Value>, BundleError> {
    match table.get("metadata") {
        None => Ok(Map::new()),
        Some(toml::Value::Table(table)) => metadata_map(table, "metadata"),
        Some(other) => Err(in_metadata(format!(
            "{METADATA_FILE} field `metadata` must be a table, got {}",
            other.type_str()
        ))),
    }
}

/// The strict reading of the metadata file and the script files it names.
/// `metadata` is the table as [`read_metadata`] read it, so that its error
/// fails this reading too.
// spec: BND#metadata-file
// spec: DEFN#bundle-rules
fn read_script(
    files: &BTreeMap<String, Bytes>,
    table: toml::Table,
    metadata: &Result<Map<String, Value>, BundleError>,
) -> Result<Script, BundleError> {
    let mut names = vec![DEFAULT_SCRIPT.to_owned()];
    for (key, value) in table {
        match key.as_str() {
            // Read, and checked, by `read_requirement`.
            "seedling" => {}
            "script" => names = script_list(value)?,
            "include" => read_include(files, value)?,
            "metadata" => {
                metadata.as_ref().map_err(Clone::clone)?;
            }
            other => {
                return Err(in_metadata(format!(
                    "{METADATA_FILE} field `{other}` is not defined"
                )));
            }
        }
    }
    if names.is_empty() {
        return Err(in_metadata(format!(
            "{METADATA_FILE} field `script` lists no files"
        )));
    }
    let mut normalised: Vec<String> = Vec::with_capacity(names.len());
    for name in &names {
        let path = normalise_path(name)
            .map_err(|e| in_metadata(format!("{METADATA_FILE} field `script`: {e}")))?;
        if normalised.contains(&path) {
            return Err(in_metadata(format!(
                "{METADATA_FILE} field `script` names {path:?} twice"
            )));
        }
        if path == METADATA_FILE {
            return Err(in_metadata(format!(
                "{METADATA_FILE} cannot itself be a script file"
            )));
        }
        normalised.push(path);
    }
    let mut texts = Vec::with_capacity(normalised.len());
    for path in &normalised {
        let bytes = files.get(path).ok_or_else(|| {
            in_metadata(format!("script file {path:?} is missing from the bundle"))
        })?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| at_path(path, format!("script file {path:?} is not valid UTF-8")))?;
        texts.push((path.as_str(), text));
    }
    Ok(Script::concatenate(texts))
}

/// The Seedling version requirement a metadata file declares, read before,
/// and without, any other field.
// spec: BND#metadata-file
pub fn requirement(metadata_file: &[u8]) -> Result<Option<VersionRequirement>, BundleError> {
    read_requirement(&read_metadata_file(Some(metadata_file))?)
}

/// The `include` table a metadata file declares, each normalised key mapped
/// to the source path a client assembles it from, or an empty map where the
/// file declares none.
///
/// Only the table is read: the rest of the metadata is left to the bundle
/// rules, so a client can assemble a bundle the server then judges whole.
// spec: BND#included-files
pub fn included(metadata_file: &[u8]) -> Result<BTreeMap<String, String>, BundleError> {
    match read_metadata_file(Some(metadata_file))?.remove("include") {
        Some(value) => include_table(value),
        None => Ok(BTreeMap::new()),
    }
}

/// Check the `include` table against the files the bundle holds. Its values
/// are where a client assembled each file from, which only the client reads.
// spec: BND#included-files
fn read_include(files: &BTreeMap<String, Bytes>, value: toml::Value) -> Result<(), BundleError> {
    for path in include_table(value)?.keys() {
        let beneath = format!("{path}/");
        let present = files.contains_key(path)
            || files
                .range::<str, _>((Bound::Included(beneath.as_str()), Bound::Unbounded))
                .next()
                .is_some_and(|(k, _)| k.starts_with(&beneath));
        if !present {
            return Err(in_metadata(format!(
                "{METADATA_FILE} field `include` key {path:?} holds no file in the bundle"
            )));
        }
    }
    Ok(())
}

// spec: BND#included-files
fn include_table(value: toml::Value) -> Result<BTreeMap<String, String>, BundleError> {
    let toml::Value::Table(table) = value else {
        return Err(in_metadata(format!(
            "{METADATA_FILE} field `include` must be a table of strings, got {}",
            value.type_str()
        )));
    };
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    for (key, source) in table {
        let at =
            |msg: String| in_metadata(format!("{METADATA_FILE} field `include` key {key:?} {msg}"));
        let source = match source {
            toml::Value::String(s) if !s.is_empty() => s,
            toml::Value::String(_) => return Err(at("has an empty source path".to_owned())),
            other => return Err(at(format!("must be a string, got {}", other.type_str()))),
        };
        let path = normalise_path(&key).map_err(|e| at(format!("is invalid: {e}")))?;
        if path.is_empty() {
            return Err(at("names the bundle root".to_owned()));
        }
        if path == METADATA_FILE {
            return Err(at(format!("names {METADATA_FILE}")));
        }
        if keys.contains_key(&path) {
            return Err(at(format!("names {path:?}, as another key does")));
        }
        if let Some(other) = overlapping(&keys, &path) {
            return Err(at(format!(
                "overlaps the key {other:?}: one is inside the other"
            )));
        }
        keys.insert(path, source);
    }
    Ok(keys)
}

fn metadata_map(table: &toml::Table, at: &str) -> Result<Map<String, Value>, BundleError> {
    table
        .iter()
        .map(|(key, item)| {
            let converted = metadata_value(item, &format!("{at}.{key}"))?;
            Ok((key.clone(), converted))
        })
        .collect()
}

/// One value of the metadata table, as the JSON value `app.metadata()` and
/// provenance both start from. `at` names where it sits, for the error.
// spec: BND#metadata-table
fn metadata_value(value: &toml::Value, at: &str) -> Result<Value, BundleError> {
    Ok(match value {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        // spec: DEFN#bundle-rules
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .ok_or_else(|| {
                in_metadata(format!(
                    "{METADATA_FILE} field `{at}` is {f}, which is not a finite number"
                ))
            })?,
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| metadata_value(item, &format!("{at}[{i}]")))
                .collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(table) => Value::Object(metadata_map(table, at)?),
    })
}

fn script_list(value: toml::Value) -> Result<Vec<String>, BundleError> {
    let wrong = |got: &str| {
        in_metadata(format!(
            "{METADATA_FILE} field `script` must be an array of strings, got {got}"
        ))
    };
    let toml::Value::Array(items) = value else {
        return Err(wrong(value.type_str()));
    };
    items
        .into_iter()
        .map(|item| match item {
            toml::Value::String(s) => Ok(s),
            other => Err(wrong(&format!("an array holding {}", other.type_str()))),
        })
        .collect()
}

const CANONICAL_MAGIC: &[u8] = b"seedling-bundle-v1\0";

/// The canonical form of a bundle: a fixed prefix, then for each file in path
/// order its path and its contents, each preceded by its length as a
/// big-endian `u64`. It depends only on the paths and bytes, never on the
/// order or archive they arrived in, so a pushed folder and the same folder
/// fetched as a tar hash identically and share one stored row.
// spec: DEFN#content-hash-computation
fn canonical_encoding(files: &BTreeMap<String, Bytes>) -> Vec<u8> {
    let size: usize = files.iter().map(|(p, c)| p.len() + c.len() + 16).sum();
    let mut out = Vec::with_capacity(CANONICAL_MAGIC.len() + size);
    out.extend_from_slice(CANONICAL_MAGIC);
    for (path, contents) in files {
        out.extend_from_slice(&(path.len() as u64).to_be_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(contents.len() as u64).to_be_bytes());
        out.extend_from_slice(contents);
    }
    out
}

fn take_chunk<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8], BundleError> {
    let corrupt = || invalid("stored bundle is truncated");
    let (len, tail) = rest.split_first_chunk::<8>().ok_or_else(corrupt)?;
    let len = usize::try_from(u64::from_be_bytes(*len)).map_err(|_| corrupt())?;
    if tail.len() < len {
        return Err(corrupt());
    }
    let (chunk, tail) = tail.split_at(len);
    *rest = tail;
    Ok(chunk)
}

// spec: DEFN#content-hash-computation
fn content_hash(files: &BTreeMap<String, Bytes>) -> String {
    let digest = Sha256::digest(canonical_encoding(files));
    let mut s = String::with_capacity(71);
    s.push_str("sha256:");
    for b in digest {
        use std::fmt::Write;
        write!(s, "{b:02x}").expect("write to String is infallible");
    }
    s
}

#[cfg(test)]
mod tests;
