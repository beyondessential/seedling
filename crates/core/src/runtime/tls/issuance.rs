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
    store,
};
use crate::runtime::{db::DbHandle, faults, secrets::Cipher};

/// Default minimum interval between auto-retries of a failed background
/// issuance for the same hostname. Operator-triggered retries
/// ([`Coordinator::issue_now`]) bypass this completely.
const AUTO_RETRY_DEBOUNCE_SECS: i64 = 60 * 60; // 1 hour

#[derive(Debug, Snafu)]
pub enum CoordinatorError {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("no contact email is configured; set one via /tls/settings/set"))]
    NoContactEmail,

    #[snafu(display(
        "hostname {hostname} resolves to a {strategy} policy; ACME-DNS does not apply"
    ))]
    NotAcmeDns {
        hostname: String,
        strategy: &'static str,
    },

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
                tracing::warn!(
                    hostname = %host,
                    error = %e,
                    "background issuance attempt did not complete"
                );
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

    /// Core issuance flow. Runs all the gating + the actual ACME order
    /// while holding the global issue lock.
    async fn run(self: &Arc<Self>, hostname: &str, trigger: AttemptTrigger) -> Result<Issued> {
        let _guard = self.issue_lock.lock().await;

        // 1. If there's already an active cert for the hostname, nothing
        //    to do (background) or a no-op success-equivalent (manual).
        let host_for_lookup = hostname.to_owned();
        let existing = self
            .db
            .call(move |db| store::find_active_for_hostname(db, &host_for_lookup))
            .map_err(|source| CoordinatorError::Storage { source })?;
        if let Some(existing) = existing {
            // Treat manual-retry against an existing cert as a no-op
            // success: the operator presumably wanted us to "make sure
            // there's a cert" and there is one.
            return Ok(Issued {
                cert_id: existing.id,
                not_after: existing.not_after.unwrap_or(0),
            });
        }

        // 2. Operator pause.
        let host_for_pause = hostname.to_owned();
        let pause = self
            .db
            .call(store::list_retry_blocks)
            .map_err(|source| CoordinatorError::Storage { source })?
            .into_iter()
            .find(|b| b.hostname == host_for_pause);
        if let Some(p) = pause {
            return Err(CoordinatorError::Paused {
                hostname: hostname.to_owned(),
                reason: p.reason.unwrap_or_else(|| "no reason given".to_owned()),
            });
        }

        // 3. Resolve policy.
        let host_for_policy = hostname.to_owned();
        let policy = self
            .db
            .call(move |db| store::resolve_policy(db, &host_for_policy))
            .map_err(|source| CoordinatorError::Storage { source })?;
        let dns_provider_name = match policy.map(|p| p.policy) {
            Some(TlsPolicy::AcmeDns { dns_provider }) => dns_provider,
            Some(TlsPolicy::Manual { .. }) => {
                return Err(CoordinatorError::NotAcmeDns {
                    hostname: hostname.to_owned(),
                    strategy: "manual",
                });
            }
            None => {
                return Err(CoordinatorError::NoPolicy {
                    hostname: hostname.to_owned(),
                });
            }
        };

        // 4. Force-retry signal: an operator (or some other path) has
        //    asked for a fresh attempt regardless of recent failures.
        //    Take the row atomically so a single retry request is
        //    consumed exactly once, even if multiple ticks race.
        let host_for_force = hostname.to_owned();
        let forced = self
            .db
            .call(move |db| store::take_force_retry(db, &host_for_force))
            .map_err(|source| CoordinatorError::Storage { source })?;

        // 5. Recent-failure debounce (skipped when force-retry is set, or
        //    when the caller is the synchronous CLI/OI path).
        let bypass_debounce = forced || trigger == AttemptTrigger::Manual;
        if !bypass_debounce {
            let host_for_attempts = hostname.to_owned();
            let recent = self
                .db
                .call(move |db| store::list_attempts(db, Some(&host_for_attempts), 1))
                .map_err(|source| CoordinatorError::Storage { source })?;
            if let Some(last) = recent.first()
                && last.outcome == AttemptOutcome::Failure
            {
                let now = jiff::Timestamp::now().as_second();
                let since = now - last.finished_at.unwrap_or(last.started_at);
                if since < AUTO_RETRY_DEBOUNCE_SECS {
                    let remaining_s = AUTO_RETRY_DEBOUNCE_SECS - since;
                    tracing::debug!(
                        %hostname,
                        debounce_remaining_s = remaining_s,
                        "background issuance debounced after recent failure"
                    );
                    return Err(CoordinatorError::Debounced {
                        hostname: hostname.to_owned(),
                        remaining_s,
                    });
                }
            }
        }

        // 5. Need the global contact email to register an account.
        let settings = self
            .db
            .call(store::get_settings)
            .map_err(|source| CoordinatorError::Storage { source })?;
        if settings.contact_email.is_empty() {
            // Surface as a fault so the operator sees it without trawling logs.
            file_cert_fault(
                &self.db,
                hostname,
                "cert_issue_failed",
                "no contact email is configured; set one via /tls/settings/set",
            );
            return Err(CoordinatorError::NoContactEmail);
        }

        // 6. Open the attempt row before running so a panic mid-flight
        //    still leaves a trace.
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
                contact_email: &settings.contact_email,
                directory_url: &acme::default_directory_url(),
                dns_provider_name: &dns_provider_name,
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
