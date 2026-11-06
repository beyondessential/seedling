//! Certificate-serving module.
//!
//! Implements the runtime side of [`tls.cert.serve`](
//! ../../../../../docs/spec/runtime.md): on-demand lookup of a stored
//! certificate by SNI hostname, with the private key decrypted only at
//! request time and never written into the proxy's persistent
//! configuration.
//!
//! The lookup function is transport-agnostic. The daemon wires it behind a
//! small HTTP server bound to a local address that Caddy's
//! `tls.certificates.get_certificate.http` module fetches at handshake
//! time. The wire format is a single response body containing the leaf
//! cert chain followed by the private key, all as PEM blocks — that's
//! the convention Caddy expects.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rand::{Rng, rng};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use snafu::{ResultExt, Snafu};
use subtle::ConstantTimeEq;

use super::{
    TlsCertOrigin, TlsCertState,
    chain::ProxyFrom,
    issuance::Coordinator,
    state::{ChainDecision, ProxyHolding},
    store,
};
use crate::runtime::{db::DbHandle, secrets::Cipher};

#[derive(Debug, Snafu)]
pub enum ServeError {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("decryption error: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },

    #[snafu(display("certificate row {id} has no PEM chain stored"))]
    MissingPem { id: i64 },
}

pub type Result<T, E = ServeError> = std::result::Result<T, E>;

/// A certificate ready for the proxy to serve.
pub struct CertBundle {
    pub cert_id: i64,
    pub origin: TlsCertOrigin,
    pub chain_pem: String,
    pub key_pem: SecretString,
}

/// Look up the active runtime-managed certificate for `hostname`.
///
/// Returns `Ok(None)` when there is no runtime-managed cert for the
/// hostname — the caller should respond 404 to Caddy, which falls back to
/// the default automation policy (ACME HTTP-01).
// r[impl tls.cert.serve]
pub async fn lookup(db: &DbHandle, cipher: &Cipher, hostname: &str) -> Result<Option<CertBundle>> {
    let hostname_owned = hostname.to_owned();
    let row_opt = db
        .call(move |db_inner| store::find_active_for_hostname(db_inner, &hostname_owned))
        .context(StorageSnafu)?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    if row.state != TlsCertState::Active {
        return Ok(None);
    }

    let Some(chain_pem) = row.cert_pem else {
        return MissingPemSnafu { id: row.id }.fail();
    };

    let key_pem = cipher.decrypt(&row.key_ciphertext).context(CipherSnafu)?;

    Ok(Some(CertBundle {
        cert_id: row.id,
        origin: row.origin,
        chain_pem,
        key_pem,
    }))
}

/// Format a [`CertBundle`] as Caddy's `tls.certificates.get_certificate.http`
/// expects: PEM cert chain followed by the PEM private key. A trailing
/// newline ensures both blocks are well-separated.
pub fn format_response(bundle: &CertBundle) -> String {
    let mut out = String::with_capacity(bundle.chain_pem.len() + 256);
    out.push_str(&bundle.chain_pem);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(bundle.key_pem.expose_secret());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Shared state for the cert-serving HTTP server.
///
/// The serve path is pure lookup: when Caddy asks for a cert by SNI we
/// either return the active runtime-managed cert or 204 to let Caddy fall
/// through to its own ACME chain. All issuance lives in the
/// [`runtime::tls::issuance`](super::issuance) coordinator, driven by the
/// reconciler tick — never by Caddy handshakes.
#[derive(Clone)]
struct ServeState {
    db: DbHandle,
    cipher: Arc<Cipher>,
    /// Decides whether the proxy may go on to order a certificate of its own for
    /// a hostname the runtime holds none for.
    coordinator: Arc<Coordinator>,
    /// Hex-encoded shared token; clients embed this in the URL path.
    /// See [`load_or_create_token`].
    token: Arc<String>,
}

/// Query string parsed from Caddy's `tls.certificates.get_certificate.http`
/// request: it always sends `?server_name=<host>`.
#[derive(Deserialize)]
struct ServeQuery {
    server_name: Option<String>,
}

async fn handle(
    State(state): State<ServeState>,
    AxumPath(token): AxumPath<String>,
    Query(query): Query<ServeQuery>,
) -> impl IntoResponse {
    // Constant-time compare so a probe can't time-based-distinguish a near-match.
    if token.as_bytes().ct_eq(state.token.as_bytes()).unwrap_u8() != 1 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(hostname) = query
        .server_name
        .as_deref()
        .map(normalise_sni)
        .filter(|s| !s.is_empty())
    else {
        return (
            StatusCode::BAD_REQUEST,
            "missing server_name query parameter",
        )
            .into_response();
    };

    // r[impl tls.cert.serve]
    // Pure lookup: have a cert? return it. No? 204 to let Caddy fall
    // through to its own ACME chain. All issuance lives in the
    // reconciler-driven coordinator; this endpoint never triggers a
    // cert flow itself.
    match lookup(&state.db, &state.cipher, &hostname).await {
        Ok(Some(bundle)) => {
            let body = format_response(&bundle);
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
                body,
            )
                .into_response()
        }
        // 204 No Content is Caddy's contract for "no cert; fall through to
        // the policy's regular issuer". Any other non-2xx would be treated
        // as an error rather than a fall-through signal, so it is also what
        // keeps the proxy from ordering when it must not.
        Ok(None) => match fall_through(&state.coordinator, &hostname) {
            FallThrough::Allow => StatusCode::NO_CONTENT.into_response(),
            FallThrough::Withhold => (
                StatusCode::SERVICE_UNAVAILABLE,
                "a certificate for this hostname is not to be ordered by the proxy yet",
            )
                .into_response(),
        },
        Err(e) => {
            tracing::warn!(%hostname, error = %e, "cert serve lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response()
        }
    }
}

/// Whether the proxy may go on to its own issuer for a hostname the runtime
/// holds no certificate for.
// r[impl tls.chain.proxy-gate]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallThrough {
    Allow,
    /// Answer so that the proxy neither orders nor serves anything for the
    /// hostname.
    Withhold,
}

/// What to tell the proxy for a hostname with no stored certificate.
///
/// The proxy is only held back where doing so costs nothing it has: asked again
/// for a hostname it holds a certificate for, any answer but a fall-through
/// takes that certificate out of service, so it is only withheld from a
/// hostname the proxy holds nothing for. Where the proxy holds one, its own
/// renewal is held back by the schedule in its configuration instead.
// r[impl tls.chain.proxy-gate]
pub fn endpoint_gate(c: &ChainDecision) -> FallThrough {
    match (c.proxy_from, &c.proxy) {
        (ProxyFrom::Always, _) => FallThrough::Allow,
        (_, Some(ProxyHolding::Nothing)) => FallThrough::Withhold,
        // The proxy holds a certificate, or what it holds could not be read, or
        // was not looked at: none of these is licence to take it down.
        (_, Some(ProxyHolding::Holds(_) | ProxyHolding::Unreadable) | None) => FallThrough::Allow,
    }
}

/// The name a client sent as SNI, as the rest of the runtime holds a hostname.
///
/// A DNS name is case-insensitive and may carry a trailing dot for the root, but
/// the hostname an ingress declares is matched exactly: by the policy patterns,
/// by the managed-ingress set, and by the keys of the chain's facts. A client
/// chooses what it sends, so a name that differed only in case would resolve as
/// one no app declares, and the gate would let the proxy order for a hostname
/// another strategy is responsible for.
// r[impl tls.chain.proxy-gate]
fn normalise_sni(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// [`endpoint_gate`] for the hostname, or [`FallThrough::Allow`] when it is on
/// no chain or the decision could not be made.
///
/// Allowing is the answer to not knowing, because the other error is a
/// hostname that serves nothing, and the cost of the first is one certificate
/// ordered earlier than the chain would have liked.
///
/// Nothing is remembered here: the decision is served from the one state the
/// coordinator holds for every hostname, which is where it is kept and the only
/// place to forget it. A second memory of the same answer would be a second age
/// for it, and one no operator action could clear.
fn fall_through(coordinator: &Coordinator, hostname: &str) -> FallThrough {
    match coordinator.chain_decisions(Some(hostname)) {
        Some(decisions) => decisions
            .get(hostname)
            .map_or(FallThrough::Allow, endpoint_gate),
        None => FallThrough::Allow,
    }
}

/// Axum middleware that logs every request to the cert endpoint at
/// info level. The token in the URL path is redacted so logs never
/// disclose it. Intended primarily for diagnosing whether the proxy is
/// actually hitting this endpoint at handshake time.
async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path_redacted = redact_token_in_path(req.uri().path());
    let server_name_present = req.uri().query().is_some_and(|q| q.contains("server_name"));
    let started = std::time::Instant::now();
    let response = next.run(req).await;
    let elapsed = started.elapsed();
    let status = response.status();
    tracing::info!(
        target: "seedling::tls::serve",
        %method,
        path = %path_redacted,
        status = status.as_u16(),
        elapsed_ms = elapsed.as_millis() as u64,
        sni_present = server_name_present,
        "cert endpoint request"
    );
    response
}

fn redact_token_in_path(path: &str) -> String {
    // Path shape is `/cert/<token>/get-certificate`; replace the token
    // segment with `<token>` so logs don't leak the shared secret.
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 4 && parts[1] == "cert" {
        let mut out = String::new();
        out.push_str(parts[0]);
        out.push_str("/cert/<token>/");
        out.push_str(&parts[3..].join("/"));
        return out;
    }
    path.to_owned()
}

/// Load (or generate and persist) the shared path-token used to authorise
/// cert-fetch requests. Stored as a 0600-permission file alongside the
/// database secret key. The file content is the hex-encoded random token.
///
/// The token is defence-in-depth on top of binding to a non-routable
/// bridge IP: it stops other host processes from harvesting private keys
/// by SNI enumeration even when they can reach the listener.
pub fn load_or_create_token(path: &Path) -> std::io::Result<String> {
    if path.exists() {
        let mode = path.metadata()?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "cert-endpoint token at {} has insecure permissions (0{:o}); expected 0600",
                    path.display(),
                    mode
                ),
            ));
        }
        let contents = std::fs::read_to_string(path)?;
        return Ok(contents.trim().to_owned());
    }
    let mut bytes = [0u8; 32];
    rng().fill_bytes(&mut bytes);
    let token = hex_encode(&bytes);
    std::fs::write(path, &token)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(token)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").expect("write to String is infallible");
    }
    s
}

/// Render the URL Caddy should fetch from. Caddy automatically appends
/// `?server_name=...` and overwrites any existing query, so the token
/// must live in the path.
pub fn caddy_url(bind_addr: SocketAddr, token: &str) -> String {
    format!("http://{bind_addr}/cert/{token}/get-certificate")
}

/// Spawn an HTTP server bound to `bind_addr` that responds to Caddy's
/// `get_certificate.http` requests by looking up the active runtime-managed
/// cert for the SNI hostname and returning it as concatenated PEM, or 404
/// when no such cert is stored. Returns the bound socket address (possibly
/// resolved from a port=0 caller) and the join handle.
///
/// `token` is the path component a caller must include in the URL; see
/// [`load_or_create_token`].
// r[impl tls.cert.serve]
pub async fn spawn_server(
    db: DbHandle,
    cipher: Arc<Cipher>,
    coordinator: Arc<Coordinator>,
    token: String,
    bind_addr: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let app_state = ServeState {
        db,
        cipher,
        coordinator,
        token: Arc::new(token),
    };
    let app = Router::new()
        .route("/cert/{token}/get-certificate", get(handle))
        .layer(middleware::from_fn(log_request))
        .with_state(app_state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "tls cert server exited");
        }
    });
    Ok((local, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;
    use crate::runtime::tls::{
        KeyType, TlsCertOrigin, TlsCertState,
        store::{CertMetadata, NewCertificate, insert_certificate},
    };
    use secrecy::SecretString;

    async fn fresh() -> (DbHandle, Cipher) {
        let db = DbHandle::open_in_memory().unwrap();
        let cipher = Cipher::for_tests();
        (db, cipher)
    }

    fn insert_active(db: &DbHandle, cipher: &Cipher, hostname: &str, key_pem: &str) -> i64 {
        let host = hostname.to_owned();
        let key_ct = cipher
            .encrypt(&SecretString::new(key_pem.to_owned().into()))
            .unwrap();
        db.call(move |db_inner: &Db| -> i64 {
            insert_certificate(
                db_inner,
                NewCertificate {
                    hostname: &host,
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Manual,
                    cert_pem: Some(&crate::runtime::tls::test_support::self_signed_pem(&[
                        &host,
                    ])),
                    csr_pem: None,
                    key_ciphertext: &key_ct,
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        issuer: Some("CN=Test".to_owned()),
                        // Relative to now: the serving lookup gates on the
                        // validity window, so a fixed expiry would turn this
                        // red on the day it passed.
                        not_before: Some(jiff::Timestamp::now().as_second() - 86_400),
                        not_after: Some(jiff::Timestamp::now().as_second() + 365 * 86_400),
                        serial: Some("01".to_owned()),
                        self_signed: false,
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap()
        })
    }

    #[tokio::test]
    async fn lookup_returns_active_cert() {
        let (db, cipher) = fresh().await;
        let key_pem = "-----BEGIN PRIVATE KEY-----\nMIGdummykey\n-----END PRIVATE KEY-----\n";
        let id = insert_active(&db, &cipher, "foo.example.com", key_pem);

        let bundle = lookup(&db, &cipher, "foo.example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bundle.cert_id, id);
        assert_eq!(bundle.origin, TlsCertOrigin::Manual);
        assert!(bundle.chain_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(bundle.key_pem.expose_secret(), key_pem);
    }

    #[tokio::test]
    async fn lookup_returns_none_when_no_cert() {
        let (db, cipher) = fresh().await;
        let result = lookup(&db, &cipher, "missing.example.com").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn format_response_concatenates_chain_then_key() {
        let bundle = CertBundle {
            cert_id: 1,
            origin: TlsCertOrigin::Manual,
            chain_pem: "-----BEGIN CERTIFICATE-----\ncertdata\n-----END CERTIFICATE-----\n"
                .to_owned(),
            key_pem: SecretString::new(
                "-----BEGIN PRIVATE KEY-----\nkeydata\n-----END PRIVATE KEY-----\n".into(),
            ),
        };
        let body = format_response(&bundle);
        let cert_pos = body.find("BEGIN CERTIFICATE").unwrap();
        let key_pos = body.find("BEGIN PRIVATE KEY").unwrap();
        assert!(cert_pos < key_pos, "cert must precede key");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_returns_pem_for_known_hostname() {
        let (db, cipher) = fresh().await;
        let key_pem = "-----BEGIN PRIVATE KEY-----\nMIGdummykey\n-----END PRIVATE KEY-----\n";
        insert_active(&db, &cipher, "served.example.com", key_pem);

        let token = "deadbeef".to_owned();
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            token.clone(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();

        let url =
            format!("http://{addr}/cert/{token}/get-certificate?server_name=served.example.com");
        let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
        assert!(body.contains("BEGIN CERTIFICATE"));
        assert!(body.contains("BEGIN PRIVATE KEY"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_returns_204_for_unknown_hostname() {
        // Caddy's http cert getter treats 204 as "no cert; fall through".
        // Any other non-2xx (e.g. 404) would be an error, breaking the
        // intended HTTP-01 fallback for hostnames without a stored cert.
        let (db, cipher) = fresh().await;
        let token = "deadbeef".to_owned();
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            token.clone(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();

        let url =
            format!("http://{addr}/cert/{token}/get-certificate?server_name=missing.example.com");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status().as_u16(), 204);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_rejects_wrong_token() {
        let (db, cipher) = fresh().await;
        let key_pem = "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n";
        insert_active(&db, &cipher, "host.example.com", key_pem);

        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            "rightoken".to_owned(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();

        let url =
            format!("http://{addr}/cert/wrongtoken/get-certificate?server_name=host.example.com");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status().as_u16(), 404);
    }

    #[test]
    fn redact_token_keeps_path_shape_but_hides_secret() {
        let redacted = redact_token_in_path("/cert/abcdef0123/get-certificate");
        assert_eq!(redacted, "/cert/<token>/get-certificate");
        assert!(!redacted.contains("abcdef"));

        // Unrelated paths pass through unchanged.
        assert_eq!(redact_token_in_path("/health"), "/health");
        assert_eq!(redact_token_in_path("/cert"), "/cert");
    }

    #[test]
    fn caddy_url_format_round_trips_through_axum_path() {
        let url = caddy_url("[::1]:7892".parse().unwrap(), "abc123");
        assert_eq!(url, "http://[::1]:7892/cert/abc123/get-certificate");
    }

    #[test]
    fn token_persistence_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cert-endpoint-token");
        let t1 = load_or_create_token(&path).unwrap();
        let t2 = load_or_create_token(&path).unwrap();
        assert_eq!(t1, t2);
        assert_eq!(t1.len(), 64, "32-byte hex token");
        let perms = path.metadata().unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn format_response_inserts_separator_when_missing() {
        let bundle = CertBundle {
            cert_id: 1,
            origin: TlsCertOrigin::Manual,
            chain_pem: "-----BEGIN CERTIFICATE-----\nCERT\n-----END CERTIFICATE-----".to_owned(),
            key_pem: SecretString::new(
                "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----".into(),
            ),
        };
        let body = format_response(&bundle);
        assert!(body.contains("-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----"));
        assert!(body.ends_with('\n'));
    }

    use std::collections::{BTreeMap, BTreeSet};

    use crate::runtime::tls::{
        env::Environment,
        state::{ChainEnv, ManagedIngress, ProxyCert},
    };

    struct Facts {
        offer_live: bool,
        typed: bool,
        proxy: Option<ProxyHolding>,
        fail: bool,
    }

    impl Environment for Facts {
        fn gather(&self, _only: Option<&str>) -> Option<(Vec<ManagedIngress>, ChainEnv)> {
            if self.fail {
                return None;
            }
            let managed = vec![ManagedIngress {
                app: seedling_protocol::names::AppName::new_unchecked("shop"),
                ingress_name: "web".to_owned(),
                hostname: "shop.example.com".to_owned(),
            }];
            let types: BTreeSet<String> = if self.typed {
                BTreeSet::from(["tamanu".to_owned()])
            } else {
                BTreeSet::new()
            };
            let env = ChainEnv {
                offer_live: self.offer_live,
                canopy_types: BTreeMap::from([("shop.example.com".to_owned(), types)]),
                proxy: self
                    .proxy
                    .clone()
                    .map(|p| BTreeMap::from([("shop.example.com".to_owned(), p)]))
                    .unwrap_or_default(),
            };
            Some((managed, env))
        }
    }

    async fn status_for(facts: Option<Facts>) -> u16 {
        let (db, cipher) = fresh().await;
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        if let Some(facts) = facts {
            coordinator.set_environment(Arc::new(facts));
        }
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            "tok".to_owned(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();
        let url = format!("http://{addr}/cert/tok/get-certificate?server_name=shop.example.com");
        reqwest::get(&url).await.unwrap().status().as_u16()
    }

    fn typed(proxy: Option<ProxyHolding>) -> Facts {
        Facts {
            offer_live: true,
            typed: true,
            proxy,
            fail: false,
        }
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_proxy_is_kept_from_ordering_while_canopy_is_responsible() {
        assert_eq!(
            status_for(Some(typed(Some(ProxyHolding::Nothing)))).await,
            503,
            "nothing is ordered, and nothing the proxy held is taken down"
        );
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_certificate_the_proxy_holds_is_never_taken_down_to_hold_it_back() {
        let held = ProxyHolding::Holds(ProxyCert {
            not_before: Some(0),
            not_after: Some(i64::MAX / 2),
            issuer_kind: "local".to_owned(),
            issuer: None,
            self_signed: true,
        });
        assert_eq!(status_for(Some(typed(Some(held)))).await, 204);
        assert_eq!(
            status_for(Some(typed(Some(ProxyHolding::Unreadable)))).await,
            204,
            "what could not be read is not read as absent"
        );
        assert_eq!(
            status_for(Some(typed(None))).await,
            204,
            "nor is what was not looked at"
        );
    }

    // r[verify tls.chain.proxy-gate]
    // r[verify tls.chain.no-certificate]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_proxy_is_let_order_once_it_is_responsible() {
        let mut facts = typed(Some(ProxyHolding::Nothing));
        facts.offer_live = false;
        assert_eq!(
            status_for(Some(facts)).await,
            204,
            "with no offer to ask, the hostname goes straight to the proxy"
        );
        let mut untyped = typed(Some(ProxyHolding::Nothing));
        untyped.typed = false;
        assert_eq!(status_for(Some(untyped)).await, 204);
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn not_knowing_is_never_a_reason_to_withhold() {
        assert_eq!(status_for(None).await, 204, "no environment yet");
        let mut failing = typed(Some(ProxyHolding::Nothing));
        failing.fail = true;
        assert_eq!(
            status_for(Some(failing)).await,
            204,
            "facts that could not be gathered"
        );
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn handshakes_for_many_hostnames_read_the_world_once() {
        struct Counting(std::sync::atomic::AtomicUsize, Facts);
        impl Environment for Counting {
            fn gather(&self, only: Option<&str>) -> Option<(Vec<ManagedIngress>, ChainEnv)> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(
                    only, None,
                    "the state is gathered for every hostname at once"
                );
                self.1.gather(only)
            }
        }
        let (db, cipher) = fresh().await;
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        let counting = Arc::new(Counting(
            std::sync::atomic::AtomicUsize::new(0),
            typed(Some(ProxyHolding::Nothing)),
        ));
        coordinator.set_environment(counting.clone());

        // Different names, all decided against the one state.
        for name in ["shop.example.com", "a.example.com", "b.example.com"] {
            assert_eq!(
                fall_through(&coordinator, name),
                if name == "shop.example.com" {
                    FallThrough::Withhold
                } else {
                    FallThrough::Allow
                }
            );
        }
        assert_eq!(
            counting.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one reading of the world serves every hostname decided against it"
        );

        coordinator.forget_chain_state();
        let _ = fall_through(&coordinator, "shop.example.com");
        assert_eq!(
            counting.0.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "and a caller that has just written something gets a fresh one"
        );
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_burst_of_handshakes_decides_once() {
        // Count how often the facts are gathered.
        struct Counting(std::sync::atomic::AtomicUsize, Facts);
        impl Environment for Counting {
            fn gather(&self, only: Option<&str>) -> Option<(Vec<ManagedIngress>, ChainEnv)> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.1.gather(only)
            }
        }
        let (db, cipher) = fresh().await;
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        let counting = Arc::new(Counting(
            std::sync::atomic::AtomicUsize::new(0),
            typed(Some(ProxyHolding::Nothing)),
        ));
        coordinator.set_environment(counting.clone());
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            "tok".to_owned(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();
        let url = format!("http://{addr}/cert/tok/get-certificate?server_name=shop.example.com");
        for _ in 0..5 {
            assert_eq!(reqwest::get(&url).await.unwrap().status().as_u16(), 503);
        }
        assert_eq!(
            counting.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a handshake decides against the state already read, not against a new one"
        );
    }

    // r[verify tls.chain.proxy-gate]
    #[test]
    fn a_name_is_taken_as_the_runtime_holds_it_however_a_client_sent_it() {
        assert_eq!(normalise_sni("SHOP.Example.COM"), "shop.example.com");
        assert_eq!(normalise_sni("shop.example.com."), "shop.example.com");
        assert_eq!(normalise_sni("shop.example.com"), "shop.example.com");
        assert_eq!(normalise_sni(""), "");
    }

    // r[verify tls.chain.proxy-gate]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_gate_is_not_evaded_by_the_case_a_client_sends() {
        let (db, cipher) = fresh().await;
        let cipher = Arc::new(cipher);
        let coordinator = Coordinator::new(db.clone(), Arc::clone(&cipher));
        coordinator.set_environment(Arc::new(typed(Some(ProxyHolding::Nothing))));
        let (addr, _handle) = spawn_server(
            db,
            cipher,
            coordinator,
            "tok".to_owned(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();
        for name in ["shop.example.com", "SHOP.Example.com", "shop.example.com."] {
            let url = format!("http://{addr}/cert/tok/get-certificate?server_name={name}");
            assert_eq!(
                reqwest::get(&url).await.unwrap().status().as_u16(),
                503,
                "{name} is the hostname Canopy is responsible for"
            );
        }
    }
}
