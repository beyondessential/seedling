//! Background renewal task for daemon-issued ACME-DNS certificates.
//!
//! On a fixed cadence (default: hourly) the task asks the issuance
//! coordinator to ensure every TLS-managed hostname whose unified state
//! says "issue now". The actual decision logic lives in
//! [`super::state::compute_state`] so the renewal task, the reconciler,
//! and the operator interface can never disagree about which certs are
//! due — they all read from the same function.
//!
//! On success the coordinator atomically supersedes the old cert with
//! the new one; subsequent handshakes pick up the replacement via the
//! `get_certificate` endpoint without reconciler involvement.
//!
//! On failure the old cert remains active. The next tick re-evaluates
//! the unified state, observes the failure-debounce window, and skips
//! until it expires; persistent failures will eventually trip the
//! [`tls.fault.expiring`] fault as the cert nears expiry.

use std::sync::Arc;
use std::time::Duration;

use super::{
    issuance::Coordinator,
    state::{self, Decision, IssueReason},
};
use crate::runtime::{db::DbHandle, secrets::Cipher};

/// Default time between renewal scans.
pub const DEFAULT_TICK: Duration = Duration::from_secs(3600);

/// Run a single renewal pass: enumerate hostnames whose unified state
/// says "issue now" and ask the coordinator to ensure each. Returns the
/// number of hostnames the coordinator was asked to act on; the actual
/// success/failure is observed via the attempt log.
// r[impl tls.acme.renewal.auto]
pub async fn tick(db: &DbHandle, coord: &Arc<Coordinator>) -> RenewalReport {
    let mut report = RenewalReport::default();

    let due = match db.call(due_hostnames) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "renewal: failed to enumerate certs");
            return report;
        }
    };

    for hostname in due {
        coord.ensure(&hostname);
        report.queued += 1;
    }

    report
}

/// Spawn the renewal task on the current Tokio runtime. The task runs
/// forever, ticking every `tick_period`.
pub fn spawn(
    db: DbHandle,
    coord: Arc<Coordinator>,
    tick_period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tick_period);
        // The first tick fires immediately; skip it so we don't try to
        // renew before the daemon's other systems have warmed up.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let report = tick(&db, &coord).await;
            if report.queued > 0 {
                tracing::info!(queued = report.queued, "tls: renewal pass complete");
            }
        }
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RenewalReport {
    /// Number of hostnames queued for issuance this pass. The coordinator
    /// may still skip individual hostnames at run time (e.g. a block was
    /// set between snapshot and execution); the attempt log is the
    /// authoritative outcome record.
    pub queued: i64,
}

/// Walk the snapshot and return hostnames whose [`Decision`] says
/// "issue now" — first issuance, due renewal, or operator force-retry.
fn due_hostnames(db: &crate::runtime::db::Db) -> rusqlite::Result<Vec<String>> {
    let snap = state::Snapshot::load(db)?;
    let mut hostnames: Vec<String> = Vec::new();

    // Existing certs: for each acme-dns active cert, ask the unified
    // state whether it's due. This covers renewals and force-retries on
    // hostnames the runtime already knows about.
    for cert in &snap.certificates {
        if cert.state != super::TlsCertState::Active || cert.origin != super::TlsCertOrigin::AcmeDns
        {
            continue;
        }
        let st = state::compute_state(&snap, &cert.hostname);
        if matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::Renewal { .. } | IssueReason::ForceRetry,
            }
        ) {
            push_unique(&mut hostnames, cert.hostname.clone());
        }
    }

    // Force-retry rows for hostnames without an existing cert (e.g. an
    // operator hits "retry" on a never-issued hostname). The reconciler's
    // per-tick `coord.ensure(hostname)` already covers known TLS-terminating
    // ingresses, but the renewal task should also catch hostnames that
    // have a force-retry queued without one (rare, but correct).
    for fr in &snap.force_retries {
        let st = state::compute_state(&snap, &fr.hostname);
        if matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::ForceRetry,
            }
        ) {
            push_unique(&mut hostnames, fr.hostname.clone());
        }
    }

    Ok(hostnames)
}

fn push_unique(out: &mut Vec<String>, hostname: String) {
    if !out.iter().any(|h| h == &hostname) {
        out.push(hostname);
    }
}
