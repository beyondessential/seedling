//! Parse PEM-encoded certificates and extract the metadata the runtime
//! exposes via the operator interface, plus utilities for validating
//! certificates supplied at upload time.

use snafu::Snafu;
use x509_parser::prelude::FromDer;

use super::store::CertMetadata;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("expected a CERTIFICATE PEM block, found none"))]
    NoCertBlock,

    #[snafu(display("PEM parse error: {message}"))]
    Pem { message: String },

    #[snafu(display("X.509 parse error: {message}"))]
    X509 { message: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Parsed view of a certificate chain. Only the leaf certificate's metadata
/// is surfaced; intermediates are kept around in [`Self::chain_pem`] so the
/// full chain can be served to clients.
#[derive(Debug, Clone)]
pub struct ParsedChain {
    pub metadata: CertMetadata,
    /// Full chain re-encoded as a single PEM blob, in leaf-first order.
    pub chain_pem: String,
    /// DNS names listed in the leaf's SubjectAlternativeName extension.
    pub san_dns_names: Vec<String>,
    /// Encoded leaf public key bytes (SubjectPublicKeyInfo, DER). Used by
    /// upload validation to confirm the supplied private key matches.
    pub leaf_spki_der: Vec<u8>,
}

/// Parse a PEM blob that may contain one or more CERTIFICATE entries
/// (leaf + intermediates). Returns metadata derived from the leaf.
pub fn parse_chain(pem: &str) -> Result<ParsedChain> {
    let mut blocks: Vec<pem::Pem> = pem::parse_many(pem.as_bytes())
        .map_err(|e| {
            PemSnafu {
                message: e.to_string(),
            }
            .build()
        })?
        .into_iter()
        .filter(|b| b.tag() == "CERTIFICATE")
        .collect();

    if blocks.is_empty() {
        return NoCertBlockSnafu.fail();
    }

    // Re-encode in canonical leaf-first PEM form.
    let mut chain_pem = String::new();
    for b in &blocks {
        chain_pem.push_str(&pem::encode(b));
    }

    let leaf = blocks.remove(0);
    let der = leaf.contents();
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(der).map_err(|e| {
        X509Snafu {
            message: e.to_string(),
        }
        .build()
    })?;

    let issuer = cert.issuer().to_string();
    let subject = cert.subject().to_string();
    let self_signed = issuer == subject && blocks.is_empty();
    let not_before = cert.validity().not_before.timestamp();
    let not_after = cert.validity().not_after.timestamp();
    let serial = cert.tbs_certificate.raw_serial_as_string();

    let mut san_dns_names = Vec::new();
    if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
        for name in &san_ext.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
                san_dns_names.push((*dns).to_owned());
            }
        }
    }

    let leaf_spki_der = cert.tbs_certificate.subject_pki.raw.to_vec();

    Ok(ParsedChain {
        metadata: CertMetadata {
            issuer: Some(issuer),
            not_before: Some(not_before),
            not_after: Some(not_after),
            serial: Some(serial),
            self_signed,
        },
        chain_pem,
        san_dns_names,
        leaf_spki_der,
    })
}

/// Returns true if any DNS name in `sans` covers `hostname`. Wildcard rules
/// per RFC 6125: `*.example.com` covers exactly one extra left-most label,
/// matches `foo.example.com` but not `example.com` and not `a.b.example.com`.
// r[impl tls.cert.validation.san-coverage]
pub fn san_covers(sans: &[String], hostname: &str) -> bool {
    let host_lc = hostname.to_ascii_lowercase();
    for san in sans {
        let san_lc = san.to_ascii_lowercase();
        if san_lc == host_lc {
            return true;
        }
        if let Some(rest) = san_lc.strip_prefix("*.") {
            // Exactly one extra label on the left.
            if let Some((first, host_rest)) = host_lc.split_once('.')
                && !first.is_empty()
                && host_rest == rest
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tls::keypair;

    fn self_signed_cert(host: &str) -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("keypair");
        let mut params = rcgen::CertificateParams::new(vec![host.to_owned()]).expect("params");
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params.self_signed(&key).expect("self-sign");
        cert.pem()
    }

    #[test]
    fn parse_chain_extracts_metadata() {
        let pem = self_signed_cert("foo.example.com");
        let parsed = parse_chain(&pem).unwrap();
        assert!(parsed.metadata.issuer.is_some());
        assert!(parsed.metadata.not_after.unwrap() > parsed.metadata.not_before.unwrap());
        assert!(parsed.metadata.self_signed);
        assert!(parsed.san_dns_names.iter().any(|n| n == "foo.example.com"));
        assert!(!parsed.leaf_spki_der.is_empty());
    }

    #[test]
    fn parse_chain_rejects_non_certificate_pem() {
        let key = keypair::generate(super::super::KeyType::EcdsaP256).unwrap();
        let err = parse_chain(secrecy::ExposeSecret::expose_secret(&key.pem)).unwrap_err();
        assert!(matches!(err, Error::NoCertBlock));
    }

    #[test]
    fn san_covers_exact_match() {
        let sans = vec!["foo.example.com".to_owned()];
        assert!(san_covers(&sans, "foo.example.com"));
        assert!(san_covers(&sans, "FOO.EXAMPLE.COM"));
    }

    #[test]
    fn san_covers_wildcard_one_label() {
        let sans = vec!["*.example.com".to_owned()];
        assert!(san_covers(&sans, "foo.example.com"));
        assert!(san_covers(&sans, "FOO.example.com"));
        assert!(!san_covers(&sans, "example.com"));
        assert!(!san_covers(&sans, "a.b.example.com"));
    }

    #[test]
    fn san_covers_returns_false_when_unrelated() {
        let sans = vec!["foo.example.com".to_owned(), "*.other.com".to_owned()];
        assert!(!san_covers(&sans, "bar.example.com"));
        assert!(!san_covers(&sans, "example.com"));
    }
}
