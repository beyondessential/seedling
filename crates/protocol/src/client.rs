use std::{net::SocketAddr, sync::Arc, time::Duration};

use super::keys::ClientIdentity;
use crate::OI_ALPN;
use crate::actor::Actor;

use quinn::{ClientConfig, Connection, Endpoint, RecvStream, TransportConfig};
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

/// How long a client waits for the data stream after the server has accepted a
/// subscription.
const SUBSCRIBE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on a subscription's response envelope. Subscription responses
/// are `{"result":{}}` or a short error, so this is generous.
const RESPONSE_LIMIT: usize = 64 * 1024;

/// Upper bound on an ordinary request's response body, which may carry
/// listings and script text.
const REQUEST_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;

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
        Self::connect_from(addr, "[::]:0".parse().unwrap(), auth, identity, actor).await
    }

    /// [`Self::connect`] with the local socket address made explicit.
    ///
    /// Production always binds the dual-stack wildcard; tests bind an IPv4
    /// wildcard so they run on hosts without IPv6.
    async fn connect_from(
        addr: SocketAddr,
        bind: SocketAddr,
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
        // i[transport.alpn]
        tls_config.alpn_protocols = vec![OI_ALPN.to_vec()];

        let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| ClientError::Connect(Box::new(e)))?;

        let mut transport = TransportConfig::default();
        transport.datagram_receive_buffer_size(Some(65536));
        let mut client_cfg = ClientConfig::new(Arc::new(quic_config));
        client_cfg.transport_config(Arc::new(transport));

        let mut endpoint = Endpoint::client(bind).map_err(|e| ClientError::Connect(Box::new(e)))?;
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

    pub fn connection(&self) -> &quinn::Connection {
        &self.conn
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

    // i[stream.dispatch.server]
    /// Accept an incoming server-initiated bidirectional stream.
    ///
    /// The server opens these to push work to a client that has offered to do
    /// it — today, a Canopy relay request. Every such stream opens with a
    /// newline-terminated JSON object naming its kind, so a client that accepts
    /// them must dispatch on that object and reset streams whose kind it does
    /// not recognise.
    pub async fn accept_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), ClientError> {
        self.conn
            .accept_bi()
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

    /// Open a subscription-style request and return the server-initiated
    /// unidirectional stream carrying its data.
    ///
    /// This is the only correct way to drive `/events/subscribe` and
    /// `/logs/stream`: the response envelope on the bidirectional stream is
    /// read to FIN and classified before the unidirectional stream is awaited,
    /// so an error response surfaces as [`ClientError::Api`] instead of
    /// parking the caller on a stream the server will never open.
    // i[impl stream.subscribe]
    pub async fn open_subscription(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RecvStream, ClientError> {
        let req = serde_json::to_vec(&serde_json::json!({
            "method": method,
            "actor": &self.actor,
            "params": params,
        }))
        .expect("request serialisation never fails");
        self.open_subscription_raw(&req).await
    }

    /// As [`Self::open_subscription`], but sending a request whose envelope the
    /// caller has already serialised.
    ///
    /// Only for callers that must preserve an envelope built elsewhere — the
    /// web gateway relays the browser session's actor rather than its own.
    // i[impl stream.subscribe]
    pub async fn open_subscription_raw(&self, request: &[u8]) -> Result<RecvStream, ClientError> {
        self.open_subscription_within(request, SUBSCRIBE_HANDSHAKE_TIMEOUT)
            .await
    }

    /// The handshake itself, with the data-stream wait bounded by `timeout`.
    ///
    /// Only the timeout is parameterised, so tests can exercise the expiry
    /// without waiting out the production budget.
    async fn open_subscription_within(
        &self,
        request: &[u8],
        timeout: Duration,
    ) -> Result<RecvStream, ClientError> {
        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        send.write_all(request)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;
        send.finish()
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        // i[stream.control] — the FIN is the message boundary, so the envelope
        // is read to end rather than to a newline it does not carry.
        let body = recv
            .read_to_end(RESPONSE_LIMIT)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;
        Self::parse_response(&body)?;

        // A server that answered successfully can still fail to open the data
        // stream (its own failure path only logs), so the wait is bounded.
        tokio::time::timeout(timeout, self.conn.accept_uni())
            .await
            .map_err(|_| {
                ClientError::Protocol(
                    "server accepted the subscription but never opened the data stream".into(),
                )
            })?
            .map_err(|e| ClientError::Transport(Box::new(e)))
    }

    /// Subscribe to the server's event stream.
    ///
    /// Sends `/events/subscribe` and returns the server-initiated
    /// unidirectional stream the daemon opens to push newline-delimited JSON
    /// events.
    pub async fn subscribe_events(&self) -> Result<RecvStream, ClientError> {
        self.open_subscription("/events/subscribe", serde_json::json!({}))
            .await
    }

    /// Send a QUIC datagram to the server.
    ///
    /// Used for UDP port-forward relay; the caller is responsible for prepending
    /// the 2-byte big-endian `forward_key` prefix.
    /// Returns quinn's error unboxed, because the caller must distinguish
    /// `TooLarge` — which concerns one datagram — from `ConnectionLost`,
    /// which concerns the forward. Flattening them into one opaque error is
    /// what let a single oversized datagram terminate a forward.
    // i[impl forward.mtu]
    pub fn send_datagram(&self, data: Vec<u8>) -> Result<(), quinn::SendDatagramError> {
        self.conn.send_datagram(data.into())
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
            .read_to_end(REQUEST_RESPONSE_LIMIT)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        Self::parse_response(&resp_bytes)
    }

    /// Classify a response envelope read from a bidirectional stream.
    ///
    /// Shared by [`Self::request`] and [`Self::open_subscription_raw`] so that
    /// every consumer of the operator interface agrees on what an error, an
    /// unparseable body, and a stream that closed without a response mean.
    fn parse_response(bytes: &[u8]) -> Result<Value, ClientError> {
        if bytes.is_empty() {
            return Err(ClientError::Protocol(
                "server closed the stream without a response".into(),
            ));
        }

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

        match serde_json::from_slice::<Response>(bytes)
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
        // i[transport.alpn] — probe must be indistinguishable from a real connection.
        tls_config.alpn_protocols = vec![OI_ALPN.to_vec()];

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quinn::{Endpoint, ServerConfig};
    use rustls::{ServerConfig as TlsServerConfig, server::AlwaysResolvesServerRawPublicKeys};

    use super::*;
    use crate::keys::ClientIdentity;

    /// What the stub server does once it has read a subscription request.
    #[derive(Clone, Copy)]
    enum StubBehaviour {
        /// Answer with an error envelope and open no data stream — the
        /// `server_busy` / `requirements_invalid` / `not_found` branches.
        Error,
        /// Finish the response stream without writing anything.
        EmptyResponse,
        /// Answer `{"result":{}}` and then never open the uni stream — the
        /// server's own `open_uni` failure path, which only logs.
        OkThenNoUni,
        /// The full, correct handshake.
        OkThenUni,
    }

    /// Accept any client key, but negotiate the raw-public-key certificate
    /// type the real daemon does — without it the handshake fails before the
    /// behaviour under test is reached.
    #[derive(Debug)]
    struct AcceptAnyClientKey;

    impl rustls::server::danger::ClientCertVerifier for AcceptAnyClientKey {
        fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
            &[]
        }

        fn verify_client_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _now: UnixTime,
        ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
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

    /// Stand up a QUIC endpoint speaking the OI wire protocol that answers one
    /// subscription request per connection according to `behaviour`.
    fn spawn_stub_server(behaviour: StubBehaviour) -> SocketAddr {
        let identity = ClientIdentity::ephemeral();
        let resolver = Arc::new(AlwaysResolvesServerRawPublicKeys::new(
            identity.to_certified_key().expect("certified key"),
        ));
        let mut tls = TlsServerConfig::builder()
            .with_client_cert_verifier(Arc::new(AcceptAnyClientKey))
            .with_cert_resolver(resolver);
        tls.alpn_protocols = vec![OI_ALPN.to_vec()];
        let quic = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("quic config");
        let endpoint = Endpoint::server(
            ServerConfig::with_crypto(Arc::new(quic)),
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("endpoint");
        let addr = endpoint.local_addr().expect("local addr");

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    let Ok((mut send, mut recv)) = conn.accept_bi().await else {
                        return;
                    };
                    let _ = recv.read_to_end(64 * 1024).await;

                    match behaviour {
                        StubBehaviour::Error => {
                            let body = br#"{"error":{"code":"server_busy","message":"stream concurrency limit reached; retry after a delay"}}"#;
                            let _ = send.write_all(body).await;
                            let _ = send.finish();
                        }
                        StubBehaviour::EmptyResponse => {
                            let _ = send.finish();
                        }
                        StubBehaviour::OkThenNoUni => {
                            let _ = send.write_all(br#"{"result":{}}"#).await;
                            let _ = send.finish();
                        }
                        StubBehaviour::OkThenUni => {
                            let _ = send.write_all(br#"{"result":{}}"#).await;
                            let _ = send.finish();
                            if let Ok(mut uni) = conn.open_uni().await {
                                let _ = uni.write_all(b"{\"event\":\"hello\"}\n").await;
                                let _ = uni.finish();
                            }
                        }
                    }
                    // Hold the connection open so the client sees the
                    // behaviour under test rather than a connection close.
                    let _ = conn.closed().await;
                });
            }
        });
        addr
    }

    /// Short enough to keep the timeout case fast, long enough that a healthy
    /// loopback handshake never trips it.
    const TEST_HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

    async fn subscribe_against(behaviour: StubBehaviour) -> Result<RecvStream, ClientError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let addr = spawn_stub_server(behaviour);
        let identity = ClientIdentity::ephemeral();
        let client = OiClient::connect_from(
            addr,
            "0.0.0.0:0".parse().unwrap(),
            ClientAuth::TrustAny,
            &identity,
            Actor::default(),
        )
        .await
        .expect("connect to stub");
        let request = serde_json::to_vec(&serde_json::json!({
            "method": "/events/subscribe",
            "actor": client.actor(),
            "params": {},
        }))
        .expect("serialisation");
        // Every outcome must be reached promptly: a regression that parks the
        // caller fails here instead of hanging the test run.
        tokio::time::timeout(
            Duration::from_secs(20),
            client.open_subscription_within(&request, TEST_HANDSHAKE_TIMEOUT),
        )
        .await
        .expect("subscribe must not block indefinitely")
    }

    // i[verify stream.subscribe]
    // An error response terminates the request: the client must surface the
    // server's code and message rather than waiting for a stream that the
    // server has already decided not to open.
    #[tokio::test]
    async fn error_response_surfaces_instead_of_hanging() {
        match subscribe_against(StubBehaviour::Error).await {
            Err(ClientError::Api { code, message }) => {
                assert_eq!(code, "server_busy");
                assert!(message.contains("concurrency"), "message preserved");
            }
            other => panic!("expected an Api error, got {other:?}"),
        }
    }

    // i[verify stream.subscribe]
    // The server drops malformed requests by finishing the bidi stream with no
    // response at all; that is an error, not a successful handshake.
    #[tokio::test]
    async fn empty_response_is_a_protocol_error() {
        match subscribe_against(StubBehaviour::EmptyResponse).await {
            Err(ClientError::Protocol(msg)) => {
                assert!(msg.contains("without a response"), "got {msg}");
            }
            other => panic!("expected a Protocol error, got {other:?}"),
        }
    }

    // i[verify stream.subscribe]
    // The server's `open_uni` failure path only logs, so a client that waits
    // unbounded on a confirmed-OK handshake still hangs forever.
    #[tokio::test]
    async fn missing_data_stream_times_out() {
        match subscribe_against(StubBehaviour::OkThenNoUni).await {
            Err(ClientError::Protocol(msg)) => {
                assert!(msg.contains("never opened the data stream"), "got {msg}");
            }
            other => panic!("expected a Protocol error, got {other:?}"),
        }
    }

    // i[verify stream.subscribe]
    #[tokio::test]
    async fn successful_handshake_returns_the_data_stream() {
        let mut stream = subscribe_against(StubBehaviour::OkThenUni)
            .await
            .expect("handshake should succeed");
        let body = stream.read_to_end(4096).await.expect("read events");
        assert_eq!(body, b"{\"event\":\"hello\"}\n");
    }
}
