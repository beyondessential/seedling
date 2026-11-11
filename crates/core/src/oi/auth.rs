use std::{
    collections::HashSet,
    io,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::RwLock;
use rustls::{
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
    client::danger::HandshakeSignatureValid,
    server::danger::{ClientCertVerified, ClientCertVerifier},
};
use rustls_pki_types::{CertificateDer, SubjectPublicKeyInfoDer, UnixTime};
use subtle::ConstantTimeEq;

use crate::runtime::db::Db;

use seedling_protocol::keys;

// ---------------------------------------------------------------------------
// In-memory trusted key set
// ---------------------------------------------------------------------------

/// The client SPKI fingerprints the server trusts, and a signal that the
/// set has changed.
///
/// The TLS verifier reads this at the handshake, but authorisation has to
/// outlive it: a revoked key must not keep a connection it opened earlier.
/// Rather than announce which key went — a signal a busy connection could
/// miss — each change bumps a generation, and a connection woken by it
/// asks again whether its own key is still here.
// spec: TRN#client-authentication
#[derive(Debug)]
pub struct TrustedKeySet {
    keys: RwLock<HashSet<String>>,
    generation: tokio::sync::watch::Sender<u64>,
}

pub type TrustedKeys = Arc<TrustedKeySet>;

impl Default for TrustedKeySet {
    fn default() -> Self {
        Self {
            keys: RwLock::new(HashSet::new()),
            generation: tokio::sync::watch::Sender::new(0),
        }
    }
}

impl TrustedKeySet {
    /// Add a fingerprint. Already being there is not a change.
    pub fn insert(&self, fingerprint: String) -> bool {
        let added = self.keys.write().insert(fingerprint);
        if added {
            self.changed();
        }
        added
    }

    /// Remove a fingerprint, reporting whether it was there.
    pub fn remove(&self, fingerprint: &str) -> bool {
        let removed = self.keys.write().remove(fingerprint);
        if removed {
            self.changed();
        }
        removed
    }

    /// Whether `fingerprint` is trusted, compared without leaking where
    /// two fingerprints first differ.
    pub fn contains(&self, fingerprint: &str) -> bool {
        let wanted = fingerprint.as_bytes();
        self.keys
            .read()
            .iter()
            .any(|t| t.as_bytes().ct_eq(wanted).into())
    }

    pub fn is_empty(&self) -> bool {
        self.keys.read().is_empty()
    }

    /// A receiver that wakes whenever the set changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation.subscribe()
    }

    fn changed(&self) {
        self.generation.send_modify(|g| *g = g.wrapping_add(1));
    }
}

pub fn new_trusted_keys() -> TrustedKeys {
    Arc::new(TrustedKeySet::default())
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

/// Load all authorized fingerprints from the DB into the in-memory set.
pub fn load_from_db(db: &Db, trusted: &TrustedKeys) -> rusqlite::Result<()> {
    let mut stmt = db.conn.prepare("SELECT fingerprint FROM authorized_keys")?;
    let fps: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for fp in fps {
        trusted.insert(fp);
    }
    Ok(())
}

/// Read `$data_dir/authorized_keys` and import any entries not already in
/// the DB. Lines have the form `<fingerprint> <label>`; `#` and blank lines
/// are ignored.
pub fn import_bootstrap_file(data_dir: &Path, db: &Db, trusted: &TrustedKeys) -> io::Result<()> {
    let path = data_dir.join("authorized_keys");
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut imported = 0u32;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let fp = match parts.next().filter(|s| !s.is_empty()) {
            Some(f) => f,
            None => continue,
        };
        let label = parts.next().unwrap_or("bootstrap").trim();

        let already: bool = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM authorized_keys WHERE fingerprint = ?1",
                [fp],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if !already {
            match db.conn.execute(
                "INSERT INTO authorized_keys (fingerprint, label, added_at) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![fp, label, now],
            ) {
                Ok(_) => {
                    trusted.insert(fp.to_owned());
                    imported += 1;
                }
                Err(e) => {
                    tracing::warn!(fingerprint = %fp, "failed to persist bootstrap key: {e}");
                }
            }
        }
    }

    if imported > 0 {
        tracing::info!(
            count = imported,
            "imported entries from bootstrap authorized_keys file"
        );
    }
    Ok(())
}

/// Look up the label for a fingerprint. Returns `None` if not found.
pub fn get_label(db: &Db, fingerprint: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = db
        .conn
        .prepare("SELECT label FROM authorized_keys WHERE fingerprint = ?1")?;
    let mut rows = stmt.query([fingerprint])?;
    rows.next()?.map(|r| r.get(0)).transpose()
}

// spec: KEY#listing-keys
pub fn list_keys(db: &Db) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = db.conn.prepare(
        "SELECT fingerprint, label, added_at \
         FROM authorized_keys ORDER BY added_at ASC",
    )?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
}

/// Insert a key, or update its label if it already exists.
// spec: KEY#authorising-a-key
pub fn authorize_key(
    db: &Db,
    trusted: &TrustedKeys,
    fp: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    db.conn.execute(
        "INSERT INTO authorized_keys (fingerprint, label, added_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(fingerprint) DO UPDATE SET label = excluded.label",
        rusqlite::params![fp, label, now],
    )?;
    trusted.insert(fp.to_owned());
    Ok(())
}

/// Remove a key. Returns `true` if it was present and removed.
// spec: KEY#revoking-a-key
pub fn revoke_key(db: &Db, trusted: &TrustedKeys, fp: &str) -> rusqlite::Result<bool> {
    let rows = db
        .conn
        .execute("DELETE FROM authorized_keys WHERE fingerprint = ?1", [fp])?;
    if rows > 0 {
        // spec: KEY#revoking-a-key — out of the set the verifier reads, and the
        // signal that goes with it ends the connections it had authorised.
        trusted.remove(fp);
        Ok(true)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Client certificate verifier
// ---------------------------------------------------------------------------

fn ring_verify_tls12(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(
        message,
        cert,
        dss,
        &rustls::crypto::ring::default_provider().signature_verification_algorithms,
    )
}

fn ring_verify_tls13_rpk(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature_with_raw_key(
        message,
        &SubjectPublicKeyInfoDer::from(cert.as_ref()),
        dss,
        &rustls::crypto::ring::default_provider().signature_verification_algorithms,
    )
}

fn ring_schemes() -> Vec<SignatureScheme> {
    rustls::crypto::ring::default_provider()
        .signature_verification_algorithms
        .supported_schemes()
}

#[derive(Debug)]
pub struct SeedlingClientVerifier {
    pub trusted: TrustedKeys,
}

impl ClientCertVerifier for SeedlingClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let fp = keys::fingerprint(end_entity.as_ref());
        if self.trusted.contains(&fp) {
            Ok(ClientCertVerified::assertion())
        } else {
            tracing::warn!(fingerprint = %fp, "rejected client with unrecognized key");
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        ring_verify_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        ring_verify_tls13_rpk(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: KEY#listing-keys
    #[test]
    fn list_empty_on_fresh_db() {
        let db = Db::open_in_memory().unwrap();
        assert!(list_keys(&db).unwrap().is_empty());
    }

    // spec: KEY#authorising-a-key
    // spec: KEY#listing-keys
    #[test]
    fn authorize_then_list_returns_inserted_key() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "laptop").unwrap();
        let list = list_keys(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "fp-1");
        assert_eq!(list[0].1, "laptop");
        assert!(trusted.contains("fp-1"));
    }

    // spec: KEY#authorising-a-key
    #[test]
    fn authorize_updates_label_on_conflict() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "old-label").unwrap();
        authorize_key(&db, &trusted, "fp-1", "new-label").unwrap();
        let list = list_keys(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, "new-label");
    }

    // spec: KEY#revoking-a-key
    #[test]
    fn revoke_removes_authorized_key() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-1", "laptop").unwrap();
        assert!(revoke_key(&db, &trusted, "fp-1").unwrap());
        assert!(list_keys(&db).unwrap().is_empty());
        assert!(!trusted.contains("fp-1"));
    }

    // spec: KEY#revoking-a-key
    #[test]
    fn revoke_returns_false_for_unknown_fingerprint() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        assert!(!revoke_key(&db, &trusted, "unknown-fp").unwrap());
    }

    #[test]
    fn get_label_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        let trusted = new_trusted_keys();
        authorize_key(&db, &trusted, "fp-x", "ci-bot").unwrap();
        assert_eq!(get_label(&db, "fp-x").unwrap().as_deref(), Some("ci-bot"));
        assert!(get_label(&db, "fp-other").unwrap().is_none());
    }
}
