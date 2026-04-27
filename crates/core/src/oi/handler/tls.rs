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
use crate::runtime::tls::{
    DnsProviderKind, TlsPolicy,
    acme::{self, IssueParams},
    store,
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
    /// Optional override for the ACME directory URL. Defaults to Let's
    /// Encrypt production. Persisted only via the resulting account row.
    #[serde(default)]
    pub directory_url: Option<String>,
}

// i[tls.policy.set-acme-dns]
pub(crate) fn set_policy_acme_dns(state: &OiState, params: SetAcmeDnsParams) -> HandlerResult {
    let SetAcmeDnsParams {
        hostname,
        dns_provider,
        directory_url,
    } = params;

    let hostname_for_db = hostname.clone();
    let dns_provider_for_db = dns_provider.clone();
    state
        .db
        .call(move |db| store::set_policy_acme_dns(db, &hostname_for_db, &dns_provider_for_db))
        .map_err(db_error)?;

    // Auto-issue is only meaningful for exact hostnames. Wildcard policies
    // have no concrete name to acquire a cert for; first issuance happens
    // lazily when an ingress for a matching hostname appears or via the
    // explicit /tls/certificates/issue-acme-dns endpoint.
    let mut auto_issue_kicked = false;
    let is_exact = !hostname.contains('*');
    if is_exact {
        // Look up the global contact email; if none is configured, leave
        // auto-issue to the operator (they'll set the email and retry, or
        // run the explicit issue command).
        let settings = state.db.call(store::get_settings).map_err(db_error)?;
        if !settings.contact_email.is_empty() {
            let hostname_for_check = hostname.clone();
            let existing = state
                .db
                .call(move |db| store::find_active_for_hostname(db, &hostname_for_check))
                .map_err(db_error)?;
            if existing.is_none() {
                let directory_url = directory_url.unwrap_or_else(acme::default_directory_url);
                let db = state.db.clone();
                let cipher = Arc::clone(&state.cipher);
                let hostname_for_task = hostname.clone();
                let dns_provider_for_task = dns_provider.clone();
                let contact_for_task = settings.contact_email.clone();
                tokio::spawn(async move {
                    let result = acme::issue(
                        &db,
                        &cipher,
                        IssueParams {
                            hostname: &hostname_for_task,
                            contact_email: &contact_for_task,
                            directory_url: &directory_url,
                            dns_provider_name: &dns_provider_for_task,
                        },
                    )
                    .await;
                    match result {
                        Ok(issued) => tracing::info!(
                            hostname = %hostname_for_task,
                            cert_id = issued.cert_id,
                            not_after = issued.not_after,
                            "auto-issued cert on policy set"
                        ),
                        Err(e) => tracing::warn!(
                            hostname = %hostname_for_task,
                            error = %e,
                            "auto-issue on policy set failed; operator can retry via /tls/certificates/issue-acme-dns"
                        ),
                    }
                });
                auto_issue_kicked = true;
            }
        }
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
    /// Optional override of the global contact email setting. Errors when
    /// neither this field nor the global setting is populated.
    #[serde(default)]
    pub contact_email: Option<String>,
    /// Optional override; defaults to Let's Encrypt production.
    #[serde(default)]
    pub directory_url: Option<String>,
}

/// Synchronously run an ACME-DNS issuance for a hostname. Blocks the OI
/// thread for the duration of the flow (typically tens of seconds).
/// The hostname must already be bound to an `acme_dns` policy with a
/// configured DNS provider; this trigger is what the operator runs after
/// setting the policy to acquire the first cert.
// i[tls.cert.issue-acme-dns]
pub(crate) fn issue_acme_dns(state: &OiState, params: IssueAcmeDnsParams) -> HandlerResult {
    // Resolve the hostname's policy via wildcard rules so an operator can
    // issue against `foo.example.com` whenever a `*.example.com` or `*`
    // policy applies, without needing to add an explicit exact-match
    // policy first.
    let hostname_for_lookup = params.hostname.clone();
    let policy = state
        .db
        .call(move |db| store::resolve_policy(db, &hostname_for_lookup))
        .map_err(db_error)?;
    let dns_provider_name = match policy.map(|p| p.policy) {
        Some(TlsPolicy::AcmeDns { dns_provider }) => dns_provider,
        Some(_) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "hostname {} resolves to a manual policy; bind it to acme_dns first",
                    params.hostname
                ),
            ));
        }
        None => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "hostname {} has no matching policy; bind it via /tls/policies/set-acme-dns first",
                    params.hostname
                ),
            ));
        }
    };

    // Contact email: use the explicit param if supplied, else fall back to
    // the global tls_settings row.
    let contact_email = match params.contact_email {
        Some(c) if !c.is_empty() => c,
        _ => {
            let settings = state.db.call(store::get_settings).map_err(db_error)?;
            if settings.contact_email.is_empty() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "no contact email: supply contact_email or set the global one via /tls/settings/set",
                ));
            }
            settings.contact_email
        }
    };

    let directory_url = params
        .directory_url
        .unwrap_or_else(acme::default_directory_url);

    // Run the async ACME flow on the current Tokio runtime, blocking the OI
    // worker for the duration. ACME-DNS issuance takes tens of seconds; we
    // accept that for the sync UX.
    let db = state.db.clone();
    let cipher = Arc::clone(&state.cipher);
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            acme::issue(
                &db,
                &cipher,
                IssueParams {
                    hostname: &params.hostname,
                    contact_email: &contact_email,
                    directory_url: &directory_url,
                    dns_provider_name: &dns_provider_name,
                },
            )
            .await
        })
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
