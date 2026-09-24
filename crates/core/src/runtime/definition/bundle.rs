use std::{collections::BTreeMap, fmt, io::Read};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use seedling_protocol::error::{ErrorCode, OiError};
use semver::Version;
use sha2::{Digest, Sha256};

use super::version::VersionRequirement;

/// Upper bound on a bundle's total file size.
///
/// Pushed bundles arrive base64-encoded inside one control-stream request, and
/// `/apps/bundle` returns them the same way, so the cap has to leave the
/// encoded form inside the 4 MiB request and response limits.
pub const BUNDLE_SIZE_LIMIT: usize = 2 * 1024 * 1024;

pub const METADATA_FILE: &str = "seedling.toml";
pub const DEFAULT_SCRIPT: &str = "app.seed.rhai";

/// Why a bundle cannot be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    // i[impl definition.bundle.limits]
    Invalid(String),
    // i[impl definition.bundle.seedling-versions]
    Unsupported {
        requirement: String,
        running: Version,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(msg) => f.write_str(msg),
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
            BundleError::Invalid(_) => ErrorCode::BundleInvalid,
            BundleError::Unsupported { .. } => ErrorCode::UnsupportedSeedling,
        };
        OiError::new(code, e.to_string())
    }
}

fn invalid(msg: impl Into<String>) -> BundleError {
    BundleError::Invalid(msg.into())
}

/// Normalise a bundle path: relative, `/`-separated, `.` and `..` resolved
/// lexically, without touching any filesystem.
// i[impl definition.bundle.limits]
// l[impl app.file]
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
    // l[impl bsl.bundle]
    fn concatenate<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut text = String::new();
        let mut segments = Vec::new();
        let mut next_line = 1;
        for (file, contents) in files {
            let mut lines = contents.lines().count();
            text.push_str(contents);
            // Every file is closed with a newline so the next one starts on
            // a line of its own and the line table stays exact.
            if !contents.ends_with('\n') {
                text.push('\n');
            }
            if contents.is_empty() {
                lines = 1;
            }
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
    fn locate(&self, line: usize) -> Option<(&str, usize)> {
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
    // l[impl bsl.bundle.script-errors]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    files: BTreeMap<String, Bytes>,
    hash: String,
    seedling: Option<VersionRequirement>,
    script: Result<Script, BundleError>,
}

impl Bundle {
    /// Validate a set of files as a bundle.
    ///
    /// Only the rules that make a bundle unholdable fail here: size, paths,
    /// and a `seedling` field that cannot be read. The rest of the metadata
    /// and the script files are checked by [`Self::script`], since a bundle
    /// for a newer Seedling may define fields this one does not know.
    // i[impl definition.bundle.limits]
    pub fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, BundleError> {
        let mut out: BTreeMap<String, Bytes> = BTreeMap::new();
        let mut total = 0usize;
        for (raw, contents) in files {
            let path = normalise_path(&raw).map_err(invalid)?;
            if path.is_empty() {
                return Err(invalid(format!("path {raw:?} names the bundle root")));
            }
            total = total.saturating_add(contents.len());
            if total > BUNDLE_SIZE_LIMIT {
                return Err(invalid(format!(
                    "bundle exceeds the {BUNDLE_SIZE_LIMIT}-byte size limit at {path:?}"
                )));
            }
            if let Some(dir) = out
                .keys()
                .find(|k| is_beneath(k, &path) || is_beneath(&path, k))
            {
                return Err(invalid(format!(
                    "path {path:?} conflicts with {dir:?}: one is inside the other"
                )));
            }
            if out.insert(path.clone(), Bytes::from(contents)).is_some() {
                return Err(invalid(format!("path {path:?} appears twice")));
            }
        }
        let seedling = read_requirement(out.get(METADATA_FILE))?;
        let script = read_script(&out);
        let hash = content_hash(&out);
        Ok(Self {
            files: out,
            hash,
            seedling,
            script,
        })
    }

    /// A lone script, as a bundle holding only `app.seed.rhai`.
    // l[impl bsl.bundle]
    pub fn from_script(text: &str) -> Result<Self, BundleError> {
        Self::from_files([(DEFAULT_SCRIPT.to_owned(), text.as_bytes().to_vec())])
    }

    /// Decode the wire form: an object map of path to base64 contents.
    // i[impl definition.source]
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
    // i[impl definition.fetch]
    // i[impl definition.bundle.limits]
    pub fn from_tar_gz(data: &[u8]) -> Result<Self, BundleError> {
        let decoder = flate2::read::GzDecoder::new(data);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|e| invalid(format!("bundle archive is unreadable: {e}")))?;
        let mut files = Vec::new();
        let mut total = 0usize;
        for entry in entries {
            let mut entry =
                entry.map_err(|e| invalid(format!("bundle archive is unreadable: {e}")))?;
            let path = entry
                .path()
                .map_err(|e| invalid(format!("bundle archive has an unreadable path: {e}")))?
                .to_string_lossy()
                .into_owned();
            match entry.header().entry_type() {
                tar::EntryType::Regular | tar::EntryType::Continuous => {}
                tar::EntryType::Directory => {
                    normalise_path(&path).map_err(invalid)?;
                    continue;
                }
                tar::EntryType::XGlobalHeader => continue,
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
        Self::from_files(files)
    }

    // i[impl definition.content-hash]
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
    // i[impl definition.bundle.seedling-versions]
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

    /// Everything a definition must pass before it is evaluated for an app:
    /// the version requirement first, then the rest of the metadata.
    pub fn check_installable(&self, version: &Version) -> Result<&Script, BundleError> {
        self.check_supported(version)?;
        self.script()
    }
}

fn is_beneath(dir: &str, path: &str) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

// l[impl bsl.bundle.metadata]
fn read_requirement(file: Option<&Bytes>) -> Result<Option<VersionRequirement>, BundleError> {
    let Some(bytes) = file else {
        return Ok(None);
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid(format!("{METADATA_FILE} is not valid UTF-8")))?;
    let table: toml::Table = toml::from_str(text)
        .map_err(|e| invalid(format!("{METADATA_FILE} cannot be parsed: {e}")))?;
    match table.get("seedling") {
        None => Ok(None),
        Some(toml::Value::String(s)) => VersionRequirement::parse(s)
            .map(Some)
            .map_err(|e| invalid(format!("{METADATA_FILE} field `seedling`: {e}"))),
        Some(other) => Err(invalid(format!(
            "{METADATA_FILE} field `seedling` must be a string, got {}",
            other.type_str()
        ))),
    }
}

// l[impl bsl.bundle.metadata]
// i[impl definition.bundle.limits]
fn read_script(files: &BTreeMap<String, Bytes>) -> Result<Script, BundleError> {
    let mut names = vec![DEFAULT_SCRIPT.to_owned()];
    if let Some(bytes) = files.get(METADATA_FILE) {
        // Already known to be UTF-8 and TOML by `read_requirement`.
        let text = std::str::from_utf8(bytes).unwrap_or_default();
        let table: toml::Table = toml::from_str(text).unwrap_or_default();
        for (key, value) in table {
            match key.as_str() {
                // Read, and checked, by `read_requirement`.
                "seedling" => {}
                "script" => names = script_list(value)?,
                other => {
                    return Err(invalid(format!(
                        "{METADATA_FILE} field `{other}` is not defined"
                    )));
                }
            }
        }
    }
    if names.is_empty() {
        return Err(invalid(format!(
            "{METADATA_FILE} field `script` lists no files"
        )));
    }
    let mut normalised: Vec<String> = Vec::with_capacity(names.len());
    for name in &names {
        let path = normalise_path(name)
            .map_err(|e| invalid(format!("{METADATA_FILE} field `script`: {e}")))?;
        if normalised.contains(&path) {
            return Err(invalid(format!(
                "{METADATA_FILE} field `script` names {path:?} twice"
            )));
        }
        if path == METADATA_FILE {
            return Err(invalid(format!(
                "{METADATA_FILE} cannot itself be a script file"
            )));
        }
        normalised.push(path);
    }
    let mut texts = Vec::with_capacity(normalised.len());
    for path in &normalised {
        let bytes = files
            .get(path)
            .ok_or_else(|| invalid(format!("script file {path:?} is missing from the bundle")))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| invalid(format!("script file {path:?} is not valid UTF-8")))?;
        texts.push((path.as_str(), text));
    }
    Ok(Script::concatenate(texts))
}

fn script_list(value: toml::Value) -> Result<Vec<String>, BundleError> {
    let wrong = |got: &str| {
        invalid(format!(
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
// i[impl definition.content-hash]
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

// i[impl definition.content-hash]
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
