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
    TlsPolicy, keypair, state, store,
    store::CertMetadata,
    validate::{self, ValidateError},
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

    let primary_san = validated
        .parsed
        .san_dns_names
        .first()
        .cloned()
        .expect("validate_upload rejects empty SAN lists");
    let chain_pem = validated.parsed.chain_pem.clone();
    let metadata = CertMetadata {
        issuer: validated.parsed.metadata.issuer.clone(),
        not_before: validated.parsed.metadata.not_before,
        not_after: validated.parsed.metadata.not_after,
        serial: validated.parsed.metadata.serial.clone(),
        self_signed: validated.parsed.metadata.self_signed,
    };
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
            let id = store::insert_certificate(
                db,
                &label_for_insert,
                TlsCertState::Active,
                TlsCertOrigin::Manual,
                Some(&chain_pem),
                None,
                &key_ciphertext,
                key_type,
                metadata,
                note_for_insert.as_deref(),
                None,
            )?;
            // Replace any prior active cert with the same primary SAN
            // (renewal-of-same-cert flow) so serving picks the new one
            // up immediately. Other certs with overlapping SAN coverage
            // stay around; resolution picks the most-recent active row
            // covering each hostname.
            store::supersede_other_active_for_hostname(db, &label_for_insert, id)?;
            Ok(id)
        })
        .map_err(db_error)?;

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
                &host_for_insert,
                TlsCertState::CsrPending,
                TlsCertOrigin::Csr,
                None,
                Some(&csr_pem_for_insert),
                &key_ciphertext,
                key_type,
                CertMetadata::default(),
                None,
                None,
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
/// cert's SubjectPublicKeyInfo matches the stored key, runs the
/// standard SAN-coverage / expiry checks, and on success transitions
/// the row to `active`. Any prior active certificate for the same
/// hostname is superseded.
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

    // Validation reuses the upload rules: SAN-list non-empty, expiry,
    // and (here against the CSR's stored key) SPKI match. SAN coverage
    // for the originally-requested hostname is enforced as part of the
    // SPKI match — the CSR was built with that name as its only SAN.
    let validated =
        validate::validate_upload(&cert_pem, &stored_key).map_err(map_validate_error)?;

    let chain_pem = validated.parsed.chain_pem.clone();
    let metadata = CertMetadata {
        issuer: validated.parsed.metadata.issuer.clone(),
        not_before: validated.parsed.metadata.not_before,
        not_after: validated.parsed.metadata.not_after,
        serial: validated.parsed.metadata.serial.clone(),
        self_signed: validated.parsed.metadata.self_signed,
    };

    let host_for_update = cert_row.hostname.clone();
    state
        .db
        .call(move |db| -> rusqlite::Result<()> {
            store::update_certificate(
                db,
                id,
                TlsCertState::Active,
                Some(&chain_pem),
                Some(&metadata),
            )?;
            store::supersede_other_active_for_hostname(db, &host_for_update, id)?;
            Ok(())
        })
        .map_err(db_error)?;

    Ok(json!({
        "id": id,
        "warnings": validated.warnings,
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
    let cert = state
        .db
        .call(move |db| store::get_certificate(db, id))
        .map_err(db_error)?
        .ok_or_else(|| OiError::not_found(format!("certificate not found: {id}")))?;
    if cert.state != TlsCertState::CsrPending {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "certificate {id} is in state {}; csr_cancel only applies to csr_pending rows",
                cert.state.as_str()
            ),
        ));
    }
    state
        .db
        .call(move |db| store::delete_certificate(db, id))
        .map_err(db_error)?;
    Ok(json!({ "ok": true }))
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
    {
        let reg = state.registry.read();
        for managed in state::managed_ingresses(&reg) {
            if let Some(filter) = params.app.as_deref()
                && managed.app.as_str() != filter
            {
                continue;
            }
            hostname_apps
                .entry(managed.hostname)
                .or_default()
                .insert(managed.app.as_str().to_owned());
        }
    }

    // Load the unified snapshot once. The OI then asks the same
    // `compute_state` function the issuance coordinator uses, projecting
    // the result into the JSON shape the operator UI expects. Because
    // both call sites read from the same function, what the operator
    // sees here is exactly what the runtime will do.
    let snapshot = state.db.call(state::Snapshot::load).map_err(db_error)?;

    // Resolve Caddy's data volume once per request (cached on OiState
    // after first hit). For hostnames whose strategy is "default"
    // (no operator policy bound) we read Caddy's certmagic store off
    // disk to surface issuer / expiry / last-issued, since the runtime
    // has no DB rows for those certs.
    let caddy_data_path = resolve_caddy_data_path(state);

    let result: Vec<Value> = hostname_apps
        .into_iter()
        .map(|(hostname, apps)| {
            let st = state::compute_state(&snapshot, &hostname);
            let caddy_cert = if matches!(st.decision, state::Decision::Default) {
                caddy_data_path
                    .as_deref()
                    .and_then(|p| crate::system::caddy::read_caddy_cert(p, &hostname))
            } else {
                None
            };
            project_hostname_state(&hostname, &apps, &st, caddy_cert.as_ref())
        })
        .collect();

    Ok(json!({ "hostnames": result }))
}

/// Resolve (and cache) the host filesystem path of the Caddy data
/// volume. Returns `None` if the volume can't be located right now —
/// e.g. Caddy hasn't started yet, or the runtime is using a container
/// driver whose mount-point query failed. The cell is one-shot success;
/// transient failures retry on the next call.
fn resolve_caddy_data_path(state: &OiState) -> Option<std::path::PathBuf> {
    if let Some(p) = state.caddy_data_path.get() {
        return Some(p.clone());
    }
    let runtime = Arc::clone(&state.container_runtime);
    let resolved: Result<std::path::PathBuf, _> = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            runtime
                .volume_mountpoint(crate::system::caddy::CADDY_DATA_VOLUME)
                .await
        })
    });
    match resolved {
        Ok(p) => {
            // Best-effort fill; if another caller raced ahead and won
            // we use whatever they stored.
            let _ = state.caddy_data_path.set(p.clone());
            Some(p)
        }
        Err(e) => {
            tracing::debug!(
                error = %e,
                "tls/hostnames: caddy data volume unresolved; default-strategy hostnames will not show cert metadata"
            );
            None
        }
    }
}

/// Render a [`state::HostnameState`] as the JSON the OI rollup serves.
/// `caddy_cert` is supplied for default-strategy hostnames: it carries
/// what Caddy's own automatic TLS has on disk, so the rollup can show
/// issuer / expiry / "last issued" for certs the runtime doesn't own.
fn project_hostname_state(
    hostname: &str,
    apps: &BTreeSet<String>,
    st: &state::HostnameState<'_>,
    caddy_cert: Option<&crate::system::caddy::CaddyCertView>,
) -> Value {
    let policy_json = match st.policy.map(|p| (&p.policy, p.hostname.as_str())) {
        None => json!({ "strategy": "default" }),
        Some((TlsPolicy::AcmeDns { dns_provider }, pattern)) => json!({
            "strategy": "acme_dns",
            "dns_provider": dns_provider,
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
        .and_then(|a| a.error.clone());
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
    caddy_cert: Option<&crate::system::caddy::CaddyCertView>,
) -> &'static str {
    use state::Decision::*;
    use state::IssueReason as R;
    let now = jiff::Timestamp::now().as_second();
    let active_not_after = st.active_cert.and_then(|c| c.not_after);
    match &st.decision {
        Default => match (active_not_after, caddy_cert.and_then(|c| c.not_after)) {
            (Some(na), _) if na < now => "expired",
            (Some(_), _) => "active",
            (None, Some(na)) if na < now => "expired",
            (None, Some(_)) => "active",
            (None, None) => "default",
        },
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

/// Map the unified decision to `(next_issuance_at, source)`.
fn decision_next_issuance(st: &state::HostnameState<'_>) -> (Option<i64>, Option<&'static str>) {
    use state::Decision::*;
    use state::IssueReason as R;
    match &st.decision {
        Default | Blocked { .. } | NoContactEmail => (None, None),
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
    caddy_cert: Option<&crate::system::caddy::CaddyCertView>,
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
    if let Some(att) = st.last_success {
        let provider = st.policy.map(|p| match &p.policy {
            TlsPolicy::AcmeDns { dns_provider } => dns_provider.clone(),
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
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub cert_profile: Option<Option<String>>,
}

// `Option<Option<T>>` distinguishes "field absent" from "field present
// but null". serde-json's default Option deserialiser collapses both
// into None, which would make it impossible to clear the profile via
// the same endpoint that updates it. This wrapper preserves the
// three-state semantics.
fn deserialize_optional_field<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Ok(Some(Option::<String>::deserialize(deserializer)?))
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
