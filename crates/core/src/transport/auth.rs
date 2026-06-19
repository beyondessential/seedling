//! Transport-layer authentication: the trust-set machinery used by the
//! TLS client certificate verifier across all ALPNs registered on the
//! shared QUIC endpoint.
//!
//! Trust is **protocol-scoped**: each ALPN registers its own trusted-keys
//! set with [`ProtocolTrustRegistry`]. A key authorised for one ALPN does
//! not authorise others — the post-handshake gate in
//! [`crate::transport::endpoint`] enforces this against the negotiated
//! ALPN before the registered handler is invoked.
//!
//! At TLS handshake time the verifier accepts any key trusted by **any**
//! registered protocol, because rustls's `ClientCertVerifier` API does
//! not expose the negotiated ALPN at cert-verification time. Per-protocol
//! authorisation is then enforced post-handshake.

use std::{collections::HashSet, sync::Arc};

use parking_lot::RwLock;
use rustls::{
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
    client::danger::HandshakeSignatureValid,
    server::danger::{ClientCertVerified, ClientCertVerifier},
};
use rustls_pki_types::{CertificateDer, SubjectPublicKeyInfoDer, UnixTime};
use subtle::ConstantTimeEq;

use seedling_protocol::keys;

/// Thread-safe in-memory set of trusted SPKI fingerprints for one protocol.
///
/// Mutations are reflected in subsequent TLS handshakes immediately.
pub type TrustedKeys = Arc<RwLock<HashSet<String>>>;

pub fn new_trusted_keys() -> TrustedKeys {
    Arc::new(RwLock::new(HashSet::new()))
}

/// Registry mapping each registered ALPN to its trusted-keys set.
///
/// Built once at daemon startup. Shared with the TLS verifier (which
/// consults the union for handshake admission) and with the post-handshake
/// gate (which checks the negotiated ALPN's set).
#[derive(Debug, Default)]
pub struct ProtocolTrustRegistry {
    inner: RwLock<Vec<(Vec<u8>, TrustedKeys)>>,
}

impl ProtocolTrustRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register `trust` as the trusted-keys set for `alpn`. Replaces any
    /// existing registration for the same ALPN.
    pub fn register(&self, alpn: &[u8], trust: TrustedKeys) {
        let mut inner = self.inner.write();
        if let Some(slot) = inner.iter_mut().find(|(a, _)| a == alpn) {
            slot.1 = trust;
        } else {
            inner.push((alpn.to_vec(), trust));
        }
    }

    /// True if `fp` is trusted for the given negotiated ALPN.
    pub fn is_trusted_for(&self, alpn: &[u8], fp: &str) -> bool {
        let inner = self.inner.read();
        let fp_bytes = fp.as_bytes();
        inner.iter().any(|(a, t)| {
            a == alpn
                && t.read()
                    .iter()
                    .any(|trusted| trusted.as_bytes().ct_eq(fp_bytes).into())
        })
    }

    /// True if `fp` is trusted by any registered protocol. Used at TLS
    /// handshake time when the negotiated ALPN is not yet visible to
    /// rustls's `ClientCertVerifier`.
    pub fn is_trusted_any(&self, fp: &str) -> bool {
        let inner = self.inner.read();
        let fp_bytes = fp.as_bytes();
        inner.iter().any(|(_, t)| {
            t.read()
                .iter()
                .any(|trusted| trusted.as_bytes().ct_eq(fp_bytes).into())
        })
    }

    /// ALPN identifiers in registration order, suitable for
    /// `tls_config.alpn_protocols`.
    pub fn alpn_list(&self) -> Vec<Vec<u8>> {
        self.inner.read().iter().map(|(a, _)| a.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// rustls signature verification helpers
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

// ---------------------------------------------------------------------------
// Client certificate verifier
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SeedlingClientVerifier {
    pub registry: Arc<ProtocolTrustRegistry>,
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
        if self.registry.is_trusted_any(&fp) {
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

    fn fps(reg: &ProtocolTrustRegistry, alpn: &[u8]) -> bool {
        reg.is_trusted_for(alpn, "fp-1")
    }

    #[test]
    fn register_and_lookup_per_alpn() {
        let reg = ProtocolTrustRegistry::default();
        let oi = new_trusted_keys();
        oi.write().insert("fp-1".to_owned());
        reg.register(b"oi/1", oi);
        assert!(fps(&reg, b"oi/1"));
        assert!(!fps(&reg, b"grove/1"));
        assert!(reg.is_trusted_any("fp-1"));
        assert!(!reg.is_trusted_any("fp-other"));
    }

    #[test]
    fn registering_same_alpn_replaces() {
        let reg = ProtocolTrustRegistry::default();
        let first = new_trusted_keys();
        first.write().insert("fp-old".to_owned());
        reg.register(b"oi/1", first);

        let second = new_trusted_keys();
        second.write().insert("fp-new".to_owned());
        reg.register(b"oi/1", second);

        assert!(reg.is_trusted_for(b"oi/1", "fp-new"));
        assert!(!reg.is_trusted_for(b"oi/1", "fp-old"));
        assert_eq!(reg.alpn_list(), vec![b"oi/1".to_vec()]);
    }

    #[test]
    fn alpn_list_preserves_registration_order() {
        let reg = ProtocolTrustRegistry::default();
        reg.register(b"oi/1", new_trusted_keys());
        reg.register(b"grove/1", new_trusted_keys());
        assert_eq!(reg.alpn_list(), vec![b"oi/1".to_vec(), b"grove/1".to_vec()]);
    }

    #[test]
    fn protocol_scoped_keys_do_not_leak_across_alpns() {
        let reg = ProtocolTrustRegistry::default();
        let oi = new_trusted_keys();
        oi.write().insert("operator-fp".to_owned());
        reg.register(b"oi/1", oi);

        let grove = new_trusted_keys();
        grove.write().insert("grove-member-fp".to_owned());
        reg.register(b"grove/1", grove);

        assert!(reg.is_trusted_for(b"oi/1", "operator-fp"));
        assert!(!reg.is_trusted_for(b"grove/1", "operator-fp"));
        assert!(reg.is_trusted_for(b"grove/1", "grove-member-fp"));
        assert!(!reg.is_trusted_for(b"oi/1", "grove-member-fp"));
        // But the handshake-time check accepts either, since rustls cannot
        // see the negotiated ALPN when verify_client_cert runs.
        assert!(reg.is_trusted_any("operator-fp"));
        assert!(reg.is_trusted_any("grove-member-fp"));
    }
}
