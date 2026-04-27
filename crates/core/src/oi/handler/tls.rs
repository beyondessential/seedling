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

use std::sync::Arc;

use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};

use seedling_protocol::error::{ErrorCode, OiError};

use super::HandlerResult;
use crate::oi::state::OiState;
use crate::runtime::tls::{DnsProviderKind, RetryBlockSource, TlsPolicy, store};

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
