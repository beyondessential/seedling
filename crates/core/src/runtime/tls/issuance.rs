//! Daemon-driven ACME-DNS issuance scheduler.
//!
//! Seedling (not Caddy) is in control of when certs get issued. This module
//! owns the queue of hostnames that need certs and serialises ACME flows
//! through a single mutex so we never run parallel orders against the CA.
//!
//! Two entry points:
//!
//! - [`Coordinator::ensure`] — reconciler-driven, fire-and-forget. The
//!   reconciler calls it every tick for each TLS-terminating hostname; the
//!   coordinator dedups in-flight requests, resolves the policy, skips when
//!   a current cert exists, and otherwise runs issuance in the background.
//! - [`Coordinator::issue_now`] — operator-driven, awaitable. Used by the
//!   `tls.cert.issue-acme-dns` OI handler so the operator gets an immediate
//!   pass/fail back. Bypasses the "recent failure" guard so a manual retry
//!   always runs.
//!
//! Failures land in the attempt log under [`tls.cert.attempt-log`] and as
//! a `cert_issue_failed` system fault. A failed background attempt does
//! NOT auto-pause subsequent issuance — that's the operator's call via
//! the retry-blocks endpoint.

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;
use seedling_protocol::names::AppName;
use snafu::Snafu;

use super::{
    AttemptOutcome, AttemptTrigger, TlsPolicy,
    acme::{self, IssueParams, Issued},
    state::{self, Decision},
    store,
};
use crate::runtime::{db::DbHandle, faults, secrets::Cipher};

#[derive(Debug, Snafu)]
pub enum CoordinatorError {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("no contact email is configured; set one via /tls/settings/set"))]
    NoContactEmail,

    #[snafu(display("hostname {hostname} has no policy; bind one via /tls/policies/set-acme-dns"))]
    NoPolicy { hostname: String },

    #[snafu(display("issuance is paused for {hostname}: {reason}"))]
    Paused { hostname: String, reason: String },

    #[snafu(display("acme protocol error: {source}"))]
    Acme { source: acme::AcmeError },

    #[snafu(display(
        "background issuance for {hostname} debounced after recent failure ({remaining_s}s left)"
    ))]
    Debounced { hostname: String, remaining_s: i64 },

    #[snafu(display("encryption error: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },
}

pub type Result<T, E = CoordinatorError> = std::result::Result<T, E>;

/// Runtime-managed issuance coordinator.
pub struct Coordinator {
    db: DbHandle,
    cipher: Arc<Cipher>,
    /// Hostnames for which an issuance task is currently running. Prevents
    /// the reconciler from queuing duplicates and the operator endpoint from
    /// double-firing.
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// Process-wide issuance lock. A single ACME flow at a time keeps us
    /// well inside CA rate limits and avoids racing on the ACME account
    /// state during account bootstrap.
    issue_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Coordinator {
    pub fn new(db: DbHandle, cipher: Arc<Cipher>) -> Arc<Self> {
        Arc::new(Self {
            db,
            cipher,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            issue_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Reconciler-driven: ensure the runtime is making progress on a cert
    /// for `hostname`. Returns immediately. Runs in the background:
    ///
    /// - skips when a current cert already exists,
    /// - skips when an issuance is already in flight for the hostname,
    /// - skips when the hostname is operator-paused,
    /// - skips when the most recent attempt was a failure within the
    ///   debounce window (unless a [force-retry] signal has been written
    ///   for the hostname, in which case the row is consumed and the
    ///   debounce is bypassed).
    ///
    /// [force-retry]: super::store::set_force_retry
    pub fn ensure(self: &Arc<Self>, hostname: &str) {
        let host = hostname.to_owned();
        {
            let mut set = self.in_flight.lock();
            if !set.insert(host.clone()) {
                return;
            }
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let result = me.run(&host, AttemptTrigger::OnDemand).await;
            me.in_flight.lock().remove(&host);
            if let Err(e) = result {
                // The reconciler hands every TLS-terminating ingress
                // hostname to ensure() each tick, including the ones
                // the runtime is intentionally not driving. NoPolicy
                // (default-strategy hostnames — caddy-internal names,
                // hostnames not covered by an acme_dns wildcard),
                // Paused (operator pause), and Debounced (recent
                // failure) are by-design no-ops; logging them at warn
                // amounts to per-tick noise. Demote to debug.
                let expected_no_op = matches!(
                    e,
                    CoordinatorError::NoPolicy { .. }
                        | CoordinatorError::Paused { .. }
                        | CoordinatorError::Debounced { .. }
                );
                if expected_no_op {
                    tracing::debug!(hostname = %host, "issuance no-op: {e}");
                } else {
                    tracing::warn!(
                        hostname = %host,
                        error = %e,
                        "background issuance attempt did not complete"
                    );
                }
            }
        });
    }

    /// Synchronous variant for CLI/OI callers that want the result back in
    /// the same RPC. Goes through the same gating and code path as
    /// [`Self::ensure`]; the only difference is the trigger label
    /// (`manual` vs `on_demand` in the attempt log) and that errors flow
    /// to the caller instead of being swallowed.
    pub async fn issue_now(self: &Arc<Self>, hostname: &str) -> Result<Issued> {
        {
            let mut set = self.in_flight.lock();
            set.insert(hostname.to_owned());
        }
        let outcome = self.run(hostname, AttemptTrigger::Manual).await;
        self.in_flight.lock().remove(hostname);
        outcome
    }

    /// Core issuance flow. Loads the unified hostname state, branches on
    /// the resulting [`Decision`], and runs the ACME flow when the
    /// decision says to.
    async fn run(self: &Arc<Self>, hostname: &str, trigger: AttemptTrigger) -> Result<Issued> {
        let _guard = self.issue_lock.lock().await;

        // Single source of truth for the decision; the OI rollup and the
        // renewal scheduler call the same function over the same DB
        // snapshot. Mutating side-effects (taking force-retry, opening
        // the attempt row) happen below this point and only when the
        // decision says to issue.
        let host_for_state = hostname.to_owned();
        let owned = self
            .db
            .call(move |db| -> rusqlite::Result<OwnedState> {
                let snap = state::Snapshot::load(db)?;
                Ok(OwnedState::from_snapshot(
                    &snap,
                    &state::compute_state(&snap, &host_for_state),
                ))
            })
            .map_err(|source| CoordinatorError::Storage { source })?;

        // For `NoContactEmail` we want a fault visible to the operator,
        // which the pure decision function can't emit. File it here
        // before mapping to the error.
        if matches!(owned.decision, Decision::NoContactEmail) {
            file_cert_fault(
                &self.db,
                hostname,
                "cert_issue_failed",
                "no contact email is configured; set one via /tls/settings/set",
            );
        }

        let dns_provider_name = match handle_non_issuing(hostname, trigger, &owned)? {
            ProceedKind::Skip(issued) => return Ok(issued),
            ProceedKind::Issue(provider) => provider,
        };

        // Take the force-retry row atomically so a single retry request is
        // consumed exactly once even if multiple ticks race.
        let host_for_force = hostname.to_owned();
        let _ = self
            .db
            .call(move |db| store::take_force_retry(db, &host_for_force))
            .map_err(|source| CoordinatorError::Storage { source })?;

        // Open the attempt row before running so a panic mid-flight still
        // leaves a trace.
        let host_for_open = hostname.to_owned();
        let attempt_id = self
            .db
            .call(move |db| store::insert_attempt(db, &host_for_open, trigger))
            .map_err(|source| CoordinatorError::Storage { source })?;

        tracing::info!(
            %hostname,
            provider = %dns_provider_name,
            ?trigger,
            "ACME-DNS issuance starting"
        );
        let result = acme::issue(
            &self.db,
            &self.cipher,
            IssueParams {
                hostname,
                contact_email: &owned.contact_email,
                directory_url: &acme::default_directory_url(),
                dns_provider_name: &dns_provider_name,
                previous_cert_pem: owned.previous_cert_pem.as_deref(),
                cert_profile: owned.cert_profile.as_deref(),
            },
        )
        .await;

        match result {
            Ok(issued) => {
                let cert_id = issued.cert_id;
                let _ = self.db.call(move |db| {
                    store::finalize_attempt(
                        db,
                        attempt_id,
                        AttemptOutcome::Success,
                        Some(cert_id),
                        None,
                    )
                });
                clear_cert_fault(&self.db, hostname, "cert_issue_failed");
                tracing::info!(
                    %hostname,
                    cert_id = issued.cert_id,
                    not_after = issued.not_after,
                    "ACME-DNS issuance succeeded"
                );
                Ok(issued)
            }
            Err(e) => {
                let msg = format!("ACME-DNS issuance for {hostname} failed: {e}");
                let err_for_attempt = msg.clone();
                let _ = self.db.call(move |db| {
                    store::finalize_attempt(
                        db,
                        attempt_id,
                        AttemptOutcome::Failure,
                        None,
                        Some(&err_for_attempt),
                    )
                });
                file_cert_fault(&self.db, hostname, "cert_issue_failed", &msg);
                tracing::warn!(%hostname, error = %e, "ACME-DNS issuance failed");
                Err(CoordinatorError::Acme { source: e })
            }
        }
    }
}

/// Fields lifted out of a [`state::HostnameState`] so the coordinator can
/// own them past the borrow on the snapshot. `compute_state` returns a
/// borrowed view; everything that survives the DB-call closure goes here.
struct OwnedState {
    decision: Decision,
    dns_provider_name: Option<String>,
    contact_email: String,
    cert_profile: Option<String>,
    active_cert_id: Option<i64>,
    active_cert_not_after: Option<i64>,
    previous_cert_pem: Option<String>,
    retry_block_reason: Option<String>,
}

impl OwnedState {
    fn from_snapshot(snap: &state::Snapshot, s: &state::HostnameState<'_>) -> Self {
        let dns_provider_name = s.policy.and_then(|p| match &p.policy {
            TlsPolicy::AcmeDns { dns_provider } => Some(dns_provider.clone()),
            // Tailscale issuance doesn't use a DNS provider; the cert
            // comes from the local tailscaled API.
            TlsPolicy::Tailscale => None,
        });
        Self {
            decision: s.decision.clone(),
            dns_provider_name,
            contact_email: snap.settings.contact_email.clone(),
            cert_profile: snap.settings.cert_profile.clone(),
            active_cert_id: s.active_cert.map(|c| c.id),
            active_cert_not_after: s.active_cert.and_then(|c| c.not_after),
            previous_cert_pem: s.active_cert.and_then(|c| c.cert_pem.clone()),
            retry_block_reason: s.retry_block.and_then(|b| b.reason.clone()),
        }
    }
}

enum ProceedKind {
    /// The decision is to skip the ACME flow. Return this `Issued` (when
    /// an active cert exists) or an error to the caller.
    Skip(Issued),
    /// The decision is to issue; continue with the named DNS provider.
    Issue(String),
}

/// Map a [`Decision`] to either an early return (skip / error) or a
/// "proceed with this provider" continuation. Pulls the manual-trigger
/// override here so the early-return logic stays declarative.
fn handle_non_issuing(
    hostname: &str,
    trigger: AttemptTrigger,
    owned: &OwnedState,
) -> Result<ProceedKind> {
    match &owned.decision {
        Decision::Default => Err(CoordinatorError::NoPolicy {
            hostname: hostname.to_owned(),
        }),
        Decision::Blocked { reason } => Err(CoordinatorError::Paused {
            hostname: hostname.to_owned(),
            reason: reason
                .clone()
                .or_else(|| owned.retry_block_reason.clone())
                .unwrap_or_else(|| "no reason given".to_owned()),
        }),
        Decision::NoContactEmail => Err(CoordinatorError::NoContactEmail),
        Decision::Debounced { until } => {
            // Manual trigger bypasses debounce; otherwise refuse with the
            // remaining time so the caller can surface it.
            if trigger == AttemptTrigger::Manual {
                proceed(owned)
            } else {
                let now = jiff::Timestamp::now().as_second();
                Err(CoordinatorError::Debounced {
                    hostname: hostname.to_owned(),
                    remaining_s: (until - now).max(0),
                })
            }
        }
        Decision::Scheduled { .. } => {
            // Cert is current; only the manual operator path proceeds.
            if trigger == AttemptTrigger::Manual {
                proceed(owned)
            } else {
                Ok(ProceedKind::Skip(Issued {
                    cert_id: owned.active_cert_id.unwrap_or(0),
                    not_after: owned.active_cert_not_after.unwrap_or(0),
                }))
            }
        }
        Decision::IssueNow { .. } => proceed(owned),
    }
}

fn proceed(owned: &OwnedState) -> Result<ProceedKind> {
    Ok(ProceedKind::Issue(owned.dns_provider_name.clone().expect(
        "non-issuing decision must have an acme-dns policy",
    )))
}

/// File a non-app-scoped fault under the `_system` sentinel app. Idempotent
/// per (kind, hostname): if the same fault is already filed we don't
/// duplicate it.
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
