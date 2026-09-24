//! Fetching definition bundles as OCI artefacts.
//!
//! Seedling talks to the registry itself rather than through the container
//! engine: selecting a manifest by artifact type, resolving a tag without
//! pulling, and listing tags are all outside what an image pull does, and the
//! re-check and the web tag picker need exactly those.

use std::{fmt, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{StreamExt, future::BoxFuture};
use seedling_protocol::error::{ErrorCode, OiError};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    Bundle, BundleError, VersionRequirement,
    auth::{self, Credentials},
    bundle::BUNDLE_SIZE_LIMIT,
};

pub const ARTIFACT_TYPE: &str = "application/vnd.bes.seedling.definition.v1";
pub const LAYER_MEDIA_TYPE: &str = "application/vnd.bes.seedling.definition.v1.tar+gzip";
pub const VERSIONS_ANNOTATION: &str = "vnd.bes.seedling.versions";

/// Headroom over the uncompressed bundle limit for a layer blob, since gzip
/// can make incompressible content slightly larger.
const BLOB_SIZE_LIMIT: usize = BUNDLE_SIZE_LIMIT + 64 * 1024;
/// Manifests and indexes are small JSON documents.
const MANIFEST_SIZE_LIMIT: usize = 4 * 1024 * 1024;

const MANIFEST_MEDIA_TYPES: &[&str] = &[
    oci_client::manifest::OCI_IMAGE_MEDIA_TYPE,
    oci_client::manifest::OCI_IMAGE_INDEX_MEDIA_TYPE,
    oci_client::manifest::IMAGE_MANIFEST_MEDIA_TYPE,
    oci_client::manifest::IMAGE_MANIFEST_LIST_MEDIA_TYPE,
];

/// Why a fetch was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The reference is not a complete OCI reference.
    BadReference(String),
    // i[impl definition.fetch.access]
    NotAllowed(String),
    // i[impl definition.fetch]
    Failed(String),
    Bundle(BundleError),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadReference(m) | Self::NotAllowed(m) | Self::Failed(m) => f.write_str(m),
            Self::Bundle(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<BundleError> for FetchError {
    fn from(e: BundleError) -> Self {
        Self::Bundle(e)
    }
}

impl From<FetchError> for OiError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::BadReference(m) => OiError::new(ErrorCode::RequirementsInvalid, m),
            FetchError::NotAllowed(m) => OiError::new(ErrorCode::RegistryNotAllowed, m),
            FetchError::Failed(m) => OiError::new(ErrorCode::FetchFailed, m),
            FetchError::Bundle(b) => b.into(),
        }
    }
}

fn failed(msg: impl Into<String>) -> FetchError {
    FetchError::Failed(msg.into())
}

/// A parsed definition reference.
#[derive(Debug, Clone)]
pub struct DefinitionRef {
    raw: String,
    parsed: oci_client::Reference,
}

impl DefinitionRef {
    /// Parse a complete reference: an explicit registry host and a tag or
    /// digest. Registry defaulting and an implied `latest` would make the
    /// recorded provenance say something the operator did not.
    // i[impl definition.source]
    pub fn parse(raw: &str) -> Result<Self, FetchError> {
        let bad = |why: &str| FetchError::BadReference(format!("reference {raw:?} {why}"));
        let (name, has_digest) = match raw.split_once('@') {
            Some((name, _)) => (name, true),
            None => (raw, false),
        };
        let Some((host, path)) = name.split_once('/') else {
            return Err(bad("must name its registry"));
        };
        if !(host.contains('.') || host.contains(':') || host == "localhost") {
            return Err(bad("must name its registry"));
        }
        let has_tag = path
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains(':'));
        if !has_tag && !has_digest {
            return Err(bad("must name a tag or a digest"));
        }
        let parsed: oci_client::Reference = raw
            .parse()
            .map_err(|e| FetchError::BadReference(format!("reference {raw:?} is invalid: {e}")))?;
        Ok(Self {
            raw: raw.to_owned(),
            parsed,
        })
    }

    /// A repository, named as a reference without tag or digest.
    // i[impl definition.tags]
    pub fn parse_repository(raw: &str) -> Result<Self, FetchError> {
        if raw.contains('@')
            || raw
                .rsplit('/')
                .next()
                .is_some_and(|last| last.contains(':'))
        {
            return Err(FetchError::BadReference(format!(
                "repository {raw:?} must not name a tag or digest"
            )));
        }
        let mut with_tag = Self::parse(&format!("{raw}:latest"))?;
        with_tag.raw = raw.to_owned();
        Ok(with_tag)
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn registry(&self) -> &str {
        self.parsed.registry()
    }

    pub fn repository(&self) -> &str {
        self.parsed.repository()
    }

    fn with_digest(&self, digest: &str) -> oci_client::Reference {
        oci_client::Reference::with_digest(
            self.parsed.registry().to_owned(),
            self.parsed.repository().to_owned(),
            digest.to_owned(),
        )
    }
}

/// The registry operations a fetch needs. Behind a trait so fetching can be
/// exercised against an in-memory registry.
pub trait Registry: Send + Sync {
    /// The manifest or index a reference names, with its digest.
    fn manifest<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<(Bytes, String), String>>;

    /// A blob's contents, read to at most `limit` bytes; more is an error.
    fn blob<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
        digest: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Bytes, String>>;

    /// Every tag of the reference's repository.
    fn tags<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<Vec<String>, String>>;
}

/// A definition fetched and decoded.
#[derive(Debug)]
pub struct Fetched {
    pub bundle: Bundle,
    /// The digest of the definition manifest that was selected.
    pub digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestDoc {
    #[serde(default)]
    artifact_type: Option<String>,
    #[serde(default)]
    config: Option<DescriptorDoc>,
    #[serde(default)]
    layers: Option<Vec<DescriptorDoc>>,
    #[serde(default)]
    manifests: Option<Vec<DescriptorDoc>>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DescriptorDoc {
    #[serde(default)]
    media_type: Option<String>,
    digest: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    artifact_type: Option<String>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
}

fn parse_doc(bytes: &[u8], what: &str) -> Result<ManifestDoc, FetchError> {
    if bytes.len() > MANIFEST_SIZE_LIMIT {
        return Err(failed(format!("{what} is too large")));
    }
    serde_json::from_slice(bytes).map_err(|e| failed(format!("{what} is not valid JSON: {e}")))
}

fn annotation_requirement(
    annotations: &std::collections::BTreeMap<String, String>,
    whose: &str,
) -> Result<Option<VersionRequirement>, FetchError> {
    annotations
        .get(VERSIONS_ANNOTATION)
        .map(|a| {
            VersionRequirement::parse(a)
                .map_err(|e| failed(format!("{whose} annotation {VERSIONS_ANNOTATION}: {e}")))
        })
        .transpose()
}

/// Choose among an index's definition entries.
// i[impl definition.fetch.select]
fn select_entry(entries: &[DescriptorDoc], running: &Version) -> Result<DescriptorDoc, FetchError> {
    let definitions: Vec<&DescriptorDoc> = entries
        .iter()
        .filter(|e| e.artifact_type.as_deref() == Some(ARTIFACT_TYPE))
        .collect();
    if definitions.is_empty() {
        return Err(failed("the image index holds no definition artefact"));
    }
    // `None` sorts below every `Some`, which is the rank an unannotated
    // entry takes: a candidate for every version, chosen only when nothing
    // more specific is.
    let mut candidates: Vec<(Option<Version>, &DescriptorDoc)> = Vec::new();
    for entry in definitions {
        match annotation_requirement(&entry.annotations, "index entry")? {
            None => candidates.push((None, entry)),
            Some(req) if req.matches(running) => candidates.push((req.minimum(), entry)),
            Some(_) => {}
        }
    }
    let Some(best) = candidates.iter().map(|(min, _)| min.clone()).max() else {
        return Err(FetchError::Bundle(BundleError::Unsupported {
            requirement: entries
                .iter()
                .filter_map(|e| e.annotations.get(VERSIONS_ANNOTATION).cloned())
                .collect::<Vec<_>>()
                .join(" or "),
            running: running.clone(),
        }));
    };
    let top: Vec<&DescriptorDoc> = candidates
        .iter()
        .filter(|(min, _)| *min == best)
        .map(|(_, e)| *e)
        .collect();
    if top.len() > 1 {
        let names: Vec<String> = top
            .iter()
            .map(|e| {
                format!(
                    "{} ({})",
                    e.digest,
                    e.annotations
                        .get(VERSIONS_ANNOTATION)
                        .map_or("unannotated", String::as_str)
                )
            })
            .collect();
        return Err(failed(format!(
            "the image index has several equally suited definition entries: {}",
            names.join(", ")
        )));
    }
    Ok(top[0].clone())
}

/// The definition manifest a reference selects: its document, its digest,
/// and the requirement the index claimed for it, if it came from an index.
async fn resolve(
    registry: &dyn Registry,
    reference: &DefinitionRef,
    running: &Version,
) -> Result<(ManifestDoc, String, Option<VersionRequirement>), FetchError> {
    let (bytes, digest) = registry
        .manifest(&reference.parsed)
        .await
        .map_err(|e| failed(format!("could not fetch {}: {e}", reference.as_str())))?;
    let doc = parse_doc(&bytes, "manifest")?;
    let Some(entries) = &doc.manifests else {
        let is_definition = doc.artifact_type.as_deref() == Some(ARTIFACT_TYPE)
            || doc.config.as_ref().and_then(|c| c.media_type.as_deref()) == Some(ARTIFACT_TYPE);
        if !is_definition {
            return Err(failed(format!(
                "{} is not a definition artefact",
                reference.as_str()
            )));
        }
        return Ok((doc, digest, None));
    };
    let entry = select_entry(entries, running)?;
    let claimed = annotation_requirement(&entry.annotations, "index entry")?;
    let child = reference.with_digest(&entry.digest);
    let (bytes, child_digest) = registry.manifest(&child).await.map_err(|e| {
        failed(format!(
            "could not fetch definition entry {}: {e}",
            entry.digest
        ))
    })?;
    if child_digest != entry.digest {
        return Err(failed(format!(
            "definition entry {} was served with digest {child_digest}",
            entry.digest
        )));
    }
    Ok((
        parse_doc(&bytes, "definition manifest")?,
        entry.digest,
        claimed,
    ))
}

/// Resolve a reference to the definition manifest digest it selects, reading
/// only manifests.
// r[impl definition.recheck]
pub async fn resolve_digest(
    registry: &dyn Registry,
    reference: &DefinitionRef,
    running: &Version,
) -> Result<String, FetchError> {
    resolve(registry, reference, running)
        .await
        .map(|(_, digest, _)| digest)
}

/// Fetch and decode the definition a reference selects.
// i[impl definition.fetch]
pub async fn fetch(
    registry: &dyn Registry,
    reference: &DefinitionRef,
    running: &Version,
) -> Result<Fetched, FetchError> {
    let (doc, digest, claimed) = resolve(registry, reference, running).await?;
    let layers = doc.layers.as_deref().unwrap_or_default();
    let [layer] = layers else {
        return Err(failed(format!(
            "definition manifest has {} layers; it must have exactly one",
            layers.len()
        )));
    };
    if layer.media_type.as_deref() != Some(LAYER_MEDIA_TYPE) {
        return Err(failed(format!(
            "definition layer has media type {:?}; expected {LAYER_MEDIA_TYPE}",
            layer.media_type.as_deref().unwrap_or("")
        )));
    }
    if layer.size.is_some_and(|s| s > BLOB_SIZE_LIMIT as u64) {
        return Err(FetchError::Bundle(BundleError::Invalid(format!(
            "definition layer exceeds the {BUNDLE_SIZE_LIMIT}-byte bundle size limit"
        ))));
    }
    let blob = registry
        .blob(&reference.parsed, &layer.digest, BLOB_SIZE_LIMIT)
        .await
        .map_err(|e| failed(format!("could not fetch definition layer: {e}")))?;
    verify_digest(&blob, &layer.digest)?;
    let bundle = Bundle::from_tar_gz(&blob)?;

    let declared = bundle.seedling_versions().map(|r| r.as_str().to_owned());
    for (whose, annotated) in [
        (
            "manifest",
            annotation_requirement(&doc.annotations, "manifest")?,
        ),
        ("index entry", claimed),
    ] {
        if let Some(annotated) = annotated
            && Some(annotated.as_str()) != declared.as_deref()
        {
            return Err(FetchError::Bundle(BundleError::Invalid(format!(
                "the {whose} annotation {VERSIONS_ANNOTATION} says {:?}, but the bundle declares {}",
                annotated.as_str(),
                declared.map_or("no requirement".to_owned(), |d| format!("{d:?}")),
            ))));
        }
    }
    Ok(Fetched { bundle, digest })
}

fn verify_digest(blob: &[u8], expected: &str) -> Result<(), FetchError> {
    let Some(hex) = expected.strip_prefix("sha256:") else {
        return Err(failed(format!(
            "definition layer digest {expected} uses an unsupported algorithm"
        )));
    };
    let actual: String = Sha256::digest(blob)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != hex {
        return Err(failed(format!(
            "definition layer does not match its digest {expected}"
        )));
    }
    Ok(())
}

/// List a repository's tags.
// i[impl definition.tags]
pub async fn list_tags(
    registry: &dyn Registry,
    repository: &DefinitionRef,
) -> Result<Vec<String>, FetchError> {
    registry.tags(&repository.parsed).await.map_err(|e| {
        failed(format!(
            "could not list tags of {}: {e}",
            repository.as_str()
        ))
    })
}

/// The registry allowlist gate, applied before any request.
// i[impl definition.fetch.access]
pub fn check_allowed(allowed: &[String], reference: &DefinitionRef) -> Result<(), FetchError> {
    if allowed.iter().any(|r| r == reference.registry()) {
        Ok(())
    } else {
        Err(FetchError::NotAllowed(format!(
            "registry {} is not on the registry allowlist",
            reference.registry()
        )))
    }
}

/// The real registry, reached over HTTPS with the host's container-engine
/// credentials.
pub struct OciRegistry {
    client: oci_client::Client,
}

impl Default for OciRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl OciRegistry {
    pub fn new() -> Self {
        let config = oci_client::client::ClientConfig {
            connect_timeout: Some(Duration::from_secs(15)),
            read_timeout: Some(Duration::from_secs(60)),
            platform_resolver: None,
            ..Default::default()
        };
        Self {
            client: oci_client::Client::new(config),
        }
    }

    // i[impl definition.fetch.access]
    fn auth(reference: &oci_client::Reference) -> oci_client::secrets::RegistryAuth {
        match auth::lookup(
            &auth::auth_file_candidates(),
            reference.registry(),
            reference.repository(),
        ) {
            Credentials::Anonymous => oci_client::secrets::RegistryAuth::Anonymous,
            Credentials::Basic { username, password } => {
                oci_client::secrets::RegistryAuth::Basic(username, password)
            }
        }
    }
}

impl Registry for OciRegistry {
    fn manifest<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<(Bytes, String), String>> {
        Box::pin(async move {
            self.client
                .pull_manifest_raw(reference, &Self::auth(reference), MANIFEST_MEDIA_TYPES)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn blob<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
        digest: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Bytes, String>> {
        Box::pin(async move {
            let auth = Self::auth(reference);
            self.client
                .auth(reference, &auth, oci_client::RegistryOperation::Pull)
                .await
                .map_err(|e| e.to_string())?;
            let mut stream = self
                .client
                .pull_blob_stream(reference, &digest)
                .await
                .map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            while let Some(chunk) = stream.stream.next().await {
                let chunk = chunk.map_err(|e| e.to_string())?;
                if out.len() + chunk.len() > limit {
                    return Err(format!("blob exceeds {limit} bytes"));
                }
                out.extend_from_slice(&chunk);
            }
            Ok(Bytes::from(out))
        })
    }

    fn tags<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            const PAGE: usize = 500;
            let auth = Self::auth(reference);
            let mut tags: Vec<String> = Vec::new();
            loop {
                let page = self
                    .client
                    .list_tags(
                        reference,
                        &auth,
                        Some(PAGE),
                        tags.last().map(String::as_str),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let count = page.tags.len();
                // A registry that ignores `last` would repeat the first
                // page forever; stop as soon as a page brings nothing new.
                let before = tags.len();
                for tag in page.tags {
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
                if count < PAGE || tags.len() == before {
                    break;
                }
            }
            Ok(tags)
        })
    }
}

/// The registry client shared by every fetch in the daemon.
pub type SharedRegistry = Arc<dyn Registry>;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;
