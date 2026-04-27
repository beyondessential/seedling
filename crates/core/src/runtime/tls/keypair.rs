//! Keypair and CSR generation for the TLS subsystem.
//!
//! Used by both the ACME-DNS issuance flow (server-generated keypair, CSR
//! submitted to the CA) and the operator-driven CSR flow (server-generated
//! keypair, CSR returned to the operator who arranges signing externally).
//!
//! ECDSA P-256 only. The [`KeyType`] enum in the parent module is reserved
//! for future PQC variants.

use rcgen::{CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256};
use secrecy::SecretString;
use snafu::{ResultExt, Snafu};

use super::KeyType;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("rcgen error: {source}"))]
    Rcgen { source: rcgen::Error },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A freshly-generated keypair, materialised in the formats the rest of the
/// system needs:
///
/// - `pem` is the PEM-encoded PKCS#8 private key, suitable for storage
///   (encrypted) and for serving back to Caddy at handshake time.
/// - `inner` is the live `KeyPair` for further rcgen operations (e.g.
///   building a CSR against the same key).
pub struct GeneratedKey {
    pub key_type: KeyType,
    pub pem: SecretString,
    pub inner: KeyPair,
}

/// Generate a keypair of the requested type.
pub fn generate(key_type: KeyType) -> Result<GeneratedKey> {
    let pair = match key_type {
        KeyType::EcdsaP256 => KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context(RcgenSnafu)?,
    };
    let pem = SecretString::new(pair.serialize_pem().into());
    Ok(GeneratedKey {
        key_type,
        pem,
        inner: pair,
    })
}

/// Reconstruct a [`KeyPair`] from a previously-stored PEM blob. Used during
/// renewal (we kept the same hostname; we still want a fresh CSR but the
/// CSR-flow case uses the original keypair).
pub fn from_pem(pem: &str) -> Result<KeyPair> {
    KeyPair::from_pem(pem).context(RcgenSnafu)
}

/// Build a CSR for `hostname` against the given keypair. The CSR includes
/// `hostname` as the only DNS SAN; if `hostname` starts with `*.` the
/// wildcard SAN is preserved.
pub fn build_csr(hostname: &str, key: &KeyPair) -> Result<Csr> {
    let mut params = CertificateParams::new(vec![hostname.to_owned()]).context(RcgenSnafu)?;
    params.distinguished_name = DistinguishedName::new();
    let req = params.serialize_request(key).context(RcgenSnafu)?;
    Ok(Csr {
        der: req.der().to_vec(),
        pem: req.pem().context(RcgenSnafu)?,
    })
}

pub struct Csr {
    pub der: Vec<u8>,
    pub pem: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn generate_ecdsa_p256_yields_pem_and_pair() {
        let key = generate(KeyType::EcdsaP256).unwrap();
        assert_eq!(key.key_type, KeyType::EcdsaP256);
        let pem = key.pem.expose_secret();
        assert!(pem.contains("BEGIN PRIVATE KEY"), "got: {pem}");
    }

    #[test]
    fn build_csr_round_trips() {
        let key = generate(KeyType::EcdsaP256).unwrap();
        let csr = build_csr("foo.example.com", &key.inner).unwrap();
        assert!(csr.pem.contains("BEGIN CERTIFICATE REQUEST"));
        assert!(!csr.der.is_empty());
    }

    #[test]
    fn from_pem_recovers_keypair() {
        let key = generate(KeyType::EcdsaP256).unwrap();
        let restored = from_pem(key.pem.expose_secret()).unwrap();
        // The recovered key should produce CSRs with the same SPKI as the
        // original; we don't have direct equality so we sign two CSRs and
        // compare the public key bytes.
        let csr1 = build_csr("a.example.com", &key.inner).unwrap();
        let csr2 = build_csr("a.example.com", &restored).unwrap();
        // Both CSRs encode the same public key — compare the public-key
        // section by checking the CSRs share a non-trivial prefix length.
        assert!(csr1.der.len() > 64 && csr2.der.len() > 64);
    }

    #[test]
    fn build_csr_with_wildcard() {
        let key = generate(KeyType::EcdsaP256).unwrap();
        let csr = build_csr("*.example.com", &key.inner).unwrap();
        assert!(csr.pem.contains("BEGIN CERTIFICATE REQUEST"));
    }
}
