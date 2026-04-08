use std::{net::SocketAddr, path::Path, sync::Arc};

use tracing::Instrument;

use quinn::Endpoint;
use rustls::ServerConfig as TlsServerConfig;

use super::{
    auth::{SeedlingClientVerifier, TrustedKeys},
    handler::{OiState, dispatch},
    keys,
};

/// Default OI listen port.
pub const DEFAULT_PORT: u16 = 7891;

fn build_tls_config(
    key: &ed25519_dalek::SigningKey,
    spki: Vec<u8>,
    trusted: TrustedKeys,
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

    let client_verifier = Arc::new(SeedlingClientVerifier { trusted });

    let tls_config = TlsServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_cert_resolver(resolver);

    Ok(tls_config)
}

fn extract_client_fp(conn: &quinn::Connection) -> Option<String> {
    let id = conn.peer_identity()?;
    let certs = id
        .downcast::<Vec<rustls_pki_types::CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| keys::fingerprint(c.as_ref()))
}

// i[transport.quic]
// i[transport.local]
// i[transport.client-auth]
pub async fn run(
    state: Arc<OiState>,
    port: u16,
    data_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key_path = data_dir.join("oi.key");
    let key = keys::load_or_generate(&key_path)?;
    let spki = keys::spki_der(&key);
    let fingerprint = keys::fingerprint(&spki);

    tracing::info!("OI SPKI fingerprint: {fingerprint}");
    state.spki_fingerprint.set(fingerprint.clone()).ok();

    // Populate trusted keys: DB first, then bootstrap file.
    {
        let db = state.db.lock();
        super::auth::load_from_db(&db, &state.trusted_keys)
            .map_err(|e| format!("loading authorized keys: {e}"))?;
        super::auth::import_bootstrap_file(data_dir, &db, &state.trusted_keys)
            .map_err(|e| format!("reading bootstrap file: {e}"))?;
    }

    if state.trusted_keys.read().is_empty() {
        tracing::warn!(
            "no authorized client keys — add fingerprints to {}/authorized_keys and restart",
            data_dir.display()
        );
    }

    let tls_config = build_tls_config(&key, spki, Arc::clone(&state.trusted_keys))?;
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
    let client_fp = extract_client_fp(&conn);

    // Resolve a human-readable name for the span: the key label if available,
    // otherwise the fingerprint, otherwise "unauthenticated".
    let client: String = client_fp
        .as_deref()
        .and_then(|fp| {
            let db = state.db.lock();
            super::auth::get_label(&db, fp).ok().flatten()
        })
        .or_else(|| client_fp.clone())
        .unwrap_or_else(|| "unauthenticated".to_owned());

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
        let client = client.clone();
        tokio::spawn(
            handle_bidi_stream(stream, state).instrument(tracing::info_span!("oi", %peer, %client)),
        );
    }
}

async fn handle_bidi_stream(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
    state: Arc<OiState>,
) {
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
