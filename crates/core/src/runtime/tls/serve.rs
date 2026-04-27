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

use std::collections::HashMap;
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
use rand_core::{OsRng, RngCore};
use secrecy::{ExposeSecret, SecretString};
use seedling_protocol::names::AppName;
use serde::Deserialize;
use snafu::{ResultExt, Snafu};
use subtle::ConstantTimeEq;

use super::{
    TlsCertOrigin, TlsCertState, TlsPolicy,
    acme::{self, IssueParams},
    store,
};
use crate::runtime::{db::DbHandle, faults, secrets::Cipher};

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
#[derive(Clone)]
struct ServeState {
    db: DbHandle,
    cipher: Arc<Cipher>,
    /// Hex-encoded shared token; clients embed this in the URL path.
    /// See [`load_or_create_token`].
    token: Arc<String>,
    /// Per-hostname mutex used to serialise on-demand issuance requests for
    /// the same SNI. Without this a burst of concurrent handshakes for a
    /// fresh hostname would each kick off a parallel ACME flow.
    issue_locks: IssueLocks,
}

type IssueLocks = Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

fn acquire_issue_lock(locks: &IssueLocks, hostname: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = locks.lock().expect("issue_locks poisoned");
    Arc::clone(
        map.entry(hostname.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
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
    let Some(hostname) = query.server_name.filter(|s| !s.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            "missing server_name query parameter",
        )
            .into_response();
    };

    // Fast path: cert already stored.
    match lookup(&state.db, &state.cipher, &hostname).await {
        Ok(Some(bundle)) => return cert_ok(&bundle).into_response(),
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(%hostname, error = %e, "cert serve lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response();
        }
    }

    // No cert. If the hostname is covered by an `acme_dns` policy we own
    // the issuance path — try now rather than 204'ing back to Caddy whose
    // own issuer chain may not be viable on this host (e.g. the host
    // isn't reachable on :80 or :443 from the public internet).
    match attempt_on_demand_issue(&state, &hostname).await {
        IssueOutcome::Served(bundle) => cert_ok(&bundle).into_response(),
        // 204 No Content is Caddy's contract for "no cert; fall through to
        // the policy's regular issuer". 404 (or any other non-2xx) would
        // be treated as an error rather than a fall-through signal.
        IssueOutcome::FallThrough => StatusCode::NO_CONTENT.into_response(),
    }
}

/// Result of [`attempt_on_demand_issue`]. Either we issued (and the cert is
/// ready to serve) or the operator hasn't configured the runtime to handle
/// this hostname and Caddy should be told to fall through.
enum IssueOutcome {
    Served(CertBundle),
    FallThrough,
}

async fn attempt_on_demand_issue(state: &ServeState, hostname: &str) -> IssueOutcome {
    // Resolve the policy first; if it's not acme_dns we have nothing to
    // contribute and Caddy can fall through to its own issuer chain.
    let host_for_lookup = hostname.to_owned();
    let policy = match state
        .db
        .call(move |db| store::resolve_policy(db, &host_for_lookup))
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(%hostname, error = %e, "policy lookup failed");
            return IssueOutcome::FallThrough;
        }
    };
    let dns_provider_name = match policy.map(|p| p.policy) {
        Some(TlsPolicy::AcmeDns { dns_provider }) => dns_provider,
        _ => return IssueOutcome::FallThrough,
    };

    // Serialise concurrent handshakes for the same SNI so a single
    // burst doesn't fan out into multiple parallel ACME orders.
    let lock = acquire_issue_lock(&state.issue_locks, hostname);
    let _guard = lock.lock().await;

    // Re-check after acquiring the lock: another holder may have just
    // finished issuance.
    match lookup(&state.db, &state.cipher, hostname).await {
        Ok(Some(bundle)) => return IssueOutcome::Served(bundle),
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(%hostname, error = %e, "cert lookup failed after lock");
            return IssueOutcome::FallThrough;
        }
    }

    // Need the global contact email to register an ACME account.
    let settings = match state.db.call(store::get_settings) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%hostname, error = %e, "settings lookup failed");
            file_cert_fault(
                &state.db,
                hostname,
                "cert_issue_failed",
                &format!("settings lookup failed: {e}"),
            );
            return IssueOutcome::FallThrough;
        }
    };
    if settings.contact_email.is_empty() {
        let msg = format!(
            "on-demand TLS for {hostname} requires a contact email; \
             set one via /tls/settings/set"
        );
        tracing::warn!("{msg}");
        file_cert_fault(&state.db, hostname, "cert_issue_failed", &msg);
        return IssueOutcome::FallThrough;
    }

    tracing::info!(
        %hostname,
        provider = %dns_provider_name,
        "on-demand ACME-DNS issuance triggered by Caddy handshake"
    );
    match acme::issue(
        &state.db,
        &state.cipher,
        IssueParams {
            hostname,
            contact_email: &settings.contact_email,
            directory_url: &acme::default_directory_url(),
            dns_provider_name: &dns_provider_name,
        },
    )
    .await
    {
        Ok(issued) => {
            tracing::info!(
                %hostname,
                cert_id = issued.cert_id,
                not_after = issued.not_after,
                "on-demand issuance succeeded"
            );
            // Clear any prior cert_issue_failed fault for this hostname.
            clear_cert_fault(&state.db, hostname, "cert_issue_failed");
            match lookup(&state.db, &state.cipher, hostname).await {
                Ok(Some(bundle)) => IssueOutcome::Served(bundle),
                Ok(None) => {
                    tracing::warn!(
                        %hostname,
                        "issuance succeeded but cert lookup returned none"
                    );
                    IssueOutcome::FallThrough
                }
                Err(e) => {
                    tracing::warn!(%hostname, error = %e, "cert lookup after issue failed");
                    IssueOutcome::FallThrough
                }
            }
        }
        Err(e) => {
            let msg = format!("on-demand ACME-DNS issuance for {hostname} failed: {e}");
            tracing::warn!("{msg}");
            file_cert_fault(&state.db, hostname, "cert_issue_failed", &msg);
            IssueOutcome::FallThrough
        }
    }
}

fn cert_ok(bundle: &CertBundle) -> impl IntoResponse {
    let body = format_response(bundle);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
        body,
    )
}

/// File a non-app-scoped fault under the `_system` sentinel app. Idempotent
/// per (kind, description): if the same fault is already filed we don't
/// duplicate it. Failures are logged but don't propagate — the caller is
/// already on a degraded path.
fn file_cert_fault(db: &DbHandle, hostname: &str, kind: &str, description: &str) {
    let kind_owned = kind.to_owned();
    let desc_owned = description.to_owned();
    let host_owned = hostname.to_owned();
    db.call(move |db_inner| {
        let app = AppName::new_unchecked("_system");
        let already = faults::list_active_faults(db_inner, Some(&app))
            .unwrap_or_default()
            .into_iter()
            .any(|f| {
                f.kind == kind_owned && f.resource_name.as_deref() == Some(host_owned.as_str())
            });
        if already {
            return;
        }
        if let Err(e) = faults::file_fault(
            db_inner,
            &app,
            Some("hostname"),
            Some(&host_owned),
            None,
            &kind_owned,
            &desc_owned,
        ) {
            tracing::warn!(error = %e, "failed to file cert fault");
        }
    });
}

fn clear_cert_fault(db: &DbHandle, hostname: &str, kind: &str) {
    let kind_owned = kind.to_owned();
    let host_owned = hostname.to_owned();
    db.call(move |db_inner| {
        let app = AppName::new_unchecked("_system");
        let to_clear: Vec<String> = faults::list_active_faults(db_inner, Some(&app))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| {
                f.kind == kind_owned && f.resource_name.as_deref() == Some(host_owned.as_str())
            })
            .map(|f| f.id)
            .collect();
        for id in to_clear {
            if let Err(e) = faults::clear_fault(db_inner, &id, &app) {
                tracing::warn!(error = %e, "failed to clear cert fault");
            }
        }
    });
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
    OsRng.fill_bytes(&mut bytes);
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
    token: String,
    bind_addr: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let app_state = ServeState {
        db,
        cipher,
        token: Arc::new(token),
        issue_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
        store::{CertMetadata, insert_certificate},
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
                &host,
                TlsCertState::Active,
                TlsCertOrigin::Manual,
                Some("-----BEGIN CERTIFICATE-----\nMIIBcert\n-----END CERTIFICATE-----\n"),
                None,
                &key_ct,
                KeyType::EcdsaP256,
                CertMetadata {
                    issuer: Some("CN=Test".to_owned()),
                    not_before: Some(1_700_000_000),
                    not_after: Some(1_800_000_000),
                    serial: Some("01".to_owned()),
                    self_signed: false,
                },
                None,
                None,
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
        let (addr, _handle) = spawn_server(
            db,
            Arc::new(cipher),
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
    async fn server_files_fault_when_acme_dns_policy_lacks_contact_email() {
        // With an acme_dns policy in place but no contact email, on-demand
        // issuance can't proceed. We must surface this to the operator via a
        // fault rather than silently 204'ing back to Caddy.
        let (db, cipher) = fresh().await;
        // Stage: provider + policy via the DB-thread closures.
        db.call(|db_inner| {
            let cipher = Cipher::for_tests();
            crate::runtime::tls::store::upsert_dns_provider(
                db_inner,
                &cipher,
                "p",
                super::super::DnsProviderKind::Route53,
                &SecretString::new(
                    r#"{"access_key_id":"AKIA","secret_access_key":"s","region":"us-east-1"}"#
                        .into(),
                ),
            )
            .unwrap();
            // The first provider auto-creates a `*` policy, which is what
            // we want here — the unknown hostname will resolve to it.
        });

        let token = "deadbeef".to_owned();
        let (addr, _handle) = spawn_server(
            db.clone(),
            Arc::new(cipher),
            token.clone(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();

        // Hit the endpoint with a fresh hostname; the daemon will try to
        // issue, find no contact email, file a fault, and 204 back.
        let url =
            format!("http://{addr}/cert/{token}/get-certificate?server_name=fresh.example.com");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status().as_u16(), 204);

        // Confirm the fault landed in the system app under our hostname.
        let active = db.call(|db_inner| {
            crate::runtime::faults::list_active_faults(
                db_inner,
                Some(&AppName::new_unchecked("_system")),
            )
            .unwrap_or_default()
        });
        let our_fault = active.iter().find(|f| {
            f.kind == "cert_issue_failed" && f.resource_name.as_deref() == Some("fresh.example.com")
        });
        assert!(
            our_fault.is_some(),
            "expected cert_issue_failed fault for fresh.example.com; got: {active:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_returns_204_for_unknown_hostname() {
        // Caddy's http cert getter treats 204 as "no cert; fall through".
        // Any other non-2xx (e.g. 404) would be an error, breaking the
        // intended HTTP-01 fallback for hostnames without a stored cert.
        let (db, cipher) = fresh().await;
        let token = "deadbeef".to_owned();
        let (addr, _handle) = spawn_server(
            db,
            Arc::new(cipher),
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

        let (addr, _handle) = spawn_server(
            db,
            Arc::new(cipher),
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
}
