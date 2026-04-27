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
    /// Number of CERTIFICATE PEM blocks the chain contained, leaf
    /// included. Useful for diagnostics when a confusion-of-which-cert-
    /// is-the-leaf is suspected.
    pub chain_len: usize,
    /// Leaf certificate's Subject DN. Empty string when the cert has no
    /// Subject DN (legitimate when SAN is the only identifier).
    pub leaf_subject: String,
    /// DNS names listed in the leaf's SubjectAlternativeName extension.
    pub san_dns_names: Vec<String>,
    /// Encoded leaf public key bytes (SubjectPublicKeyInfo, DER). Used by
    /// upload validation to confirm the supplied private key matches.
    pub leaf_spki_der: Vec<u8>,
    /// DER-encoded `keyIdentifier` octet string from the leaf's
    /// `AuthorityKeyIdentifier` extension, when present. Required (with
    /// the serial) to construct the RFC 9773 cert identifier the CA needs
    /// for ARI lookups and `replaces` on renewal.
    // r[impl tls.cert.ari]
    pub leaf_aki_der: Option<Vec<u8>>,
    /// DER-encoded ASN.1 INTEGER serial number from the leaf certificate.
    /// (Note: this is the *encoded* form, not a big-integer representation.)
    // r[impl tls.cert.ari]
    pub leaf_serial_der: Vec<u8>,
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
    let chain_len = blocks.len();

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

    // Walk parsed extensions and pull both the SAN DNS names and the
    // AKI keyIdentifier in one pass. Using the parsed-extension stream
    // (rather than `subject_alternative_name()`) avoids x509-parser's
    // duplicate-extension error path silently masking a present SAN —
    // we just take whichever SAN extension we find first.
    let mut san_dns_names = Vec::new();
    // r[impl tls.cert.ari]
    // AKI keyIdentifier octet-string contents (not the TLV wrapper);
    // RFC 9773 § 4.1 takes those bytes base64url-encoded.
    let mut leaf_aki_der = None;
    for ext in cert.extensions() {
        match ext.parsed_extension() {
            x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) => {
                for name in &san.general_names {
                    if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
                        san_dns_names.push((*dns).to_owned());
                    }
                }
            }
            x509_parser::extensions::ParsedExtension::AuthorityKeyIdentifier(aki) => {
                if let Some(kid) = &aki.key_identifier {
                    leaf_aki_der = Some(kid.0.to_vec());
                }
            }
            _ => {}
        }
    }

    let leaf_spki_der = cert.tbs_certificate.subject_pki.raw.to_vec();
    let leaf_serial_der = cert.tbs_certificate.raw_serial().to_vec();

    Ok(ParsedChain {
        metadata: CertMetadata {
            issuer: Some(issuer),
            not_before: Some(not_before),
            not_after: Some(not_after),
            serial: Some(serial),
            self_signed,
        },
        chain_pem,
        chain_len,
        leaf_subject: subject,
        san_dns_names,
        leaf_spki_der,
        leaf_aki_der,
        leaf_serial_der,
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

    /// Caddy's internal CA emits leaf certs with an empty Subject DN
    /// and only a SAN extension (often marked `critical`). Earlier code
    /// used `cert.subject_alternative_name()`, which can return Err
    /// rather than Ok(Some(...)) under edge cases — silently skipping
    /// the SAN list. Walking parsed extensions directly avoids that.
    #[test]
    fn parse_chain_extracts_san_from_caddy_local_leaf() {
        // Inline cert produced by Caddy's local CA: empty Subject DN,
        // critical SAN with one DNS name. The chain has both leaf and
        // intermediate so chain_len is exercised too.
        let pem = "-----BEGIN CERTIFICATE-----\n\
MIIBxzCCAW2gAwIBAgIRAPGljHBbVFm+j2zx72VpwHEwCgYIKoZIzj0EAwIwMzEx\n\
MC8GA1UEAxMoQ2FkZHkgTG9jYWwgQXV0aG9yaXR5IC0gRUNDIEludGVybWVkaWF0\n\
ZTAeFw0yNjA0MjAwNjQ2NTBaFw0yNjA0MjAxODQ2NTBaMAAwWTATBgcqhkjOPQIB\n\
BggqhkjOPQMBBwNCAATokbf3b4r3pH/IoIF6GJluMgqyfyXcL0hiojvqJ2X6W4s/\n\
1s5rqTO+L2P43miE3b1p0mDJM1F2F/9XFBagbALko4GUMIGRMA4GA1UdDwEB/wQE\n\
AwIHgDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFC3Q\n\
E8HWS8oP8Coe4l9p4OfqI/vpMB8GA1UdIwQYMBaAFA8JgliWBqb9BV249zkEQWqk\n\
SeORMCAGA1UdEQEB/wQWMBSCEnNlcnZlZGlyLmxvY2FsaG9zdDAKBggqhkjOPQQD\n\
AgNIADBFAiEA/OJ3FbDV3w9GaJ+ubLjOjUiMJSAwkS9dzgNKKZe2hw0CICzS2SZE\n\
06F+x1VFAWjMf1r6Qk1oTBhteiITc/UMqgT3\n\
-----END CERTIFICATE-----\n\
-----BEGIN CERTIFICATE-----\n\
MIIBxzCCAW2gAwIBAgIQL4sgwtfrews18bAJQR0pwDAKBggqhkjOPQQDAjAwMS4w\n\
LAYDVQQDEyVDYWRkeSBMb2NhbCBBdXRob3JpdHkgLSAyMDI2IEVDQyBSb290MB4X\n\
DTI2MDQyMDAyMzYxN1oXDTI2MDQyNzAyMzYxN1owMzExMC8GA1UEAxMoQ2FkZHkg\n\
TG9jYWwgQXV0aG9yaXR5IC0gRUNDIEludGVybWVkaWF0ZTBZMBMGByqGSM49AgEG\n\
CCqGSM49AwEHA0IABLuQVB998laF1CVXgLv0YVQFmnXjEcwRad/iD7ie5CCKh+38\n\
l7wMQ5E+4C+oNcFHBMTC+U5ECBGGhfJXIs+uRQWjZjBkMA4GA1UdDwEB/wQEAwIB\n\
BjASBgNVHRMBAf8ECDAGAQH/AgEAMB0GA1UdDgQWBBQPCYJYlgam/QVduPc5BEFq\n\
pEnjkTAfBgNVHSMEGDAWgBQp1FCK3DvDcBoOgxrSP1qgdAlt7zAKBggqhkjOPQQD\n\
AgNJADBGAiEAo4OD1uvy0g1CpJXGq6DkyLXfL75gMrBVZQGJqFPg00ICIQDTuv9F\n\
TAdkb2FptCcYpxytQuVaRq79ihx0VPxbWzcBYg==\n\
-----END CERTIFICATE-----\n";
        let parsed = parse_chain(pem).expect("parse");
        assert_eq!(parsed.chain_len, 2);
        assert_eq!(parsed.san_dns_names, vec!["servedir.localhost".to_owned()]);
        assert_eq!(parsed.leaf_subject, "");
        assert!(
            parsed
                .metadata
                .issuer
                .as_deref()
                .unwrap()
                .contains("ECC Intermediate")
        );
    }
}
