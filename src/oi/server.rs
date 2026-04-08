use std::{net::SocketAddr, path::Path, sync::Arc};

use tracing::Instrument;

use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use quinn::Endpoint;
use rand_core::OsRng;
use rustls::{
    ServerConfig as TlsServerConfig, server::AlwaysResolvesServerRawPublicKeys, sign::CertifiedKey,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use super::handler::{OiState, dispatch};

/// Default OI listen port.
pub const DEFAULT_PORT: u16 = 7891;

fn load_or_generate_key(key_path: &Path) -> std::io::Result<SigningKey> {
    if key_path.exists() {
        let der = std::fs::read(key_path)?;
        SigningKey::from_pkcs8_der(&der)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    } else {
        let key = SigningKey::generate(&mut OsRng);
        let doc = key
            .to_pkcs8_der()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(key_path, doc.as_bytes())?;
        Ok(key)
    }
}

fn ed25519_spki(key: &SigningKey) -> Vec<u8> {
    // Fixed DER prefix for SubjectPublicKeyInfo with Ed25519 (OID 1.3.101.112)
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let mut spki = Vec::with_capacity(44);
    spki.extend_from_slice(&PREFIX);
    spki.extend_from_slice(key.verifying_key().as_bytes());
    spki
}

fn hex_fingerprint(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn build_tls_config(
    key: &SigningKey,
    spki_der: Vec<u8>,
) -> Result<TlsServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|e| format!("key encoding: {e}"))?;
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)
        .map_err(|e| format!("signing key: {e}"))?;

    let cert = CertificateDer::from(spki_der);
    let certified_key = Arc::new(CertifiedKey::new(vec![cert], signing_key));
    let resolver = Arc::new(AlwaysResolvesServerRawPublicKeys::new(certified_key));

    let tls_config = TlsServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    Ok(tls_config)
}

// i[transport.quic]
// i[transport.local]
pub async fn run(
    state: Arc<OiState>,
    port: u16,
    data_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key_path = data_dir.join("oi.key");
    let key = load_or_generate_key(&key_path)?;
    let spki = ed25519_spki(&key);
    let fingerprint = {
        let mut hasher = Sha256::new();
        hasher.update(&spki);
        hex_fingerprint(hasher.finalize().as_ref())
    };

    tracing::info!("OI SPKI fingerprint: {fingerprint}");
    state.spki_fingerprint.set(fingerprint.clone()).ok();

    let tls_config = build_tls_config(&key, spki)?;
    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));

    let addr: SocketAddr = format!("[::1]:{port}").parse().unwrap();
    let endpoint = Endpoint::server(server_config, addr)?;

    tracing::info!("OI listening on {}", endpoint.local_addr()?);

    tokio::spawn(accept_loop(endpoint, state));

    Ok(fingerprint)
}

async fn accept_loop(endpoint: Endpoint, state: Arc<OiState>) {
    while let Some(incoming) = endpoint.accept().await {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => handle_connection(conn, state).await,
                Err(e) => tracing::warn!("incoming connection failed: {e}"),
            }
        });
    }
}

// i[stream.control]
// i[stream.dispatch]
async fn handle_connection(conn: quinn::Connection, state: Arc<OiState>) {
    let peer = conn.remote_address();
    loop {
        let stream = match conn.accept_bi().await {
            Ok(s) => s,
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => break,
            Err(e) => {
                tracing::warn!("connection error: {e}");
                break;
            }
        };
        let state = Arc::clone(&state);
        tokio::spawn(
            handle_bidi_stream(stream, state).instrument(tracing::info_span!("oi", %peer)),
        );
    }
}

async fn handle_bidi_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    state: Arc<OiState>,
) {
    // Read until client half-closes (read_to_end).
    let buf = match recv.read_to_end(4 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("stream read error: {e}");
            return;
        }
    };

    let response = tokio::task::block_in_place(|| dispatch(&state, &buf));

    if let Err(e) = send.write_all(&response).await {
        tracing::warn!("stream write error: {e}");
    }
    let _ = send.finish();
}
