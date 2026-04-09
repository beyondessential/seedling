use std::{io, path::Path, sync::Arc};

use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use rand_core::OsRng;
use rustls::{crypto::ring::sign, sign::CertifiedKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// Load an Ed25519 signing key from a PKCS#8 DER file, or generate and
/// persist one if the file does not exist.
pub fn load_or_generate(path: &Path) -> io::Result<SigningKey> {
    if path.exists() {
        let der = std::fs::read(path)?;
        SigningKey::from_pkcs8_der(&der).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let key = SigningKey::generate(&mut OsRng);
        let doc = key.to_pkcs8_der().map_err(|e| io::Error::other(e))?;
        std::fs::write(path, doc.as_bytes())?;
        Ok(key)
    }
}

/// Build the SubjectPublicKeyInfo (SPKI) DER encoding for an Ed25519 key.
///
/// Fixed structure:
/// ```text
/// SEQUENCE {
///   SEQUENCE { OID 1.3.101.112 }      -- Ed25519
///   BIT STRING { 0x00 || 32-byte key }
/// }
/// ```
pub fn spki_der(key: &SigningKey) -> Vec<u8> {
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, // SEQUENCE 42 bytes
        0x30, 0x05, // SEQUENCE 5 bytes
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112
        0x03, 0x21, 0x00, // BIT STRING 33 bytes, 0 unused bits
    ];
    let mut out = Vec::with_capacity(44);
    out.extend_from_slice(&PREFIX);
    out.extend_from_slice(key.verifying_key().as_bytes());
    out
}

/// SHA-256 fingerprint of a byte slice, returned as a lowercase hex string.
pub fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// ClientIdentity
// ---------------------------------------------------------------------------

/// A client's signing identity: key pair with pre-computed SPKI and fingerprint.
pub struct ClientIdentity {
    signing_key: SigningKey,
    spki: Vec<u8>,
    /// SHA-256 fingerprint of the SPKI, hex-encoded (no prefix).
    pub fingerprint: String,
}

impl ClientIdentity {
    /// Load from `path`, or generate a new key and persist it there.
    /// Returns `(identity, is_new)`.
    pub fn load_or_generate(path: &Path) -> io::Result<(Self, bool)> {
        let is_new = !path.exists();
        let key = load_or_generate(path)?;
        let spki = spki_der(&key);
        let fp = fingerprint(&spki);
        Ok((
            Self {
                signing_key: key,
                spki,
                fingerprint: fp,
            },
            is_new,
        ))
    }

    /// Default key path: `$XDG_STATE_HOME/seedling/client.key`.
    pub fn default_path() -> std::path::PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("seedling")
            .join("client.key")
    }

    /// Build a rustls `CertifiedKey` for use as a raw-public-key client cert.
    pub fn to_certified_key(
        &self,
    ) -> Result<Arc<CertifiedKey>, Box<dyn std::error::Error + Send + Sync>> {
        let pkcs8 = self
            .signing_key
            .to_pkcs8_der()
            .map_err(|e| format!("key encoding: {e}"))?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));
        let signing =
            sign::any_supported_type(&private_key).map_err(|e| format!("signing key: {e}"))?;
        let cert = CertificateDer::from(self.spki.clone());
        Ok(Arc::new(CertifiedKey::new(vec![cert], signing)))
    }
}
