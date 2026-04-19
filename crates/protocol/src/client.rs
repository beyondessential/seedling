use std::{net::SocketAddr, sync::Arc, time::Duration};

use super::keys::ClientIdentity;
use crate::actor::Actor;

use quinn::{ClientConfig, Connection, Endpoint, TransportConfig};
use rustls::{
    ClientConfig as TlsClientConfig, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
};
use rustls_pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ClientError {
    Connect(Box<dyn std::error::Error + Send + Sync>),
    Transport(Box<dyn std::error::Error + Send + Sync>),
    Protocol(String),
    Api { code: String, message: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "connection failed: {e}"),
            Self::Transport(e) => write!(f, "transport error: {e}"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
            Self::Api { code, message } => write!(f, "[{code}] {message}"),
        }
    }
}

impl std::error::Error for ClientError {}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum ClientAuth {
    /// Pin the server by the hex-encoded SHA-256 of its SPKI.
    Fingerprint(String),
    /// Accept any server key without verification (development only).
    TrustAny,
}

// ---------------------------------------------------------------------------
// Helpers shared between verifiers
// ---------------------------------------------------------------------------

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

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
    // In RPK mode cert contains the raw SPKI bytes, not an X.509 certificate.
    // verify_tls13_signature_with_raw_key extracts the public key from the SPKI
    // directly; the standard verify_tls13_signature would fail with BadEncoding
    // trying to parse the SPKI as X.509 via webpki.
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
// Fingerprint verifier
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FingerprintVerifier {
    expected: String,
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got = hex_digest(end_entity.as_ref());
        if got.as_bytes().ct_eq(self.expected.as_bytes()).into() {
            Ok(ServerCertVerified::assertion())
        } else {
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

// ---------------------------------------------------------------------------
// Trust-any verifier (dev/test only)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TrustAnyVerifier;

impl ServerCertVerifier for TrustAnyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
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

// ---------------------------------------------------------------------------
// Recording verifier — captures the fingerprint, accepts anything
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RecordingVerifier {
    cell: Arc<std::sync::OnceLock<String>>,
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let _ = self.cell.set(hex_digest(end_entity.as_ref()));
        Ok(ServerCertVerified::assertion())
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

// ---------------------------------------------------------------------------
// OiClient
// ---------------------------------------------------------------------------

pub struct OiClient {
    conn: Connection,
    actor: Actor,
}

impl OiClient {
    // i[wire.actor]
    pub async fn connect(
        addr: SocketAddr,
        auth: ClientAuth,
        identity: &ClientIdentity,
        actor: Actor,
    ) -> Result<Self, ClientError> {
        let verifier: Arc<dyn ServerCertVerifier> = match auth {
            ClientAuth::Fingerprint(fp) => Arc::new(FingerprintVerifier { expected: fp }),
            ClientAuth::TrustAny => Arc::new(TrustAnyVerifier),
        };

        let mut tls_config = TlsClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(build_client_cert_resolver(identity)?);
        tls_config.key_log = Arc::new(rustls::KeyLogFile::new());

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| ClientError::Connect(Box::new(e)))?;

        let mut transport = TransportConfig::default();
        transport.datagram_receive_buffer_size(Some(65536));
        let mut client_cfg = ClientConfig::new(Arc::new(quic_config));
        client_cfg.transport_config(Arc::new(transport));

        let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())
            .map_err(|e| ClientError::Connect(Box::new(e)))?;
        endpoint.set_default_client_config(client_cfg);

        let conn = tokio::time::timeout(
            Duration::from_secs(5),
            endpoint
                .connect(addr, "seedling")
                .map_err(|e| ClientError::Connect(Box::new(e)))?,
        )
        .await
        .map_err(|_| ClientError::Connect("connection timed out".into()))?
        .map_err(|e| ClientError::Connect(Box::new(e)))?;

        Ok(Self { conn, actor })
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Open a raw bidirectional stream.
    ///
    /// Used for shell sessions where the stream protocol differs from the
    /// standard request/response cycle of `request()`.
    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), ClientError> {
        self.conn
            .open_bi()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))
    }

    /// Accept an incoming server-initiated unidirectional stream.
    ///
    /// Used to receive the stdout and stderr streams opened by the server
    /// during a `/shells/start` session.
    pub async fn accept_uni(&self) -> Result<quinn::RecvStream, ClientError> {
        self.conn
            .accept_uni()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))
    }

    /// Send a QUIC datagram to the server.
    ///
    /// Used for UDP port-forward relay; the caller is responsible for prepending
    /// the 2-byte big-endian `forward_key` prefix.
    pub fn send_datagram(&self, data: Vec<u8>) -> Result<(), ClientError> {
        self.conn
            .send_datagram(data.into())
            .map_err(|e| ClientError::Transport(Box::new(e)))
    }

    /// Receive the next QUIC datagram from the server.
    ///
    /// Used for UDP port-forward relay; the returned bytes include the 2-byte
    /// big-endian `forward_key` prefix followed by the UDP payload.
    pub async fn read_datagram(&self) -> Result<Vec<u8>, ClientError> {
        self.conn
            .read_datagram()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ClientError::Transport(Box::new(e)))
    }

    /// Send a single JSON request and return the parsed result value.
    // i[wire.actor]
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let req_bytes = serde_json::to_vec(&serde_json::json!({
            "method": method,
            "actor": &self.actor,
            "params": params,
        }))
        .expect("request serialisation never fails");

        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        send.write_all(&req_bytes)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;
        send.finish()
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        let resp_bytes = recv
            .read_to_end(4 * 1024 * 1024)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Response {
            Ok { result: Value },
            Err { error: ApiError },
        }
        #[derive(serde::Deserialize)]
        struct ApiError {
            code: String,
            message: String,
        }

        match serde_json::from_slice::<Response>(&resp_bytes)
            .map_err(|e| ClientError::Protocol(format!("invalid response: {e}")))?
        {
            Response::Ok { result } => Ok(result),
            Response::Err { error } => Err(ClientError::Api {
                code: error.code,
                message: error.message,
            }),
        }
    }
}

impl OiClient {
    /// Probe the server's SPKI fingerprint without presenting client identity.
    ///
    /// Opens a non-mTLS connection to capture the server's raw public key
    /// fingerprint. The handshake will typically fail (the server requires
    /// client auth), but the `RecordingVerifier` captures the fingerprint
    /// during `verify_server_cert` before client auth is evaluated.
    ///
    /// The caller should verify the fingerprint against a known-hosts store
    /// and then open a full mTLS connection via [`connect`].
    pub async fn probe_fingerprint(addr: SocketAddr) -> Result<String, ClientError> {
        let cell = Arc::new(std::sync::OnceLock::new());
        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(RecordingVerifier {
            cell: Arc::clone(&cell),
        });

        // i[transport.fingerprint-probe]
        let ephemeral = super::keys::ClientIdentity::ephemeral();
        let mut tls_config = TlsClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(build_client_cert_resolver(&ephemeral)?);
        tls_config.key_log = Arc::new(rustls::KeyLogFile::new());

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| ClientError::Connect(Box::new(e)))?;

        let mut transport = TransportConfig::default();
        transport.datagram_receive_buffer_size(Some(65536));
        let mut client_cfg = ClientConfig::new(Arc::new(quic_config));
        client_cfg.transport_config(Arc::new(transport));

        let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())
            .map_err(|e| ClientError::Connect(Box::new(e)))?;
        endpoint.set_default_client_config(client_cfg);

        // The server requires mTLS, so this handshake will likely fail
        // because we do not present a client certificate. That is expected:
        // we only need the server's SPKI fingerprint, which the
        // RecordingVerifier captures during verify_server_cert (before
        // client auth is evaluated by the server).
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            endpoint
                .connect(addr, "seedling")
                .map_err(|e| ClientError::Connect(Box::new(e)))?,
        )
        .await;

        // Tear down the probe endpoint regardless of outcome.
        if let Ok(Ok(ref conn)) = result {
            conn.close(quinn::VarInt::from_u32(0), b"probe");
        }
        endpoint.close(quinn::VarInt::from_u32(0), b"probe");

        match cell.get() {
            Some(fp) => Ok(fp.clone()),
            None => match result {
                Ok(Err(e)) => Err(ClientError::Connect(Box::new(e))),
                Err(_) => Err(ClientError::Connect("connection timed out".into())),
                Ok(Ok(_)) => Err(ClientError::Protocol(
                    "handshake succeeded but no fingerprint was recorded".into(),
                )),
            },
        }
    }
}

fn build_client_cert_resolver(
    identity: &ClientIdentity,
) -> Result<Arc<dyn rustls::client::ResolvesClientCert>, ClientError> {
    let ck = identity.to_certified_key().map_err(ClientError::Connect)?;
    Ok(Arc::new(
        rustls::client::AlwaysResolvesClientRawPublicKeys::new(ck),
    ))
}
