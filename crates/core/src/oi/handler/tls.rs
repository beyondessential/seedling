//! Operator interface for the TLS certificate subsystem.
//!
//! Three sub-surfaces live here:
//!
//! - DNS providers (`/tls/dns-providers/*`) — credentials used by the
//!   ACME-DNS issuance flow.
//! - Policies (`/tls/policies/*`) — per-hostname strategy overrides.
//! - Certificates (`/tls/certificates/*`) — list, plus an explicit
//!   ACME-DNS issuance trigger.
//!
//! Manual cert upload and the CSR flow are added in phases 3/4.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};

use seedling_protocol::error::{ErrorCode, OiError};

use super::HandlerResult;
use crate::defs::resource::Resource;
use crate::oi::state::OiState;
use crate::runtime::tls::{
    self, AttemptOutcome, DnsProviderKind, RetryBlockSource, TlsCertAttempt, TlsCertOrigin,
    TlsCertState, TlsCertificate, TlsPolicy, TlsPolicyRow, store,
};

// ---------------------------------------------------------------------------
// DNS providers
// ---------------------------------------------------------------------------

// i[tls.dns-provider.list]
pub(crate) fn list_dns_providers(state: &OiState) -> HandlerResult {
    let rows = state.db.call(store::list_dns_providers).map_err(db_error)?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|p| {
            json!({
                "name": p.name,
                "kind": p.kind.as_str(),
                "created_at": p.created_at,
                "updated_at": p.updated_at,
            })
        })
        .collect();
    Ok(json!({ "providers": result }))
}

#[derive(Deserialize)]
pub(crate) struct UpsertDnsProviderParams {
    pub name: String,
    pub kind: String,
    /// Provider-specific config blob. For `kind = "route53"`:
    /// `{"access_key_id": ..., "secret_access_key": ..., "region": ...}`.
    pub config: Value,
}

// i[tls.dns-provider.upsert]
pub(crate) fn upsert_dns_provider(
    state: &OiState,
    params: UpsertDnsProviderParams,
) -> HandlerResult {
    let kind = DnsProviderKind::parse(&params.kind).ok_or_else(|| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("unknown dns provider kind: {}", params.kind),
        )
    })?;
    if params.name.trim().is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "name must not be empty",
        ));
    }
    let config_json = serde_json::to_string(&params.config).map_err(|e| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("config serialisation: {e}"),
        )
    })?;
    let config_secret = SecretString::new(config_json.into());
    let cipher = Arc::clone(&state.cipher);
    let name = params.name;
    let outcome = state
        .db
        .call(move |db| store::upsert_dns_provider(db, &cipher, &name, kind, &config_secret))
        .map_err(db_error)?;
    Ok(json!({
        "ok": true,
        "auto_policy_created": outcome.auto_policy_created,
    }))
}

#[derive(Deserialize)]
pub(crate) struct DnsProviderNameParams {
    pub name: String,
}

// i[tls.dns-provider.delete]
pub(crate) fn delete_dns_provider(state: &OiState, params: DnsProviderNameParams) -> HandlerResult {
    let name = params.name.clone();
    let removed = state
        .db
        .call(move |db| store::delete_dns_provider(db, &name))
        .map_err(|e| {
            // FK refusal surfaces as a sqlite ConstraintViolation; map it to
            // a clear precondition error so the operator sees "in use".
            if e.to_string().contains("FOREIGN KEY") {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!(
                        "DNS provider {} is referenced by one or more policies; clear them first",
                        params.name
                    ),
                )
            } else {
                db_error(e)
            }
        })?;
    if !removed {
        return Err(OiError::not_found(format!(
            "DNS provider not found: {}",
            params.name
        )));
    }
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Policies
// ---------------------------------------------------------------------------

// i[tls.policy.list]
pub(crate) fn list_policies(state: &OiState) -> HandlerResult {
    let rows = state.db.call(store::list_policies).map_err(db_error)?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|row| match row.policy {
            TlsPolicy::AcmeDns { dns_provider } => json!({
                "hostname": row.hostname,
                "strategy": "acme_dns",
                "dns_provider": dns_provider,
                "updated_at": row.updated_at,
            }),
            TlsPolicy::Manual { cert_id } => json!({
                "hostname": row.hostname,
                "strategy": "manual",
                "cert_id": cert_id,
                "updated_at": row.updated_at,
            }),
        })
        .collect();
    Ok(json!({ "policies": result }))
}

#[derive(Deserialize)]
pub(crate) struct SetAcmeDnsParams {
    /// Hostname or wildcard pattern (`*`, `*.example.com`).
    pub hostname: String,
    pub dns_provider: String,
}

// i[tls.policy.set-acme-dns]
pub(crate) fn set_policy_acme_dns(state: &OiState, params: SetAcmeDnsParams) -> HandlerResult {
    let SetAcmeDnsParams {
        hostname,
        dns_provider,
    } = params;

    let hostname_for_db = hostname.clone();
    let dns_provider_for_db = dns_provider.clone();
    state
        .db
        .call(move |db| store::set_policy_acme_dns(db, &hostname_for_db, &dns_provider_for_db))
        .map_err(db_error)?;

    // Issuance for an exact hostname kicks in via the issuance coordinator.
    // For wildcard policies there's no concrete hostname to issue against
    // until an ingress for a matching name appears, at which point the
    // reconciler hands it to the coordinator.
    let mut auto_issue_kicked = false;
    if !hostname.contains('*') {
        state.tls_coordinator.ensure(&hostname);
        auto_issue_kicked = true;
    }

    Ok(json!({ "ok": true, "auto_issue_kicked": auto_issue_kicked }))
}

#[derive(Deserialize)]
pub(crate) struct SetManualParams {
    pub hostname: String,
    pub cert_id: i64,
}

// i[tls.policy.set-manual]
pub(crate) fn set_policy_manual(state: &OiState, params: SetManualParams) -> HandlerResult {
    let SetManualParams { hostname, cert_id } = params;
    state
        .db
        .call(move |db| store::set_policy_manual(db, &hostname, cert_id))
        .map_err(db_error)?;
    Ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
pub(crate) struct HostnameParams {
    pub hostname: String,
}

// i[tls.policy.clear]
pub(crate) fn clear_policy(state: &OiState, params: HostnameParams) -> HandlerResult {
    let hostname = params.hostname.clone();
    let cleared = state
        .db
        .call(move |db| store::clear_policy(db, &hostname))
        .map_err(db_error)?;
    if !cleared {
        return Err(OiError::not_found(format!(
            "no policy for hostname {}",
            params.hostname
        )));
    }
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

// i[tls.cert.list]
pub(crate) fn list_certificates(state: &OiState) -> HandlerResult {
    let rows = state.db.call(store::list_certificates).map_err(db_error)?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id,
                "hostname": c.hostname,
                "state": c.state.as_str(),
                "origin": c.origin.as_str(),
                "key_type": c.key_type.as_str(),
                "issuer": c.issuer,
                "not_before": c.not_before,
                "not_after": c.not_after,
                "serial": c.serial,
                "self_signed": c.self_signed,
                "note": c.note,
                "acme_account_id": c.acme_account_id,
                "created_at": c.created_at,
                "updated_at": c.updated_at,
            })
        })
        .collect();
    Ok(json!({ "certificates": result }))
}

#[derive(Deserialize)]
pub(crate) struct IssueAcmeDnsParams {
    pub hostname: String,
}

/// Synchronously run an ACME-DNS issuance for a hostname via the issuance
/// coordinator. Blocks the OI worker for the duration of the flow
/// (typically tens of seconds). The hostname must resolve to an
/// `acme_dns` policy; the coordinator handles the rest, including
/// recording the attempt and filing a fault on failure.
///
/// `contact_email` and `directory_url` are no longer accepted as
/// per-call overrides — the global setting is the source of truth.
// i[tls.cert.issue-acme-dns]
pub(crate) fn issue_acme_dns(state: &OiState, params: IssueAcmeDnsParams) -> HandlerResult {
    let coord = Arc::clone(&state.tls_coordinator);
    let hostname = params.hostname;
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move { coord.issue_now(&hostname).await })
    });
    match result {
        Ok(issued) => Ok(json!({
            "cert_id": issued.cert_id,
            "not_after": issued.not_after,
        })),
        Err(e) => Err(OiError::new(
            ErrorCode::Internal,
            format!("acme-dns issuance failed: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub(crate) struct RetryParams {
    pub hostname: String,
}

/// Operator-driven retry: clears any operator pause for `hostname`,
/// records the persistent force-retry signal, and ensures the issuance
/// coordinator picks it up immediately. Returns immediately; the
/// reconciler / coordinator runs the actual ACME flow in the background.
/// Survives a daemon restart between this call and the actual flow:
/// the force-retry row stays in the DB until consumed.
// i[tls.cert.retry]
pub(crate) fn retry(state: &OiState, params: RetryParams) -> HandlerResult {
    let RetryParams { hostname } = params;
    let host_for_db = hostname.clone();
    state
        .db
        .call(move |db| -> rusqlite::Result<()> {
            store::clear_retry_block(db, &host_for_db)?;
            store::set_force_retry(db, &host_for_db)?;
            Ok(())
        })
        .map_err(db_error)?;
    state.tls_coordinator.ensure(&hostname);
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Hostname view (consolidated per-hostname rollup)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct ListHostnamesParams {
    /// Optional app filter; when set, only TLS-terminating hostnames
    /// declared by this app's ingresses are returned.
    #[serde(default)]
    pub app: Option<String>,
}

// i[tls.hostname.list]
// r[impl tls.cert.hostname-view]
pub(crate) fn list_hostnames(state: &OiState, params: ListHostnamesParams) -> HandlerResult {
    // Walk the registry to discover every TLS-terminating ingress hostname
    // and the apps declaring it. Multiple apps may declare the same
    // hostname (e.g. shared ingress); collect them all.
    let mut hostname_apps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    {
        let reg = state.registry.read();
        for entry in reg.iter() {
            if let Some(filter) = params.app.as_deref()
                && entry.name.as_str() != filter
            {
                continue;
            }
            let def = entry.app.def.load();
            for resource in def.resources.values() {
                if let Resource::Ingress(ing) = resource {
                    let ing_def = ing.def.lock();
                    if ing_def.tls {
                        hostname_apps
                            .entry(ing_def.hostname.clone())
                            .or_default()
                            .insert(entry.name.as_str().to_owned());
                    }
                }
            }
        }
    }

    // Single DB call collects everything we need to roll up per hostname.
    let snapshot = state
        .db
        .call(|db| -> rusqlite::Result<HostnameSnapshot> {
            Ok(HostnameSnapshot {
                policies: store::list_policies(db)?,
                certificates: store::list_certificates(db)?,
                attempts: store::list_attempts(db, None, 1000)?,
                retry_blocks: store::list_retry_blocks(db)?,
                force_retries: store::list_force_retries(db)?,
            })
        })
        .map_err(db_error)?;

    let block_by_host: HashMap<&str, &_> = snapshot
        .retry_blocks
        .iter()
        .map(|b| (b.hostname.as_str(), b))
        .collect();
    let force_by_host: HashMap<&str, i64> = snapshot
        .force_retries
        .iter()
        .map(|f| (f.hostname.as_str(), f.requested_at))
        .collect();

    // Most-recent active cert per hostname.
    let mut active_cert_by_host: HashMap<&str, &TlsCertificate> = HashMap::new();
    for cert in &snapshot.certificates {
        if cert.state == TlsCertState::Active {
            active_cert_by_host
                .entry(cert.hostname.as_str())
                .and_modify(|existing| {
                    if cert.created_at > existing.created_at {
                        *existing = cert;
                    }
                })
                .or_insert(cert);
        }
    }
    let cert_by_id: HashMap<i64, &TlsCertificate> =
        snapshot.certificates.iter().map(|c| (c.id, c)).collect();

    // First (newest) attempt per hostname; first success per hostname.
    // `list_attempts` returns newest-first, so the first hit is the most
    // recent.
    let mut last_attempt_by_host: HashMap<&str, &TlsCertAttempt> = HashMap::new();
    let mut last_success_by_host: HashMap<&str, &TlsCertAttempt> = HashMap::new();
    for att in &snapshot.attempts {
        last_attempt_by_host
            .entry(att.hostname.as_str())
            .or_insert(att);
        if att.outcome == AttemptOutcome::Success {
            last_success_by_host
                .entry(att.hostname.as_str())
                .or_insert(att);
        }
    }

    let now = jiff::Timestamp::now().as_second();
    let result: Vec<Value> = hostname_apps
        .into_iter()
        .map(|(hostname, apps)| {
            let resolved = resolve_policy_view(&hostname, &snapshot.policies);
            let active_cert = active_cert_by_host.get(hostname.as_str()).copied();
            let last_attempt = last_attempt_by_host.get(hostname.as_str()).copied();
            let last_success = last_success_by_host.get(hostname.as_str()).copied();
            let block = block_by_host.get(hostname.as_str()).copied();
            let force_retry_at = force_by_host.get(hostname.as_str()).copied();

            let policy_json = match &resolved {
                None => json!({ "strategy": "default" }),
                Some(r) => match &r.row.policy {
                    TlsPolicy::AcmeDns { dns_provider } => json!({
                        "strategy": "acme_dns",
                        "dns_provider": dns_provider,
                        "pattern": r.row.hostname,
                        "is_wildcard_match": r.row.hostname != hostname,
                    }),
                    TlsPolicy::Manual { cert_id } => json!({
                        "strategy": "manual",
                        "cert_id": cert_id,
                        "pattern": r.row.hostname,
                        "is_wildcard_match": r.row.hostname != hostname,
                    }),
                },
            };

            let active_cert_json =
                resolve_active_cert(active_cert, &resolved, &cert_by_id).map_or(Value::Null, |c| {
                    json!({
                        "id": c.id,
                        "origin": c.origin.as_str(),
                        "issuer": c.issuer,
                        "not_before": c.not_before,
                        "not_after": c.not_after,
                        "self_signed": c.self_signed,
                        "ari_window_start": c.ari_window_start,
                        "ari_window_end": c.ari_window_end,
                    })
                });

            let last_issuance_json = build_last_issuance(
                resolve_active_cert(active_cert, &resolved, &cert_by_id),
                last_success,
                resolved.as_ref(),
            );

            let last_error = last_attempt
                .filter(|a| a.outcome == AttemptOutcome::Failure)
                .and_then(|a| a.error.clone());

            let status = compute_status(
                &resolved,
                resolve_active_cert(active_cert, &resolved, &cert_by_id),
                last_attempt,
                block.is_some(),
                force_retry_at.is_some(),
                now,
            );

            let (next_issuance_at, next_issuance_source) = compute_next_issuance(
                &resolved,
                resolve_active_cert(active_cert, &resolved, &cert_by_id),
            );

            let block_json = block.map_or(
                Value::Null,
                |b| json!({ "set_at": b.set_at, "reason": b.reason }),
            );

            json!({
                "hostname": hostname,
                "apps": apps.into_iter().collect::<Vec<_>>(),
                "policy": policy_json,
                "status": status,
                "active_cert": active_cert_json,
                "last_issuance": last_issuance_json,
                "last_error": last_error,
                "retry_block": block_json,
                "force_retry_at": force_retry_at,
                "next_issuance_at": next_issuance_at,
                "next_issuance_source": next_issuance_source,
            })
        })
        .collect();

    Ok(json!({ "hostnames": result }))
}

struct HostnameSnapshot {
    policies: Vec<TlsPolicyRow>,
    certificates: Vec<TlsCertificate>,
    attempts: Vec<TlsCertAttempt>,
    retry_blocks: Vec<crate::runtime::tls::TlsCertRetryBlock>,
    force_retries: Vec<crate::runtime::tls::TlsCertForceRetry>,
}

struct ResolvedPolicy {
    row: TlsPolicyRow,
}

/// Pick the most-specific policy that matches `hostname` from the supplied
/// list. Mirrors `store::resolve_policy` without re-querying.
fn resolve_policy_view(hostname: &str, policies: &[TlsPolicyRow]) -> Option<ResolvedPolicy> {
    let mut best: Option<(u32, &TlsPolicyRow)> = None;
    for row in policies {
        if tls::pattern_matches(&row.hostname, hostname) {
            let score = tls::pattern_specificity(&row.hostname);
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, row));
            }
        }
    }
    best.map(|(_, row)| ResolvedPolicy { row: row.clone() })
}

/// For a manual policy, the active cert is the one referenced by the
/// policy regardless of hostname column. For ACME-DNS / default, fall
/// through to the "active cert with this hostname" lookup.
fn resolve_active_cert<'a>(
    by_host: Option<&'a TlsCertificate>,
    resolved: &Option<ResolvedPolicy>,
    by_id: &HashMap<i64, &'a TlsCertificate>,
) -> Option<&'a TlsCertificate> {
    if let Some(r) = resolved
        && let TlsPolicy::Manual { cert_id } = &r.row.policy
    {
        return by_id.get(cert_id).copied();
    }
    by_host
}

fn build_last_issuance(
    active: Option<&TlsCertificate>,
    last_success: Option<&TlsCertAttempt>,
    resolved: Option<&ResolvedPolicy>,
) -> Value {
    if let Some(cert) = active
        && cert.origin == TlsCertOrigin::Manual
    {
        return json!({
            "kind": "manual",
            "at": cert.created_at,
            "cert_id": cert.id,
        });
    }
    if let Some(att) = last_success {
        let provider = resolved.and_then(|r| match &r.row.policy {
            TlsPolicy::AcmeDns { dns_provider } => Some(dns_provider.clone()),
            TlsPolicy::Manual { .. } => None,
        });
        return json!({
            "kind": "acme_dns",
            "at": att.started_at,
            "cert_id": att.cert_id,
            "provider": provider,
        });
    }
    if let Some(cert) = active {
        return json!({
            "kind": cert.origin.as_str(),
            "at": cert.created_at,
            "cert_id": cert.id,
        });
    }
    Value::Null
}

fn compute_status(
    resolved: &Option<ResolvedPolicy>,
    active: Option<&TlsCertificate>,
    last_attempt: Option<&TlsCertAttempt>,
    has_block: bool,
    force_retry: bool,
    now: i64,
) -> &'static str {
    if has_block {
        return "blocked";
    }
    if let Some(cert) = active {
        if let Some(na) = cert.not_after
            && now > na
        {
            return "expired";
        }
        return "active";
    }
    if force_retry {
        return "pending";
    }
    if let Some(att) = last_attempt {
        return match att.outcome {
            AttemptOutcome::Success => "active",
            AttemptOutcome::Failure => "error",
            AttemptOutcome::Pending => "pending",
        };
    }
    match resolved.as_ref().map(|r| &r.row.policy) {
        None => "default",
        Some(TlsPolicy::AcmeDns { .. }) => "pending",
        Some(TlsPolicy::Manual { .. }) => "no_cert",
    }
}

/// Compute the expected next issuance date for the hostname.
///
/// - ACME-DNS with active cert + ARI window → window start, source `ari`.
/// - ACME-DNS with active cert, no ARI → 1/3-of-lifetime mark, source `fallback`.
/// - ACME-DNS with no active cert → `None` ts, source `immediate`.
/// - Manual / default → `None`, source `None`.
fn compute_next_issuance(
    resolved: &Option<ResolvedPolicy>,
    active: Option<&TlsCertificate>,
) -> (Option<i64>, Option<&'static str>) {
    let is_acme_dns = matches!(
        resolved.as_ref().map(|r| &r.row.policy),
        Some(TlsPolicy::AcmeDns { .. })
    );
    if !is_acme_dns {
        return (None, None);
    }
    match active {
        None => (None, Some("immediate")),
        Some(cert) => {
            if let Some(start) = cert.ari_window_start {
                return (Some(start), Some("ari"));
            }
            let (Some(nb), Some(na)) = (cert.not_before, cert.not_after) else {
                return (None, Some("fallback"));
            };
            // Fallback matches the issuance-coordinator threshold: renew
            // when remaining lifetime drops below 1/3 of total. Equivalent
            // to firing at not_before + 2/3 * lifetime.
            let lifetime = na - nb;
            let due = nb + (lifetime * 2 / 3);
            (Some(due), Some("fallback"))
        }
    }
}

// ---------------------------------------------------------------------------
// Cert attempt log
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct ListAttemptsParams {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default = "default_attempts_limit")]
    pub limit: i64,
}

fn default_attempts_limit() -> i64 {
    100
}

// i[tls.cert.attempts.list]
pub(crate) fn list_attempts(state: &OiState, params: ListAttemptsParams) -> HandlerResult {
    let ListAttemptsParams { hostname, limit } = params;
    let rows = state
        .db
        .call(move |db| store::list_attempts(db, hostname.as_deref(), limit))
        .map_err(db_error)?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|a| {
            json!({
                "id": a.id,
                "hostname": a.hostname,
                "triggered_by": a.triggered_by.as_str(),
                "started_at": a.started_at,
                "finished_at": a.finished_at,
                "outcome": a.outcome.as_str(),
                "cert_id": a.cert_id,
                "error": a.error,
            })
        })
        .collect();
    Ok(json!({ "attempts": result }))
}

// ---------------------------------------------------------------------------
// Retry blocks
// ---------------------------------------------------------------------------

// i[tls.retry-block.list]
pub(crate) fn list_retry_blocks(state: &OiState) -> HandlerResult {
    let rows = state.db.call(store::list_retry_blocks).map_err(db_error)?;
    let result: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "hostname": r.hostname,
                "set_at": r.set_at,
                "set_by": r.set_by.as_str(),
                "reason": r.reason,
            })
        })
        .collect();
    Ok(json!({ "blocks": result }))
}

#[derive(Deserialize)]
pub(crate) struct SetRetryBlockParams {
    pub hostname: String,
    #[serde(default)]
    pub reason: Option<String>,
}

// i[tls.retry-block.set]
pub(crate) fn set_retry_block(state: &OiState, params: SetRetryBlockParams) -> HandlerResult {
    let SetRetryBlockParams { hostname, reason } = params;
    state
        .db
        .call(move |db| {
            store::set_retry_block(db, &hostname, RetryBlockSource::Operator, reason.as_deref())
        })
        .map_err(db_error)?;
    Ok(json!({ "ok": true }))
}

// i[tls.retry-block.clear]
pub(crate) fn clear_retry_block(state: &OiState, params: HostnameParams) -> HandlerResult {
    let HostnameParams { hostname } = params;
    let cleared = state
        .db
        .call(move |db| store::clear_retry_block(db, &hostname))
        .map_err(db_error)?;
    Ok(json!({ "ok": true, "cleared": cleared }))
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

// i[tls.settings.get]
pub(crate) fn get_settings(state: &OiState) -> HandlerResult {
    let s = state.db.call(store::get_settings).map_err(db_error)?;
    Ok(json!({
        "contact_email": s.contact_email,
        "updated_at": s.updated_at,
    }))
}

#[derive(Deserialize)]
pub(crate) struct SetSettingsParams {
    pub contact_email: String,
}

// i[tls.settings.set]
pub(crate) fn set_settings(state: &OiState, params: SetSettingsParams) -> HandlerResult {
    state
        .db
        .call(move |db| store::set_contact_email(db, &params.contact_email))
        .map_err(db_error)?;
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_error(e: rusqlite::Error) -> OiError {
    OiError::new(ErrorCode::NotFound, format!("db error: {e}"))
}
