use std::{io, path::Path, sync::Arc};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use rand::rngs::ThreadRng;
use rustls::{crypto::ring::sign, sign::CertifiedKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// Load an Ed25519 signing key from a PKCS#8 DER file, or generate and
/// persist one if the file does not exist.
// r[infra.key.file-permissions]
// i[key.client.file-permissions]
pub fn load_or_generate(path: &Path) -> io::Result<SigningKey> {
    if path.exists() {
        #[cfg(unix)]
        {
            let mode = path.metadata()?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "key file {} has insecure permissions (0{:o}); expected 0600",
                        path.display(),
                        mode
                    ),
                ));
            }
        }
        let der = std::fs::read(path)?;
        SigningKey::from_pkcs8_der(&der).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let key = SigningKey::generate(&mut ThreadRng::default());
        let doc = key.to_pkcs8_der().map_err(io::Error::other)?;
        {
            use std::io::Write;
            #[cfg(unix)]
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?
                .write_all(doc.as_bytes())?;
            #[cfg(not(unix))]
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(doc.as_bytes())?;
        }
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

/// Why an operator-supplied fingerprint was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidFingerprint {
    /// Not 64 hex characters once normalised.
    NotSha256Hex,
}

impl std::fmt::Display for InvalidFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSha256Hex => f.write_str(
                "fingerprint must be a SHA-256 digest: 64 hex characters, \
                 optionally prefixed `sha256:`",
            ),
        }
    }
}

impl std::error::Error for InvalidFingerprint {}

/// Canonicalise an operator-supplied fingerprint into the form
/// [`fingerprint`] produces, rejecting anything that could never match one.
///
/// Fingerprints reach the runtime as free text — a CLI flag, a config file, a
/// paste from another tool — and comparison against a real client is
/// byte-for-byte. So a value that is merely *shaped* wrong is not a near
/// miss: it is a key that will never authenticate, and storing it as
/// authorised tells an operator they have granted access they have not.
pub fn parse_fingerprint(s: &str) -> Result<String, InvalidFingerprint> {
    let trimmed = s.trim();
    let body = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    if body.len() != 64 || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(InvalidFingerprint::NotSha256Hex);
    }
    Ok(body.to_ascii_lowercase())
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
    /// Generate a fresh key in memory without persisting it.
    ///
    /// Used for fingerprint probe connections where the client must present
    /// an RPK client certificate but must not reveal its real identity.
    pub fn ephemeral() -> Self {
        let key = SigningKey::generate(&mut ThreadRng::default());
        let spki = spki_der(&key);
        let fp = fingerprint(&spki);
        Self {
            signing_key: key,
            spki,
            fingerprint: fp,
        }
    }

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

#[cfg(all(test, unix))]
mod tests {

    use super::{InvalidFingerprint, parse_fingerprint};

    // i[verify key.authorize]
    #[test]
    fn a_canonical_fingerprint_parses_unchanged() {
        let fp = "a".repeat(64);
        assert_eq!(parse_fingerprint(&fp), Ok(fp.clone()));
    }

    // i[verify key.authorize]
    // Comparison against a real client is byte-for-byte, so these all had to
    // be normalised rather than stored as given.
    #[test]
    fn case_whitespace_and_a_sha256_prefix_are_normalised() {
        let fp = "a".repeat(64);
        assert_eq!(parse_fingerprint(&fp.to_uppercase()), Ok(fp.clone()));
        assert_eq!(parse_fingerprint(&format!("  {fp}\n")), Ok(fp.clone()));
        assert_eq!(parse_fingerprint(&format!("sha256:{fp}")), Ok(fp.clone()));
        assert_eq!(
            parse_fingerprint(&format!("SHA256:{}", fp.to_uppercase())),
            Ok(fp)
        );
    }

    // i[verify key.authorize]
    // A wrong-shaped value is not a near miss: it is a key that can never
    // authenticate, and storing it says access was granted when it was not.
    #[test]
    fn a_value_that_could_never_match_is_refused() {
        for bad in [
            "",
            "sha256:",
            &"a".repeat(63),
            &"a".repeat(65),
            &"z".repeat(64),
            "not a fingerprint",
        ] {
            assert_eq!(
                parse_fingerprint(bad),
                Err(InvalidFingerprint::NotSha256Hex),
                "should refuse {bad:?}"
            );
        }
    }
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    // r[verify infra.key.file-permissions]
    // i[verify key.client.file-permissions]
    #[test]
    fn load_or_generate_creates_key_with_owner_only_perms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("client.key");
        let _ = load_or_generate(&path).expect("create key");
        let mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "new key file should be 0600, got 0{mode:o}");
    }

    // r[verify infra.key.file-permissions]
    // i[verify key.client.file-permissions]
    #[test]
    fn load_or_generate_rejects_existing_key_with_group_perms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("loose.key");
        // Seed the file first with acceptable mode, then relax the mode.
        let _ = load_or_generate(&path).expect("first create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = load_or_generate(&path).expect_err("should refuse 0644 mode");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn load_or_generate_roundtrips_key_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rt.key");
        let k1 = load_or_generate(&path).expect("create");
        let k2 = load_or_generate(&path).expect("reload");
        assert_eq!(k1.to_bytes(), k2.to_bytes());
    }
}
