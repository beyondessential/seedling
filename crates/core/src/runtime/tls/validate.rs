//! Validation rules for operator-supplied certificates.
//!
//! Used by the manual-upload (`tls.cert.upload-manual`) and CSR-cert-upload
//! (`tls.cert.csr.upload-cert`) flows. The rules come from the spec block
//! `tls.cert.validation.*` in `docs/spec/runtime.md`:
//!
//! - **SAN list non-empty**: the cert must carry at least one DNS SAN,
//!   otherwise it cannot ever auto-bind to a hostname.
//! - **Self-signed**: accepted with a warning so the operator interface
//!   can flag it.
//! - **Expired**: outright rejected when `not_after` is in the past.
//! - **Not yet valid**: accepted with a warning.
//! - **Key match**: the supplied private key's SubjectPublicKeyInfo
//!   must equal the leaf certificate's SPKI. (For CSR uploads the
//!   stored CSR keypair plays the role of the supplied key.)
//!
//! The cert auto-binds to whatever hostnames its SANs cover at
//! resolution time (see [`super::store::find_active_for_hostname`]);
//! validation does not pin it to any one hostname.
//!
//! Each function returns a [`Validated`] with the parsed metadata and
//! the list of warnings, or a [`ValidateError`] describing the precise
//! reason for rejection.

use jiff::Timestamp;
use rcgen::PublicKeyData;
use secrecy::{ExposeSecret, SecretString};
use snafu::{ResultExt, Snafu};

use super::{KeyType, keypair, parse};

#[derive(Debug, Snafu)]
pub enum ValidateError {
    #[snafu(display("certificate parse error: {source}"))]
    ParseCert { source: parse::Error },

    #[snafu(display("private key parse error: {source}"))]
    ParseKey { source: keypair::Error },

    #[snafu(display(
        "certificate has no DNS SANs; nothing to bind to (leaf subject={subject:?}, issuer={issuer:?})"
    ))]
    NoSans {
        subject: String,
        issuer: Option<String>,
    },

    #[snafu(display("certificate has already expired (notAfter = {not_after}, now = {now})"))]
    Expired { not_after: i64, now: i64 },

    #[snafu(display("supplied private key does not match the certificate's public key"))]
    KeyMismatch,
}

pub type Result<T, E = ValidateError> = std::result::Result<T, E>;

/// Successful validation result. Carries the parsed leaf metadata (so the
/// OI handler can persist it without re-parsing) plus any non-fatal
/// `warnings` the operator should be told about.
#[derive(Debug)]
pub struct Validated {
    pub parsed: parse::ParsedChain,
    pub warnings: Vec<&'static str>,
    /// Key type detected from the supplied private key. Always
    /// [`KeyType::EcdsaP256`] today; widening this enum is the path
    /// for future PQC support.
    pub key_type: KeyType,
}

/// Validate an operator-supplied cert+key pair. The cert auto-binds to
/// whatever hostnames its SANs cover at resolution time, so validation
/// does not take a hostname parameter; instead it asserts that the SAN
/// list is non-empty (otherwise the cert can never bind to anything).
// r[impl tls.cert.validation.self-signed]
// r[impl tls.cert.validation.expired]
pub fn validate_upload(cert_pem: &str, key_pem: &SecretString) -> Result<Validated> {
    let parsed = parse::parse_chain(cert_pem).context(ParseCertSnafu)?;

    if parsed.san_dns_names.is_empty() {
        return NoSansSnafu {
            subject: parsed.leaf_subject.clone(),
            issuer: parsed.metadata.issuer.clone(),
        }
        .fail();
    }

    // Validity window. Already-expired certs are rejected; a not-yet-valid
    // cert is accepted with a warning so the operator can stage uploads
    // ahead of cutover.
    let now = Timestamp::now().as_second();
    if let Some(not_after) = parsed.metadata.not_after
        && not_after < now
    {
        return ExpiredSnafu { not_after, now }.fail();
    }

    let mut warnings = Vec::new();
    if parsed.metadata.self_signed {
        warnings.push("self_signed");
    }
    if let Some(not_before) = parsed.metadata.not_before
        && not_before > now
    {
        warnings.push("not_yet_valid");
    }

    // Key match.
    let key = keypair::from_pem(key_pem.expose_secret()).context(ParseKeySnafu)?;
    let key_spki = key.subject_public_key_info();
    if key_spki != parsed.leaf_spki_der {
        return KeyMismatchSnafu.fail();
    }

    // Today rcgen only hands us ECDSA-P256 keys (it's the only algorithm
    // exposed via `KeyType`). When more variants land, key.is_compatible
    // will let us classify by detected algorithm.
    let key_type = KeyType::EcdsaP256;

    Ok(Validated {
        parsed,
        warnings,
        key_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256};
    use secrecy::SecretString;

    fn key_and_cert(
        host: &str,
        params_tweak: impl FnOnce(&mut CertificateParams),
    ) -> (String, SecretString) {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("keypair");
        let mut params = CertificateParams::new(vec![host.to_owned()]).expect("params");
        params.distinguished_name = DistinguishedName::new();
        params_tweak(&mut params);
        let cert = params.self_signed(&key).expect("self-sign");
        (cert.pem(), SecretString::new(key.serialize_pem().into()))
    }

    fn make(host: &str) -> (String, SecretString) {
        key_and_cert(host, |_| {})
    }

    #[test]
    fn accepts_self_signed_with_warning() {
        let (cert, key) = make("foo.example.com");
        let v = validate_upload(&cert, &key).unwrap();
        assert!(v.warnings.contains(&"self_signed"));
        assert!(!v.parsed.san_dns_names.is_empty());
    }

    #[test]
    fn accepts_wildcard_san() {
        let (cert, key) = make("*.example.com");
        let v = validate_upload(&cert, &key).unwrap();
        assert!(v.parsed.san_dns_names.iter().any(|s| s == "*.example.com"));
    }

    fn odt(seconds: i64) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(seconds).expect("in-range timestamp")
    }

    #[test]
    fn rejects_expired_cert() {
        let (cert, key) = key_and_cert("foo.example.com", |p| {
            // rcgen `not_after` defaults to now + ~1y; force a past one.
            p.not_before = odt(1);
            p.not_after = odt(2);
        });
        let err = validate_upload(&cert, &key).unwrap_err();
        assert!(matches!(err, ValidateError::Expired { .. }), "{err:?}");
    }

    #[test]
    fn flags_not_yet_valid() {
        let future = jiff::Timestamp::now().as_second() + 7 * 86400;
        let later = future + 30 * 86400;
        let (cert, key) = key_and_cert("foo.example.com", |p| {
            p.not_before = odt(future);
            p.not_after = odt(later);
        });
        let v = validate_upload(&cert, &key).unwrap();
        assert!(v.warnings.contains(&"not_yet_valid"));
    }

    #[test]
    fn rejects_key_mismatch() {
        let (cert, _key) = make("foo.example.com");
        let other_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let other_pem = SecretString::new(other_key.serialize_pem().into());
        let err = validate_upload(&cert, &other_pem).unwrap_err();
        assert!(matches!(err, ValidateError::KeyMismatch), "{err:?}");
    }

    #[test]
    fn rejects_garbage_pem() {
        let (cert, _key) = make("foo.example.com");
        let bad_key = SecretString::new("not a pem file".to_owned().into());
        let err = validate_upload(&cert, &bad_key).unwrap_err();
        assert!(matches!(err, ValidateError::ParseKey { .. }), "{err:?}");

        let bad_cert = "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n";
        let key_pem = SecretString::new("dummy".to_owned().into());
        let err = validate_upload(bad_cert, &key_pem).unwrap_err();
        assert!(matches!(err, ValidateError::ParseCert { .. }), "{err:?}");
    }
}
