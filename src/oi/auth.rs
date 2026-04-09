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

use crate::runtime::db::Db;

use super::keys;

// ---------------------------------------------------------------------------
// In-memory trusted key set
// ---------------------------------------------------------------------------

/// Thread-safe in-memory set of trusted client SPKI fingerprints.
pub type TrustedKeys = Arc<RwLock<HashSet<String>>>;

pub fn new_trusted_keys() -> TrustedKeys {
    Arc::new(RwLock::new(HashSet::new()))
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
    let mut set = trusted.write();
    for fp in fps {
        set.insert(fp);
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
            let _ = db.conn.execute(
                "INSERT INTO authorized_keys (fingerprint, label, added_at) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![fp, label, now],
            );
            trusted.write().insert(fp.to_owned());
            imported += 1;
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
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

// i[key.list]
pub fn list_keys(db: &Db) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = db.conn.prepare(
        "SELECT fingerprint, label, added_at \
         FROM authorized_keys ORDER BY added_at ASC",
    )?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
}

/// Insert a key, or update its label if it already exists.
// i[key.authorize]
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
    trusted.write().insert(fp.to_owned());
    Ok(())
}

/// Remove a key. Returns `true` if it was present and removed.
// i[key.revoke]
pub fn revoke_key(db: &Db, trusted: &TrustedKeys, fp: &str) -> rusqlite::Result<bool> {
    let rows = db
        .conn
        .execute("DELETE FROM authorized_keys WHERE fingerprint = ?1", [fp])?;
    if rows > 0 {
        trusted.write().remove(fp);
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
        if self.trusted.read().contains(&fp) {
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
