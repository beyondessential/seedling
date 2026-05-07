//! Shared QUIC endpoint, accept loop, and ALPN dispatch.
//!
//! The endpoint advertises every ALPN registered with [`AlpnHandlers`]; on
//! accept, the negotiated ALPN selects the registered handler, after the
//! per-protocol trust gate (in [`crate::transport::auth`]) has approved
//! the client's fingerprint.

use std::{net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use parking_lot::RwLock;
use quinn::Endpoint;
use rustls::ServerConfig as TlsServerConfig;

use seedling_protocol::keys;

use crate::transport::auth::{ProtocolTrustRegistry, SeedlingClientVerifier};

/// Default listen port. The shared endpoint is one TCP/UDP port for all
/// registered ALPNs.
pub const DEFAULT_PORT: u16 = 7891;

/// Per-connection context passed to the registered ALPN handler.
pub struct ConnectionContext {
    /// Client's SPKI SHA-256 fingerprint, or `None` if no client cert was
    /// presented (currently impossible since RPK mTLS is required).
    pub client_fp: Option<String>,
    /// Human-readable label for log spans, derived from `client_fp` via the
    /// [`LabelLookup`] callback. Falls back to the fingerprint, then
    /// `"unauthenticated"`.
    pub client_label: String,
    /// ALPN identifier negotiated for this connection.
    pub negotiated_alpn: Vec<u8>,
}

/// Future returned by a registered handler. The transport layer awaits it
/// to keep the per-connection task alive for the lifetime of the
/// connection.
pub type ConnectionFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Handler invoked once per accepted connection, after the post-handshake
/// trust gate has approved the negotiated ALPN.
pub type ConnectionHandler =
    Arc<dyn Fn(quinn::Connection, ConnectionContext) -> ConnectionFuture + Send + Sync>;

/// Registry of ALPN → handler mappings. Each protocol (OI, grove, …)
/// registers its handler before [`run`] is called.
#[derive(Default)]
pub struct AlpnHandlers {
    inner: RwLock<Vec<(Vec<u8>, ConnectionHandler)>>,
}

impl AlpnHandlers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self, alpn: &[u8], handler: ConnectionHandler) {
        let mut inner = self.inner.write();
        if let Some(slot) = inner.iter_mut().find(|(a, _)| a == alpn) {
            slot.1 = handler;
        } else {
            inner.push((alpn.to_vec(), handler));
        }
    }

    pub fn lookup(&self, alpn: &[u8]) -> Option<ConnectionHandler> {
        self.inner
            .read()
            .iter()
            .find(|(a, _)| a == alpn)
            .map(|(_, h)| Arc::clone(h))
    }

    pub fn alpn_list(&self) -> Vec<Vec<u8>> {
        self.inner.read().iter().map(|(a, _)| a.clone()).collect()
    }
}

/// Optional fingerprint → label resolver, used to decorate per-connection
/// log spans with human-readable peer names.
pub type LabelLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct EndpointConfig {
    pub key_path: PathBuf,
    pub addrs: Vec<SocketAddr>,
    pub trust: Arc<ProtocolTrustRegistry>,
    pub handlers: Arc<AlpnHandlers>,
    pub label_lookup: Option<LabelLookup>,
}

fn build_tls_config(
    key: &ed25519_dalek::SigningKey,
    spki: Vec<u8>,
    trust: Arc<ProtocolTrustRegistry>,
    alpn_list: Vec<Vec<u8>>,
) -> Result<TlsServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use rustls::{server::AlwaysResolvesServerRawPublicKeys, sign::CertifiedKey};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|e| format!("key encoding: {e}"))?;
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
        .map_err(|e| format!("signing key: {e}"))?;

    let cert = CertificateDer::from(spki);
    let certified_key = Arc::new(CertifiedKey::new(vec![cert], signing_key));
    let resolver = Arc::new(AlwaysResolvesServerRawPublicKeys::new(certified_key));

    let client_verifier = Arc::new(SeedlingClientVerifier { registry: trust });

    let mut tls_config = TlsServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_cert_resolver(resolver);

    tls_config.key_log = Arc::new(rustls::KeyLogFile::new());
    // i[transport.alpn]
    tls_config.alpn_protocols = alpn_list;

    Ok(tls_config)
}

/// Extract the client's SPKI SHA-256 fingerprint from a connected peer's
/// certificate, when present.
pub fn extract_client_fp(conn: &quinn::Connection) -> Option<String> {
    let id = conn.peer_identity()?;
    let certs = id
        .downcast::<Vec<rustls_pki_types::CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| keys::fingerprint(c.as_ref()))
}

fn extract_negotiated_alpn(conn: &quinn::Connection) -> Option<Vec<u8>> {
    let hd = conn.handshake_data()?;
    let hd = hd.downcast::<quinn::crypto::rustls::HandshakeData>().ok()?;
    hd.protocol
}

// i[transport.quic]
// i[transport.server-identity]
// i[transport.client-auth]
// i[transport.listen]
pub async fn run(
    config: EndpointConfig,
) -> Result<(String, Vec<Endpoint>), Box<dyn std::error::Error + Send + Sync>> {
    let key = keys::load_or_generate(&config.key_path)?;
    let spki = keys::spki_der(&key);
    let fingerprint = keys::fingerprint(&spki);

    tracing::info!("transport SPKI fingerprint: {fingerprint}");

    let alpn_list = config.handlers.alpn_list();
    if alpn_list.is_empty() {
        return Err("no ALPN handlers registered".into());
    }

    let tls_config = build_tls_config(&key, spki, Arc::clone(&config.trust), alpn_list)?;
    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)?;
    let mut transport = quinn::TransportConfig::default();
    // Send PING frames every 10 s so idle connections do not trip the idle
    // timeout. As long as a peer (e.g. seedling-ctl, or a grove peer) is alive
    // and the network is healthy, PINGs reset the idle timer and the
    // connection stays open indefinitely.
    transport.keep_alive_interval(Some(Duration::from_secs(10)));
    // 30 s: long enough that a single missed PING does not drop the
    // connection, short enough that a genuinely dead connection is detected
    // and cleaned up promptly.
    transport.max_idle_timeout(Some(quinn::VarInt::from_u32(30_000).into()));
    // Enable QUIC datagrams for UDP port-forward relay.
    transport.datagram_receive_buffer_size(Some(65536));
    let transport_cfg = Arc::new(transport);

    // Build one server config and clone it per endpoint (cheap: all heavy
    // state is behind Arcs inside quinn::ServerConfig).
    let mut base_server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));
    base_server_config.transport_config(Arc::clone(&transport_cfg));

    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        tracing::warn!("SSLKEYLOGFILE is set — TLS session keys are being logged to disk");
    }

    let mut endpoints = Vec::with_capacity(config.addrs.len());
    for addr in &config.addrs {
        let endpoint = Endpoint::server(base_server_config.clone(), *addr)?;
        tracing::info!("transport listening on {}", endpoint.local_addr()?);
        let ep_clone = endpoint.clone();
        tokio::spawn(accept_loop(
            endpoint,
            Arc::clone(&config.handlers),
            Arc::clone(&config.trust),
            config.label_lookup.clone(),
        ));
        endpoints.push(ep_clone);
    }

    Ok((fingerprint, endpoints))
}

async fn accept_loop(
    endpoint: Endpoint,
    handlers: Arc<AlpnHandlers>,
    trust: Arc<ProtocolTrustRegistry>,
    label_lookup: Option<LabelLookup>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let handlers = Arc::clone(&handlers);
        let trust = Arc::clone(&trust);
        let label_lookup = label_lookup.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => dispatch_connection(conn, handlers, trust, label_lookup).await,
                Err(e) => tracing::warn!("incoming connection failed: {e}"),
            }
        });
    }
}

async fn dispatch_connection(
    conn: quinn::Connection,
    handlers: Arc<AlpnHandlers>,
    trust: Arc<ProtocolTrustRegistry>,
    label_lookup: Option<LabelLookup>,
) {
    let alpn = match extract_negotiated_alpn(&conn) {
        Some(a) => a,
        None => {
            tracing::warn!(
                peer = %conn.remote_address(),
                "connection without ALPN data; closing"
            );
            conn.close(quinn::VarInt::from_u32(0), b"missing alpn");
            return;
        }
    };

    let client_fp = extract_client_fp(&conn);

    if let Some(fp) = client_fp.as_deref()
        && !trust.is_trusted_for(&alpn, fp)
    {
        tracing::warn!(
            fingerprint = %fp,
            alpn = %String::from_utf8_lossy(&alpn),
            "client fingerprint not authorised for negotiated ALPN; closing"
        );
        conn.close(quinn::VarInt::from_u32(0), b"alpn not authorised");
        return;
    }

    let handler = match handlers.lookup(&alpn) {
        Some(h) => h,
        None => {
            tracing::warn!(
                alpn = %String::from_utf8_lossy(&alpn),
                "no registered handler for negotiated ALPN; closing"
            );
            conn.close(quinn::VarInt::from_u32(0), b"alpn unhandled");
            return;
        }
    };

    let client_label = client_fp
        .as_deref()
        .and_then(|fp| label_lookup.as_ref().and_then(|f| f(fp)))
        .or_else(|| client_fp.clone())
        .unwrap_or_else(|| "unauthenticated".to_owned());

    let ctx = ConnectionContext {
        client_fp,
        client_label,
        negotiated_alpn: alpn,
    };

    handler(conn, ctx).await;
}

/// Read bytes from `recv` until a `\n` is found or `max_len` is reached.
/// Returns `(line_including_newline, leftover_bytes_after_newline)`. On
/// stream close before any data, returns `([], [])`.
pub async fn read_json_line(
    recv: &mut quinn::RecvStream,
    max_len: usize,
) -> Result<(Vec<u8>, Vec<u8>), quinn::ReadError> {
    let mut buf = vec![0u8; max_len.min(65536)];
    let mut total = 0usize;
    loop {
        let chunk_size = (max_len - total).min(1024);
        if chunk_size == 0 {
            break;
        }
        match recv.read(&mut buf[total..total + chunk_size]).await? {
            Some(0) | None => break,
            Some(n) => {
                total += n;
                if let Some(pos) = buf[..total].iter().position(|&b| b == b'\n') {
                    let line = buf[..=pos].to_vec();
                    let leftover = buf[pos + 1..total].to_vec();
                    return Ok((line, leftover));
                }
            }
        }
    }
    Ok((buf[..total].to_vec(), Vec::new()))
}
