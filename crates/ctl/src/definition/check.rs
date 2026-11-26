//! Checking a definition with no server, by evaluating it as a server would
//! on an update.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use seedling_bundle::{
    Bundle, BundleError, METADATA_FILE, Place,
    source::{SourceError, read_folder_definition, read_lone_file},
};
use seedling_core::{
    ScriptLimits,
    runtime::{
        apps::{self, Rejection, ScriptError},
        definition::{
            fetch::{self, DefinitionRef, FetchError, OciRegistry},
            version,
        },
        seeds::AppSeed,
    },
    sysconst::{self, SystemFacts, Undetermined},
};
use seedling_protocol::names::AppName;
use semver::Version;
use serde_json::{Value, json};

use super::github::{self, GithubUrl};

/// `definitions check`.
// spec: CLI#definition-checks
#[derive(clap::Args, Debug)]
pub(crate) struct CheckArgs {
    /// A script file, a definition folder, or a GitHub URL naming a folder
    /// at a branch, tag, or commit
    #[arg(required_unless_present = "reference")]
    source: Option<String>,
    /// Fetch the definition from this OCI reference and check it
    #[arg(long = "ref", value_name = "REFERENCE", conflicts_with = "source")]
    reference: Option<String>,
    /// Evaluate with a sample parameter value, and validate it
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,
    /// Evaluate with a host constant set to a value
    #[arg(long = "const", value_name = "NAME=VALUE")]
    constants: Vec<String>,
    /// Print the errors to standard output as JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    limits: ScriptLimits,
}

/// What kind of check an error comes from.
// spec: CLI#check-results
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Source,
    Version,
    Bundle,
    Script,
    Validation,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Version => "version",
            Self::Bundle => "bundle",
            Self::Script => "script",
            Self::Validation => "validation",
        }
    }
}

/// One error a check reports.
// spec: CLI#check-results
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CheckError {
    pub kind: Kind,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub parameter: Option<String>,
}

impl CheckError {
    fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            file: None,
            line: None,
            parameter: None,
        }
    }

    fn at(mut self, file: String, line: Option<usize>) -> Self {
        self.file = Some(file);
        self.line = line;
        self
    }

    // spec: CLI#check-results
    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "message": self.message,
            "file": self.file,
            "line": self.line,
            "parameter": self.parameter,
        })
    }
}

impl std::fmt::Display for CheckError {
    // spec: CLI#check-results
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(file) = &self.file {
            f.write_str(file)?;
            if let Some(line) = self.line {
                write!(f, ":{line}")?;
            }
            f.write_str(": ")?;
        }
        f.write_str(&self.message)
    }
}

/// Where the files of the bundle being checked were read from, which an
/// error's location names.
// spec: CLI#check-results
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Origin {
    /// A file source: every location names the path it was given as.
    File(String),
    /// A folder source, by the path it was given as, with the path each
    /// file placed from a source was read from, by bundle path.
    Folder {
        folder: String,
        sources: BTreeMap<String, String>,
    },
    /// A source whose locations are bundle paths.
    Bundle,
}

impl Origin {
    /// The file a location in the bundle file at `path` names.
    fn file(&self, path: &str) -> String {
        match self {
            Self::File(given) => given.clone(),
            Self::Folder { folder, sources } => sources
                .get(path)
                .cloned()
                .unwrap_or_else(|| lexical_join(Path::new(folder), path).display().to_string()),
            Self::Bundle => path.to_owned(),
        }
    }

    /// The error a bundle error is reported as.
    // spec: CLI#check-results
    fn bundle_error(&self, e: BundleError) -> CheckError {
        match e {
            BundleError::Unsupported { .. } => {
                CheckError::new(Kind::Version, e.to_string()).at(self.file(METADATA_FILE), None)
            }
            BundleError::Invalid { message, place } => {
                let error = CheckError::new(Kind::Bundle, message);
                match place {
                    Some(Place::MetadataFile { line }) => error.at(self.file(METADATA_FILE), line),
                    Some(Place::Path(path)) => error.at(self.file(&path), None),
                    None => error,
                }
            }
        }
    }

    /// The error a failure to read or assemble the source is reported as.
    // spec: CLI#check-results
    fn source_error(&self, e: SourceError) -> CheckError {
        match e {
            SourceError::Bundle(e) => self.bundle_error(e),
            SourceError::Include(message) => {
                CheckError::new(Kind::Source, message).at(self.file(METADATA_FILE), None)
            }
            SourceError::Other(message) => CheckError::new(Kind::Source, message),
        }
    }
}

/// The path each included file was read from, given the folder as `given`
/// and each file's source relative to the folder: relative to the working
/// directory where `given` is relative, so it reads as the folder does.
// spec: CLI#check-results
// A `..` resolved against the path as given would climb out of a symlinked
// folder rather than the folder it names, so the source is resolved against
// the canonical folder.
fn read_from(
    given: &Path,
    sources: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let canonical = given
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", given.display()))?;
    let base = if given.is_relative() {
        std::env::current_dir()
            .and_then(|cwd| cwd.canonicalize())
            .ok()
    } else {
        None
    };
    Ok(sources
        .into_iter()
        .map(|(path, within)| {
            let file = lexical_join(&canonical, &within);
            let shown = base
                .as_deref()
                .and_then(|base| pathdiff::diff_paths(&file, base))
                .unwrap_or(file);
            (path, shown.display().to_string())
        })
        .collect())
}

/// `rel`, a `/`-separated path, joined onto `base`, with each `..` taking
/// off a name `base` ends in rather than standing after it.
fn lexical_join(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for segment in rel.split('/') {
        match segment {
            "" | "." => {}
            ".." if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            segment => out.push(segment),
        }
    }
    out
}

/// The options that settle what a check evaluates with, each read and
/// refused before anything is checked.
// spec: CLI#definition-checks
#[derive(Debug)]
pub(super) struct Options {
    reference: Option<DefinitionRef>,
    samples: BTreeMap<String, String>,
    facts: SystemFacts,
}

impl Options {
    pub fn read(args: &CheckArgs) -> Result<Self, String> {
        let reference = args
            .reference
            .as_deref()
            .map(DefinitionRef::parse)
            .transpose()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            reference,
            samples: sample_values(&args.set)?,
            facts: host_constants(&args.constants)?,
        })
    }
}

/// The sample parameter values, each `name=value` split at the first `=`.
// spec: CLI#sample-parameter-values
pub(super) fn sample_values(set: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for pair in set {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("--set takes name=value, got {pair:?}"))?;
        if values.insert(name.to_owned(), value.to_owned()).is_some() {
            return Err(format!("parameter {name:?} is set more than once"));
        }
    }
    Ok(values)
}

/// The host facts a check evaluates with: each constant at its default for a
/// check, unless a `NAME=value` in `set` sets it.
// spec: CLI#check-host-constants
pub(super) fn host_constants(set: &[String]) -> Result<SystemFacts, String> {
    let mut facts = SystemFacts {
        ipv4_egress: false,
        ipv6_egress: false,
        nat64_active: false,
        available_threads: sysconst::detect_available_threads().ok(),
        available_memory: Err("a definition check gives it only with --const".to_owned()),
        has_snapshots: false,
        node_name: Ok(String::new()),
        cpu_architecture: std::env::consts::ARCH.to_owned(),
        timezone: "UTC".to_owned(),
        undetermined: Undetermined::FailsRead,
    };
    let mut seen = Vec::new();
    for pair in set {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("--const takes NAME=value, got {pair:?}"))?;
        if seen.contains(&name) {
            return Err(format!("host constant {name} is set more than once"));
        }
        seen.push(name);
        let wrong = |what: &str| format!("host constant {name} must be {what}, got {value:?}");
        let boolean = || match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(wrong("true or false")),
        };
        let positive = || match value.parse::<i64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(wrong("an integer greater than zero")),
        };
        match name {
            "AVAILABLE_THREADS" => facts.available_threads = Some(positive()?),
            "AVAILABLE_MEMORY" => facts.available_memory = Ok(positive()?),
            "CPU_ARCHITECTURE" if value.is_empty() => return Err(wrong("a name")),
            "CPU_ARCHITECTURE" => facts.cpu_architecture = value.to_owned(),
            "HOST_HAS_IPV4" => facts.ipv4_egress = boolean()?,
            "HOST_HAS_IPV6" => facts.ipv6_egress = boolean()?,
            "NAT64_ACTIVE" => facts.nat64_active = boolean()?,
            "HAS_SNAPSHOTS" => facts.has_snapshots = boolean()?,
            "NODE_NAME" => facts.node_name = Ok(value.to_owned()),
            "TIMEZONE" if jiff::tz::db().get(value).is_err() => {
                return Err(wrong("an IANA time zone name"));
            }
            "TIMEZONE" => facts.timezone = value.to_owned(),
            _ => return Err(format!("{name} is not a host constant")),
        }
    }
    Ok(facts)
}

/// Check the definition `args` names, printing what the check found, and
/// give the status to exit with.
// spec: CLI#check-results
pub(super) async fn run(args: &CheckArgs) -> i32 {
    let options = match Options::read(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    sysconst::set(options.facts.clone());
    let errors = check(args.source.as_deref(), &options, &args.limits).await;
    if args.json {
        let errors: Vec<Value> = errors.iter().map(CheckError::to_json).collect();
        println!("{}", json!({ "errors": errors }));
    } else {
        for e in &errors {
            eprintln!("{e}");
        }
    }
    if errors.is_empty() { 0 } else { 1 }
}

/// Read the definition and check it, stopping at the first check that fails.
// spec: CLI#definition-checks
async fn check(source: Option<&str>, options: &Options, limits: &ScriptLimits) -> Vec<CheckError> {
    let running = version::running();
    let (bundle, origin) = match read(source, options.reference.as_ref(), &running).await {
        Ok(read) => read,
        Err(e) => return vec![e],
    };
    if let Err(e) = bundle.check_supported(&running) {
        return vec![origin.bundle_error(e)];
    }
    if let Err(e) = bundle.script() {
        return vec![origin.bundle_error(e)];
    }
    evaluate(bundle, &origin, &options.samples, limits)
}

/// The bundle a source names, held to the CLI's version before anything
/// else of its metadata is read, and to the rules every bundle keeps.
// spec: CLI#definition-checks
async fn read(
    source: Option<&str>,
    reference: Option<&DefinitionRef>,
    running: &Version,
) -> Result<(Bundle, Origin), CheckError> {
    if let Some(reference) = reference {
        // spec: CLI#check-results
        // A fetched bundle has no file the CLI read, so its locations are
        // bundle paths.
        let fetched = fetch::fetch(&OciRegistry::new(), reference, running)
            .await
            .map_err(|e| match e {
                // Only an index's entries are held to the version before
                // the bundle is read, and none of them is the bundle's own.
                FetchError::Bundle(BundleError::Unsupported { .. }) => {
                    CheckError::new(Kind::Version, e.to_string())
                }
                FetchError::Bundle(e) => Origin::Bundle.bundle_error(e),
                e => CheckError::new(Kind::Source, e.to_string()),
            })?;
        return Ok((fetched.bundle, Origin::Bundle));
    }
    let source = source.expect("clap requires a source or a reference");
    let (files, origin) = if let Some(url) = GithubUrl::parse(source) {
        let (assembled, _) = github::download(&url, Some(running))
            .await
            .map_err(|e| Origin::Bundle.source_error(e))?;
        (assembled.files, Origin::Bundle)
    } else {
        let path = PathBuf::from(source);
        let meta = std::fs::metadata(&path)
            .map_err(|e| CheckError::new(Kind::Source, format!("cannot read {source}: {e}")))?;
        if meta.is_dir() {
            let mut origin = Origin::Folder {
                folder: source.to_owned(),
                sources: BTreeMap::new(),
            };
            let assembled =
                read_folder_definition(&path, Some(running)).map_err(|e| origin.source_error(e))?;
            if let Origin::Folder { sources, .. } = &mut origin {
                *sources = read_from(&path, assembled.sources)
                    .map_err(|e| CheckError::new(Kind::Source, e))?;
            }
            (assembled.files, origin)
        } else {
            let origin = Origin::File(source.to_owned());
            let files = read_lone_file(&path).map_err(|e| origin.source_error(e))?;
            (files, origin)
        }
    };
    let bundle = Bundle::from_files(files).map_err(|e| origin.bundle_error(e))?;
    Ok((bundle, origin))
}

/// Evaluate the bundle as an update with `samples` as the app's values
/// would, and validate them.
// spec: CLI#definition-checks
// spec: CLI#sample-parameter-values
fn evaluate(
    bundle: Bundle,
    origin: &Origin,
    samples: &BTreeMap<String, String>,
    limits: &ScriptLimits,
) -> Vec<CheckError> {
    let name = AppName::new("check").expect("a valid app name");
    let (app, evaluated) = apps::evaluate_validated(
        &name,
        &Arc::new(bundle),
        samples,
        &apps::supplied_values(samples),
        &AppSeed::throwaway(),
        limits,
    );
    let mut rejections = match evaluated {
        Ok(rejections) => rejections,
        Err(ScriptError { message, at }) => {
            let error = CheckError::new(Kind::Script, message);
            return vec![match at {
                Some((file, line)) => error.at(origin.file(&file), Some(line)),
                None => error,
            }];
        }
    };
    let def = app.def.load();
    // spec: CLI#sample-parameter-values
    rejections.extend(
        samples
            .keys()
            .filter(|name| !def.params.keys().any(|p| p.as_str() == name.as_str()))
            .map(|name| Rejection {
                name: name.clone(),
                reason: "not declared".to_owned(),
            }),
    );
    rejections.sort_by(|a, b| a.name.cmp(&b.name));
    rejections
        .into_iter()
        .map(|Rejection { name, reason }| CheckError {
            parameter: Some(name.clone()),
            ..CheckError::new(Kind::Validation, format!("parameter {name:?}: {reason}"))
        })
        .collect()
}
