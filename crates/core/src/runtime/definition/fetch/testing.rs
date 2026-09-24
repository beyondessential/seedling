//! An in-memory OCI registry for exercising fetches.

use std::{
    collections::BTreeMap,
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
};

use bytes::Bytes;
use futures_util::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ARTIFACT_TYPE, LAYER_MEDIA_TYPE, Registry, VERSIONS_ANNOTATION};

pub fn digest_of(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// A gzipped tar of `files`, the layer of a definition artefact.
pub fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, *contents).unwrap();
    }
    let tar = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

#[derive(Default)]
pub struct FakeRegistry {
    /// Keyed by `registry/repository:tag` or `registry/repository@digest`.
    manifests: Mutex<BTreeMap<String, Bytes>>,
    /// Digests the registry claims, for keys where that is not the digest of
    /// the bytes it serves.
    reported: Mutex<BTreeMap<String, String>>,
    blobs: Mutex<BTreeMap<String, Bytes>>,
    tags: Mutex<BTreeMap<String, Vec<String>>>,
    unreachable: Mutex<bool>,
    pub requests: AtomicUsize,
    pub blob_requests: AtomicUsize,
}

impl FakeRegistry {
    pub fn set_unreachable(&self, down: bool) {
        *self.unreachable.lock() = down;
    }

    fn put_manifest(&self, repo: &str, doc: &Value) -> String {
        let bytes = serde_json::to_vec(doc).unwrap();
        let digest = digest_of(&bytes);
        self.manifests
            .lock()
            .insert(format!("{repo}@{digest}"), Bytes::from(bytes));
        digest
    }

    /// Serve `bytes` for a key that already resolves, as a registry that
    /// does not honour the digest or tag it was asked for. `reported` is the
    /// digest it claims for them, standing in for the `Docker-Content-Digest`
    /// header a real registry sends; without one it reports them honestly.
    pub fn serve_instead(&self, key: &str, bytes: &[u8], reported: Option<&str>) {
        self.manifests
            .lock()
            .insert(key.to_owned(), Bytes::copy_from_slice(bytes));
        match reported {
            Some(d) => self.reported.lock().insert(key.to_owned(), d.to_owned()),
            None => self.reported.lock().remove(key),
        };
    }

    /// Point `repo:tag` at the manifest with `digest`.
    pub fn tag(&self, repo: &str, tag: &str, digest: &str) {
        let doc = self
            .manifests
            .lock()
            .get(&format!("{repo}@{digest}"))
            .cloned()
            .expect("tagging a manifest that exists");
        self.manifests.lock().insert(format!("{repo}:{tag}"), doc);
        let mut tags = self.tags.lock();
        let list = tags.entry(repo.to_owned()).or_default();
        if !list.iter().any(|t| t == tag) {
            list.push(tag.to_owned());
        }
    }

    /// Publish a definition artefact holding `files`, returning its manifest
    /// digest. `annotation` is the manifest's version annotation.
    pub fn push_definition(
        &self,
        repo: &str,
        files: &[(&str, &[u8])],
        annotation: Option<&str>,
    ) -> String {
        let layer = tar_gz(files);
        let layer_digest = digest_of(&layer);
        self.blobs
            .lock()
            .insert(layer_digest.clone(), Bytes::from(layer.clone()));
        let mut annotations = serde_json::Map::new();
        if let Some(a) = annotation {
            annotations.insert(VERSIONS_ANNOTATION.into(), json!(a));
        }
        self.put_manifest(
            repo,
            &json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "artifactType": ARTIFACT_TYPE,
                "config": {
                    "mediaType": "application/vnd.oci.empty.v1+json",
                    "digest": "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
                    "size": 2
                },
                "layers": [{
                    "mediaType": LAYER_MEDIA_TYPE,
                    "digest": layer_digest,
                    "size": layer.len(),
                }],
                "annotations": annotations,
            }),
        )
    }

    /// Publish an image manifest that is not a definition.
    pub fn push_image(&self, repo: &str) -> String {
        self.put_manifest(
            repo,
            &json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "config": {
                    "mediaType": "application/vnd.oci.image.config.v1+json",
                    "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "size": 2
                },
                "layers": [],
            }),
        )
    }

    /// Publish an index over `entries`: `(digest, artifact type, annotation)`.
    pub fn push_index(&self, repo: &str, entries: &[(&str, Option<&str>, Option<&str>)]) -> String {
        let manifests: Vec<Value> = entries
            .iter()
            .map(|(digest, artifact_type, annotation)| {
                let mut entry = json!({
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": digest,
                    "size": 100,
                });
                if let Some(t) = artifact_type {
                    entry["artifactType"] = json!(t);
                } else {
                    entry["platform"] = json!({"architecture": "amd64", "os": "linux"});
                }
                if let Some(a) = annotation {
                    entry["annotations"] = json!({ VERSIONS_ANNOTATION: a });
                }
                entry
            })
            .collect();
        self.put_manifest(
            repo,
            &json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.index.v1+json",
                "manifests": manifests,
            }),
        )
    }

    fn check_up(&self) -> Result<(), String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if *self.unreachable.lock() {
            Err("connection refused".to_owned())
        } else {
            Ok(())
        }
    }
}

fn key(reference: &oci_client::Reference) -> String {
    let repo = format!("{}/{}", reference.registry(), reference.repository());
    match (reference.digest(), reference.tag()) {
        (Some(d), _) => format!("{repo}@{d}"),
        (None, Some(t)) => format!("{repo}:{t}"),
        (None, None) => format!("{repo}:latest"),
    }
}

impl Registry for FakeRegistry {
    fn manifest<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<(Bytes, String), String>> {
        Box::pin(async move {
            self.check_up()?;
            let bytes = self
                .manifests
                .lock()
                .get(&key(reference))
                .cloned()
                .ok_or_else(|| "manifest unknown".to_owned())?;
            let digest = self
                .reported
                .lock()
                .get(&key(reference))
                .cloned()
                .unwrap_or_else(|| digest_of(&bytes));
            Ok((bytes, digest))
        })
    }

    fn blob<'a>(
        &'a self,
        _reference: &'a oci_client::Reference,
        digest: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Bytes, String>> {
        Box::pin(async move {
            self.check_up()?;
            self.blob_requests.fetch_add(1, Ordering::SeqCst);
            let blob = self
                .blobs
                .lock()
                .get(digest)
                .cloned()
                .ok_or_else(|| "blob unknown".to_owned())?;
            if blob.len() > limit {
                return Err("blob too large".to_owned());
            }
            Ok(blob)
        })
    }

    fn tags<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            self.check_up()?;
            let repo = format!("{}/{}", reference.registry(), reference.repository());
            Ok(self.tags.lock().get(&repo).cloned().unwrap_or_default())
        })
    }
}
