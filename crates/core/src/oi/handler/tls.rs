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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};

use seedling_protocol::error::{ErrorCode, OiError};

use super::HandlerResult;
use crate::oi::state::OiState;
use crate::runtime::tls::{
    AttemptOutcome, DnsProviderKind, KeyType, RetryBlockSource, TlsCertOrigin, TlsCertState,
    TlsPolicy,
    chain::{self, Strategy},
    keypair, parse, state,
    state::NextSource,
    store,
    store::CertMetadata,
    validate::{self, ValidateError},
};

#[cfg(test)]
mod tests;

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
            // r[impl ingress.site.tailscale]
            TlsPolicy::Tailscale => json!({
                "hostname": row.hostname,
                "strategy": "tailscale",
                "updated_at": row.updated_at,
            }),
            // r[impl tls.strategy.canopy-only]
            TlsPolicy::Canopy => json!({
                "hostname": row.hostname,
                "strategy": "canopy",
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
/// Validate a TLS policy hostname pattern.
///
/// `r[tls.policy.wildcard]` defines the shapes: the catch-all `*`, a
/// left-anchored `*.<suffix>`, or an exact hostname. Anything else — an empty
/// string, a bare `*.`, a name with a stray character — produced a stored
/// policy that could never match any hostname, indistinguishable in the
/// policy list from one that works.
// r[impl tls.policy.wildcard]
fn validate_policy_hostname(hostname: &str) -> Result<(), OiError> {
    let ok = if hostname == "*" {
        true
    } else if let Some(suffix) = hostname.strip_prefix("*.") {
        super::is_valid_dns_name(suffix)
    } else {
        super::is_valid_dns_name(hostname)
    };
    if ok {
        Ok(())
    } else {
        Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("hostname {hostname:?} must be `*`, `*.<suffix>`, or a valid hostname"),
        ))
    }
}

pub(crate) fn set_policy_acme_dns(state: &OiState, params: SetAcmeDnsParams) -> HandlerResult {
    let SetAcmeDnsParams {
        hostname,
        dns_provider,
    } = params;

    // A pattern that matches nothing is a policy row nothing will ever use,
    // and it looked identical to a working one in the policy list.
    validate_policy_hostname(&hostname)?;

    let hostname_for_db = hostname.clone();
    let dns_provider_for_db = dns_provider.clone();
    let provider_for_check = dns_provider.clone();
    state
        .db
        .call(move |db| {
            // Checked here rather than left to the foreign key: the FK refusal
            // arrived as `not_found: db error: FOREIGN KEY constraint failed`,
            // which names neither the provider nor what to do about it, and
            // gave a client the wrong error code to key off. Inside the same
            // closure, so nothing can delete the provider in between.
            if store::get_dns_provider_raw(db, &provider_for_check)?.is_none() {
                return Ok(false);
            }
            store::set_policy_acme_dns(db, &hostname_for_db, &dns_provider_for_db).map(|()| true)
        })
        .map_err(db_error)?
        .then_some(())
        .ok_or_else(|| {
            OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("DNS provider {dns_provider:?} does not exist; add it first"),
            )
        })?;

    // Issuance for an exact hostname kicks in via the issuance coordinator.
    // For wildcard policies there's no concrete hostname to issue against
    // until an ingress for a matching name appears, at which point the
    // reconciler hands it to the coordinator.
    decisions_have_moved(state);
    let mut auto_issue_kicked = false;
    if !hostname.contains('*') {
        state.tls_coordinator.ensure(&hostname);
        auto_issue_kicked = true;
    }

    Ok(json!({ "ok": true, "auto_issue_kicked": auto_issue_kicked }))
}

#[derive(Deserialize)]
pub(crate) struct HostnameParams {
    pub hostname: String,
}

// i[tls.policy.set-canopy]
// r[impl tls.strategy.canopy-only]
pub(crate) fn set_policy_canopy(state: &OiState, params: HostnameParams) -> HandlerResult {
    let hostname = params.hostname;
    validate_policy_hostname(&hostname)?;
    let for_db = hostname.clone();
    state
        .db
        .call(move |db| store::set_policy_canopy(db, &for_db))
        .map_err(db_error)?;
    // A usable certificate is left until its renewal point, and a hostname with
    // none is picked up by the next Canopy report tick, so there is nothing to
    // kick here. The chain the proxy is told about does change, though.
    decisions_have_moved(state);
    Ok(json!({ "ok": true }))
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
    decisions_have_moved(state);
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
            // Whether the stored certificate covers the hostname its CSR asked
            // for. Decided here so the RFC 6125 rule has one implementation
            // rather than one per client. `null` where there is nothing to
            // decide — no request, no certificate yet — or where the stored
            // PEM will not parse, which is not the same as "does not cover".
            // r[impl tls.csr.flow]
            let request_covered = match (c.requested_hostname.as_deref(), c.cert_pem.as_deref()) {
                (Some(requested), Some(pem)) => match parse::cert_covers(pem, requested) {
                    Ok(covered) => Some(covered),
                    // Null here reads as "no flag" to the operator, same as a
                    // met request, so the reason has to be said somewhere.
                    Err(e) => {
                        tracing::warn!(
                            cert_id = c.id,
                            error = %e,
                            "stored certificate could not be parsed; cannot report whether it \
                             covers the hostname its CSR requested"
                        );
                        None
                    }
                },
                _ => None,
            };
            json!({
                "id": c.id,
                "hostname": c.hostname,
                "requested_hostname": c.requested_hostname,
                "request_covered": request_covered,
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

/// What a validated operator-supplied certificate becomes in storage.
///
/// A certificate binds to hostnames by SAN coverage alone, so its row is
/// labelled with — and supersedes under — its own primary SAN, never a name
/// the operator asked for. Both upload paths build this, so neither can
/// reintroduce a label the certificate does not carry.
// r[impl tls.cert.validation.san-coverage]
struct StoredCert {
    primary_san: String,
    chain_pem: String,
    metadata: CertMetadata,
}

impl StoredCert {
    fn from_validated(validated: &validate::Validated) -> Self {
        Self {
            primary_san: validated
                .parsed
                .san_dns_names
                .first()
                .cloned()
                .expect("validate_upload rejects empty SAN lists"),
            chain_pem: validated.parsed.chain_pem.clone(),
            metadata: validated.parsed.metadata.clone(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct UploadManualParams {
    pub cert_pem: String,
    pub key_pem: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Upload an operator-supplied (cert, key) pair. The cert auto-binds
/// to whatever hostnames its SANs cover at resolution time; the row's
/// `hostname` column is just the first SAN, kept as a primary label
/// for display. The pair is validated per [`tls.cert.validation.*`]
/// and the supplied key's public key is confirmed to match the leaf
/// cert's `SubjectPublicKeyInfo`. Stored with the key encrypted at
/// rest. Returns `{ id, warnings }` (warnings: `self_signed`,
/// `not_yet_valid`, …).
// i[tls.cert.upload-manual]
// r[impl tls.strategy.manual]
pub(crate) fn upload_manual(state: &OiState, params: UploadManualParams) -> HandlerResult {
    let UploadManualParams {
        cert_pem,
        key_pem,
        note,
    } = params;

    let key_secret = SecretString::new(key_pem.into());
    let validated =
        validate::validate_upload(&cert_pem, &key_secret).map_err(map_validate_error)?;

    let StoredCert {
        primary_san,
        chain_pem,
        metadata,
    } = StoredCert::from_validated(&validated);
    let key_type = validated.key_type;

    let cipher = Arc::clone(&state.cipher);
    let key_ciphertext = cipher.encrypt(&key_secret).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("private key encryption failed: {e}"),
        )
    })?;

    let label_for_insert = primary_san.clone();
    let note_for_insert = note;
    let id = state
        .db
        .call(move |db| -> rusqlite::Result<i64> {
            store::insert_and_supersede(
                db,
                store::NewCertificate {
                    hostname: &label_for_insert,
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Manual,
                    cert_pem: Some(&chain_pem),
                    csr_pem: None,
                    key_ciphertext: &key_ciphertext,
                    key_type,
                    metadata,
                    note: note_for_insert.as_deref(),
                    acme_account_id: None,
                },
            )
        })
        .map_err(db_error)?;

    decisions_have_moved(state);
    Ok(json!({
        "id": id,
        "primary_san": primary_san,
        "san_dns_names": validated.parsed.san_dns_names,
        "warnings": validated.warnings,
    }))
}

#[derive(Deserialize)]
pub(crate) struct CertIdParams {
    pub id: i64,
}

/// Delete a stored certificate. Refused with a clear precondition
/// error when a policy still references the row, so the operator
/// gets pointed at the correct unbinding step.
// i[tls.cert.delete]
pub(crate) fn delete_certificate(state: &OiState, params: CertIdParams) -> HandlerResult {
    let CertIdParams { id } = params;
    let removed = state
        .db
        .call(move |db| store::delete_certificate(db, id))
        .map_err(|e| {
            if e.to_string().contains("FOREIGN KEY") {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!(
                        "certificate {id} is referenced by a manual policy; clear the policy first"
                    ),
                )
            } else {
                db_error(e)
            }
        })?;
    if !removed {
        return Err(OiError::not_found(format!("certificate not found: {id}")));
    }
    decisions_have_moved(state);
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// CSR flow
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct CsrBeginParams {
    pub hostname: String,
    /// Optional key type. Defaults to `ecdsa_p256`, the only kind
    /// currently supported.
    #[serde(default)]
    pub key_type: Option<String>,
}

/// Generate a server-side keypair and a CSR for `hostname`. The
/// private key is encrypted at rest and never leaves the runtime; the
/// CSR is returned in the response and is also retrievable later via
/// `tls.cert.csr.get` while the row is still in `csr_pending`. After
/// the operator has the CSR signed externally, they upload the signed
/// cert via `tls.cert.csr.upload-cert` to transition the row to
/// `active`.
// i[tls.cert.csr.begin]
// r[impl tls.csr.flow]
pub(crate) fn csr_begin(state: &OiState, params: CsrBeginParams) -> HandlerResult {
    let CsrBeginParams { hostname, key_type } = params;
    if hostname.trim().is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "hostname must not be empty",
        ));
    }
    let key_type = match key_type.as_deref() {
        None | Some("ecdsa_p256") => KeyType::EcdsaP256,
        Some(other) => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("unsupported key type: {other}"),
            ));
        }
    };

    let generated = keypair::generate(key_type).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("keypair generation failed: {e}"),
        )
    })?;
    let csr = keypair::build_csr(&hostname, &generated.inner)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("csr generation failed: {e}")))?;

    let cipher = Arc::clone(&state.cipher);
    let key_ciphertext = cipher.encrypt(&generated.pem).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("private key encryption failed: {e}"),
        )
    })?;

    let host_for_insert = hostname.clone();
    let csr_pem_for_insert = csr.pem.clone();
    let id = state
        .db
        .call(move |db| {
            store::insert_certificate(
                db,
                store::NewCertificate {
                    // Until a certificate arrives the row has no primary SAN
                    // of its own, so the requested name stands in as its
                    // label. The upload replaces it with the SAN the signed
                    // certificate actually carries.
                    hostname: &host_for_insert,
                    requested_hostname: Some(&host_for_insert),
                    state: TlsCertState::CsrPending,
                    origin: TlsCertOrigin::Csr,
                    cert_pem: None,
                    csr_pem: Some(&csr_pem_for_insert),
                    key_ciphertext: &key_ciphertext,
                    key_type,
                    metadata: CertMetadata::default(),
                    note: None,
                    acme_account_id: None,
                },
            )
        })
        .map_err(db_error)?;

    Ok(json!({
        "id": id,
        "csr_pem": csr.pem,
    }))
}

/// Re-fetch the PEM CSR for a pending row. Returns `not_found` when the
/// id is unknown, and `requirements_invalid` when the row is no longer
/// in `csr_pending` (cert already uploaded or row cancelled).
// i[tls.cert.csr.get]
pub(crate) fn csr_get(state: &OiState, params: CertIdParams) -> HandlerResult {
    let CertIdParams { id } = params;
    let cert = state
        .db
        .call(move |db| store::get_certificate(db, id))
        .map_err(db_error)?;
    let Some(cert) = cert else {
        return Err(OiError::not_found(format!("certificate not found: {id}")));
    };
    if cert.state != TlsCertState::CsrPending {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "certificate {id} is in state {} — no pending CSR to return",
                cert.state.as_str()
            ),
        ));
    }
    let Some(csr_pem) = cert.csr_pem else {
        return Err(OiError::new(
            ErrorCode::Internal,
            format!("certificate {id} is csr_pending but has no stored CSR"),
        ));
    };
    Ok(json!({ "id": id, "csr_pem": csr_pem }))
}

#[derive(Deserialize)]
pub(crate) struct CsrUploadCertParams {
    pub id: i64,
    pub cert_pem: String,
}

/// Upload the externally-signed certificate for a pending CSR. The
/// runtime decrypts the stored private key, verifies that the leaf
/// cert's SubjectPublicKeyInfo matches the stored key, runs the same
/// validation as a manual upload, and on success transitions the row to
/// `active` under the label the signed certificate carries. Any prior
/// active certificate under that same label is superseded — never one
/// serving a hostname the arriving certificate does not cover.
///
/// The hostname the CSR was requested for is a record of the request, not
/// a binding: an issued certificate that does not cover it is still stored
/// and still serves what it does cover, with `request_not_covered` in the
/// response warnings.
// i[tls.cert.csr.upload-cert]
// r[impl tls.csr.flow]
pub(crate) fn csr_upload_cert(state: &OiState, params: CsrUploadCertParams) -> HandlerResult {
    let CsrUploadCertParams { id, cert_pem } = params;

    // Pull the row.
    let cert_row = state
        .db
        .call(move |db| store::get_certificate(db, id))
        .map_err(db_error)?
        .ok_or_else(|| OiError::not_found(format!("certificate not found: {id}")))?;
    if cert_row.state != TlsCertState::CsrPending {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "certificate {id} is in state {}; cert upload only applies to csr_pending rows",
                cert_row.state.as_str()
            ),
        ));
    }

    let cipher = Arc::clone(&state.cipher);
    let stored_key = cipher.decrypt(&cert_row.key_ciphertext).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("private key decryption failed: {e}"),
        )
    })?;

    // Validation reuses the manual-upload rules: SAN-list non-empty, expiry,
    // and (here against the CSR's stored key) SPKI match. The SPKI match
    // proves the certificate was issued for this CSR's keypair; it says
    // nothing about which names the CA chose to sign.
    let validated =
        validate::validate_upload(&cert_pem, &stored_key).map_err(map_validate_error)?;

    let StoredCert {
        primary_san,
        chain_pem,
        metadata,
    } = StoredCert::from_validated(&validated);

    // A CA may sign a name set other than the one requested. The certificate
    // is still a real one for a key the runtime holds, so it is accepted and
    // binds to whatever it does cover; what it cannot do is claim the
    // requested hostname. Report the unmet request instead: the hostname
    // keeps whichever certificate is already serving it, and goes on showing
    // as uncovered in the per-hostname rollup until one arrives.
    // r[impl tls.csr.flow]
    // r[impl tls.cert.validation.san-coverage]
    let requested = cert_row
        .requested_hostname
        .as_deref()
        // Rows predating the column were backfilled from `hostname`, which is
        // where a pending CSR's requested name has always lived.
        .unwrap_or(cert_row.hostname.as_str());
    let mut warnings = validated.warnings.clone();
    if !parse::san_covers(&validated.parsed.san_dns_names, requested) {
        warnings.push("request_not_covered");
    }

    let label_for_update = primary_san.clone();
    let activated = state
        .db
        .call(move |db| {
            store::activate_pending_csr(db, id, &label_for_update, &chain_pem, &metadata)
        })
        .map_err(db_error)?;

    // The row was read, its key decrypted and the certificate validated before
    // this point, none of it under a lock. A cancellation landing in that
    // window means there is nothing to activate, and saying so beats reporting
    // a certificate stored that was not.
    if !activated {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("certificate {id} is no longer awaiting a CSR upload"),
        ));
    }

    decisions_have_moved(state);
    Ok(json!({
        "id": id,
        "primary_san": primary_san,
        "san_dns_names": validated.parsed.san_dns_names,
        "warnings": warnings,
    }))
}

/// Cancel a pending CSR row. Refused for any state other than
/// `csr_pending` so an operator cannot accidentally drop an active
/// cert via this path; deletion of an active cert goes through
/// `tls.cert.delete` instead.
// i[tls.cert.csr.cancel]
// r[impl tls.csr.flow]
pub(crate) fn csr_cancel(state: &OiState, params: CertIdParams) -> HandlerResult {
    let CertIdParams { id } = params;
    // One call, so no upload can activate the row between the check and the
    // deletion.
    match state
        .db
        .call(move |db| store::cancel_pending_csr(db, id))
        .map_err(db_error)?
    {
        store::CsrCancel::Cancelled => Ok(json!({ "ok": true })),
        store::CsrCancel::NotFound => {
            Err(OiError::not_found(format!("certificate not found: {id}")))
        }
        store::CsrCancel::NotPending(cert_state) => Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "certificate {id} is in state {}; csr_cancel only applies to csr_pending rows",
                cert_state.as_str()
            ),
        )),
    }
}

/// Forget the chain decisions, because what was just written is something they
/// rest on.
///
/// Every handler that writes a policy or a certificate calls this. The
/// decisions are made against one reading of the world, held for a window, and
/// served from there to the certificate endpoint, to the proxy's renewal points
/// and to an operator retry: without this they would all go on acting on the
/// world as it was before the write.
// r[impl tls.chain.cycle]
fn decisions_have_moved(state: &OiState) {
    state.tls_coordinator.forget_chain_state();
}

fn map_validate_error(e: ValidateError) -> OiError {
    OiError::new(ErrorCode::RequirementsInvalid, e.to_string())
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
    // The decision below is to rest on what was just written, not on what the
    // coordinator read up to half a minute ago.
    decisions_have_moved(state);
    // A hostname on a chain Canopy is not part of has nothing else to take the
    // retry; the coordinator knows which that is.
    state.tls_coordinator.settle_retry(&hostname);
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
    // Use the same managed-ingresses enumeration the issuance
    // coordinator and expiry sweep consume, so the rollup cannot
    // disagree with them about which hostnames the runtime is
    // managing. A hostname may be claimed by more than one app (shared
    // ingress); collect all of them.
    let mut hostname_apps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let all_managed = {
        let reg = state.registry.read();
        state::managed_ingresses(&reg)
    };
    for managed in &all_managed {
        if let Some(filter) = params.app.as_deref()
            && managed.app.as_str() != filter
        {
            continue;
        }
        hostname_apps
            .entry(managed.hostname.clone())
            .or_default()
            .insert(managed.app.as_str().to_owned());
    }

    // Load the unified snapshot once. The OI then asks the same
    // `compute_state` function the issuance coordinator uses, projecting
    // the result into the JSON shape the operator UI expects. Because
    // both call sites read from the same function, what the operator
    // sees here is exactly what the runtime will do.
    // The chain depends on what Canopy and the proxy are doing as well as on
    // the database, and on every app declaring a hostname, not just the ones
    // the filter keeps.
    let env = state
        .chain_env_for_display(&all_managed)
        .map_err(db_error)?;
    let snapshot = state
        .db
        .call(move |db| state::Snapshot::load(db, env))
        .map_err(db_error)?;

    let result: Vec<Value> = hostname_apps
        .into_iter()
        .map(|(hostname, apps)| {
            let st = state::compute_state(&snapshot, &hostname);
            // What the proxy holds was read when the chain's facts were
            // gathered, so it is taken from there rather than read and parsed a
            // second time for the display.
            let caddy_cert = match snapshot.env.proxy.get(&hostname) {
                Some(state::ProxyHolding::Holds(cert)) => Some(cert),
                Some(state::ProxyHolding::Nothing | state::ProxyHolding::Unreadable) | None => None,
            };
            project_hostname_state(&hostname, &apps, &st, caddy_cert)
        })
        .collect();

    Ok(json!({ "hostnames": result }))
}

/// Render a [`state::HostnameState`] as the JSON the OI rollup serves.
/// `caddy_cert` is what the proxy holds in its own cache, so the rollup can
/// show issuer / expiry / "last issued" for certificates the runtime does not
/// own.
fn project_hostname_state(
    hostname: &str,
    apps: &BTreeSet<String>,
    st: &state::HostnameState<'_>,
    caddy_cert: Option<&state::ProxyCert>,
) -> Value {
    let policy_json = match st.policy.map(|p| (&p.policy, p.hostname.as_str())) {
        None => json!({ "strategy": "default" }),
        Some((TlsPolicy::AcmeDns { dns_provider }, pattern)) => json!({
            "strategy": "acme_dns",
            "dns_provider": dns_provider,
            "pattern": pattern,
            "is_wildcard_match": pattern != hostname,
        }),
        // r[impl ingress.site.tailscale]
        Some((TlsPolicy::Tailscale, pattern)) => json!({
            "strategy": "tailscale",
            "pattern": pattern,
            "is_wildcard_match": pattern != hostname,
        }),
        // r[impl tls.strategy.canopy-only]
        Some((TlsPolicy::Canopy, pattern)) => json!({
            "strategy": "canopy",
            "pattern": pattern,
            "is_wildcard_match": pattern != hostname,
        }),
    };

    let active_cert_json = match (st.active_cert, caddy_cert) {
        (Some(c), _) => json!({
            "id": c.id,
            "origin": c.origin.as_str(),
            "issuer": c.issuer,
            "not_before": c.not_before,
            "not_after": c.not_after,
            "self_signed": c.self_signed,
            "ari_window_start": c.ari_window_start,
            "ari_window_end": c.ari_window_end,
        }),
        (None, Some(cc)) => json!({
            "id": null,
            "origin": "caddy",
            "caddy_issuer": cc.issuer_kind,
            "issuer": cc.issuer,
            "not_before": cc.not_before,
            "not_after": cc.not_after,
            "self_signed": cc.self_signed,
            "ari_window_start": null,
            "ari_window_end": null,
        }),
        (None, None) => Value::Null,
    };

    let last_issuance_json = build_last_issuance(st, caddy_cert);
    let last_error = st
        .last_attempt
        .filter(|a| a.outcome == AttemptOutcome::Failure)
        .and_then(|a| a.error.clone())
        .or_else(|| match &st.decision {
            state::Decision::Chain(c) => c.last_error.clone(),
            _ => None,
        });
    let chain_json = project_chain(st);
    let status = decision_status(st, caddy_cert);
    let (next_issuance_at, next_issuance_source) = decision_next_issuance(st);
    let block_json = st.retry_block.map_or(
        Value::Null,
        |b| json!({ "set_at": b.set_at, "reason": b.reason }),
    );

    json!({
        "hostname": hostname,
        "apps": apps.iter().cloned().collect::<Vec<_>>(),
        "policy": policy_json,
        "status": status,
        "chain": chain_json,
        "active_cert": active_cert_json,
        "last_issuance": last_issuance_json,
        "last_error": last_error,
        "retry_block": block_json,
        "force_retry_at": st.force_retry_at,
        "next_issuance_at": next_issuance_at,
        "next_issuance_source": next_issuance_source,
    })
}

/// Map the unified [`state::Decision`] (plus the active cert it
/// resolved against) to a status label for the UI. The label is a
/// projection of the decision, not a re-computation — when the
/// decision says "Issue now (renewal)" but the existing cert is still
/// valid, status is `"active"` because the operator's mental model of
/// the cert is "it's serving".
///
/// For `Decision::Default`, the runtime has no opinion; the status
/// reflects whichever cert is actually serving — a SAN-bound manual
/// cert from the runtime store, or whatever proxy-managed cert is on
/// disk.
fn decision_status(
    st: &state::HostnameState<'_>,
    caddy_cert: Option<&state::ProxyCert>,
) -> &'static str {
    use state::Decision::*;
    use state::IssueReason as R;
    let now = jiff::Timestamp::now().as_second();
    let active_not_after = st.active_cert.and_then(|c| c.not_after);
    match &st.decision {
        Default => serving_status(active_not_after, caddy_cert, now).unwrap_or("default"),
        Chain(c) => {
            if matches!(c.act, state::ChainAct::Blocked { .. }) {
                return "blocked";
            }
            if let Some(status) = serving_status(active_not_after, caddy_cert, now) {
                return status;
            }
            // Nothing is serving the hostname. Canopy cannot be asked about it,
            // so nothing is in progress and the operator has something to fix.
            if c.act == state::ChainAct::CannotAskCanopy {
                return "error";
            }
            // Canopy being asked is an acquisition in progress, and a failure it
            // reported is an error.
            match (&c.cycle, &c.last_error) {
                (chain::Cycle::Running(r), Some(_)) if r.responsible == Strategy::Canopy => "error",
                (chain::Cycle::Running(r), None) if r.responsible == Strategy::Canopy => "pending",
                _ => "default",
            }
        }
        Blocked { .. } => "blocked",
        NoContactEmail | Debounced { .. } => "error",
        Scheduled { .. } => "active",
        IssueNow {
            reason: R::First | R::ForceRetry,
        } => "pending",
        IssueNow {
            reason: R::Renewal { .. },
        } => {
            // Cert is past its renewal trigger. If it's also past expiry
            // (which can happen if every retry has failed), surface that
            // instead of pretending it's still active.
            if active_not_after.is_some_and(|na| na < now) {
                "expired"
            } else {
                "active"
            }
        }
    }
}

/// The status a hostname reflects from whichever certificate is actually
/// serving it: a stored one, or whatever the proxy has on disk. `None` when
/// there is neither.
fn serving_status(
    stored_not_after: Option<i64>,
    caddy_cert: Option<&state::ProxyCert>,
    now: i64,
) -> Option<&'static str> {
    match (stored_not_after, caddy_cert.and_then(|c| c.not_after)) {
        (Some(na), _) if na < now => Some("expired"),
        (Some(_), _) => Some("active"),
        (None, Some(na)) if na < now => Some("expired"),
        (None, Some(_)) => Some("active"),
        (None, None) => None,
    }
}

/// The `chain` object of the hostname view: the strategy responsible, when it
/// falls through, and when the next cycle starts. Null for a hostname with no
/// chain to report.
// r[impl tls.cert.hostname-view]
fn project_chain(st: &state::HostnameState<'_>) -> Value {
    use state::Decision::*;
    match &st.decision {
        Chain(c) => match &c.cycle {
            // Nothing running, so the strategy named is the one that will act
            // when the next cycle starts. What produced the certificate serving
            // the hostname meanwhile is its own origin, under `active_cert`.
            chain::Cycle::Idle { next_cycle_at } => json!({
                "responsible": c.strategies.first().map_or("acme_http01", |s| s.as_str()),
                "fall_through_at": null,
                "next_cycle_at": (*next_cycle_at != i64::MAX).then_some(*next_cycle_at),
            }),
            chain::Cycle::Running(r) => json!({
                "responsible": r.responsible.as_str(),
                "fall_through_at": r.fall_through_at,
                "next_cycle_at": null,
            }),
        },
        // A hostname bound to a policy that names one strategy is on no chain:
        // there is no ladder to report, and what it is doing and when is in the
        // policy and the next-issuance fields already.
        Default
        | Blocked { .. }
        | NoContactEmail
        | Debounced { .. }
        | Scheduled { .. }
        | IssueNow { .. } => Value::Null,
    }
}

/// Map the unified decision to `(next_issuance_at, source)`.
fn decision_next_issuance(st: &state::HostnameState<'_>) -> (Option<i64>, Option<&'static str>) {
    use state::Decision::*;
    use state::IssueReason as R;
    match &st.decision {
        Default | Blocked { .. } | NoContactEmail => (None, None),
        Chain(c) => match &c.cycle {
            chain::Cycle::Idle { next_cycle_at } if *next_cycle_at != i64::MAX => {
                let source = if st.active_cert.is_some_and(|c| c.ari_window_start.is_some()) {
                    NextSource::Ari
                } else {
                    NextSource::Fallback
                };
                (Some(*next_cycle_at), Some(source.as_str()))
            }
            chain::Cycle::Idle { .. } => (None, None),
            // Waiting out the after-failure cadence: say when, as the ACME-DNS
            // path does, rather than claiming the next ask is immediate.
            chain::Cycle::Running(r) if r.responsible == Strategy::Canopy => match &c.act {
                state::ChainAct::Debounced { until } => (Some(*until), Some("debounce")),
                // Nothing asks Canopy until an operator acts, so no issuance is
                // coming to be promised.
                state::ChainAct::Blocked { .. } | state::ChainAct::CannotAskCanopy => (None, None),
                _ => (None, Some("immediate")),
            },
            chain::Cycle::Running(_) => (None, None),
        },
        // Debounce after a failure: surface the time the runtime is
        // willing to retry, with a distinct source so the UI can render
        // "retry blocked until …" rather than "queued for next tick".
        Debounced { until } => (Some(*until), Some("debounce")),
        Scheduled { next_at, source } => (Some(*next_at), Some(source.as_str())),
        IssueNow {
            reason: R::First | R::ForceRetry,
        } => (None, Some("immediate")),
        IssueNow {
            reason: R::Renewal {
                scheduled_at,
                source,
            },
        } => (Some(*scheduled_at), Some(source.as_str())),
    }
}

/// Build the `last_issuance` object: prefer the active cert's metadata
/// for manual uploads (which never go through the attempt log), fall
/// back to the latest successful attempt for ACME-DNS, then to the
/// active cert's bare creation timestamp, then — for default-strategy
/// hostnames — to whatever Caddy's on-disk store has.
fn build_last_issuance(
    st: &state::HostnameState<'_>,
    caddy_cert: Option<&state::ProxyCert>,
) -> Value {
    if let Some(cert) = st.active_cert
        && cert.origin == TlsCertOrigin::Manual
    {
        return json!({
            "kind": "manual",
            "at": cert.created_at,
            "cert_id": cert.id,
        });
    }
    if let Some(cert) = st.active_cert
        && cert.origin == TlsCertOrigin::Canopy
    {
        // r[impl tls.canopy.serve]
        return json!({
            "kind": "canopy",
            "at": cert.created_at,
            "cert_id": cert.id,
        });
    }
    if let Some(att) = st.last_success {
        let provider = st.policy.and_then(|p| match &p.policy {
            TlsPolicy::AcmeDns { dns_provider } => Some(dns_provider.clone()),
            // r[impl ingress.site.tailscale]
            TlsPolicy::Tailscale | TlsPolicy::Canopy => None,
        });
        return json!({
            "kind": "acme_dns",
            "at": att.started_at,
            "cert_id": att.cert_id,
            "provider": provider,
        });
    }
    if let Some(cert) = st.active_cert {
        return json!({
            "kind": cert.origin.as_str(),
            "at": cert.created_at,
            "cert_id": cert.id,
        });
    }
    if let Some(cc) = caddy_cert {
        // Use the cert's own not_before as the issuance timestamp: it's
        // what the CA stamped on the cert, so it survives certmagic
        // rewriting the file for unrelated reasons (restart, metadata
        // refresh, …).
        return json!({
            "kind": "caddy",
            "at": cc.not_before,
            "cert_id": null,
            "provider": cc.issuer_kind,
        });
    }
    Value::Null
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
        "cert_profile": s.cert_profile,
        "updated_at": s.updated_at,
    }))
}

#[derive(Deserialize)]
pub(crate) struct SetSettingsParams {
    /// New contact email. Pass through unchanged when omitted; pass an
    /// empty string to clear.
    #[serde(default)]
    pub contact_email: Option<String>,
    /// New ACME profile. Pass `null` to clear (use the CA's default
    /// profile); pass a non-empty string to opt into a profile by name
    /// (e.g. Let's Encrypt's `shortlived`); omit to leave unchanged.
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    pub cert_profile: Option<Option<String>>,
}

// i[tls.settings.set]
pub(crate) fn set_settings(state: &OiState, params: SetSettingsParams) -> HandlerResult {
    let SetSettingsParams {
        contact_email,
        cert_profile,
    } = params;
    state
        .db
        .call(move |db| -> rusqlite::Result<()> {
            if let Some(email) = contact_email.as_deref() {
                store::set_contact_email(db, email)?;
            }
            if let Some(profile) = cert_profile {
                store::set_cert_profile(db, profile.as_deref())?;
            }
            Ok(())
        })
        .map_err(db_error)?;
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db_error(e: rusqlite::Error) -> OiError {
    OiError::new(ErrorCode::NotFound, format!("db error: {e}"))
}
