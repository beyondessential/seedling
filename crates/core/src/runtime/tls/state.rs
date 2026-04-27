//! Single source of truth for "what should the runtime do with this
//! TLS-terminating hostname right now?"
//!
//! Three callers consume this module:
//!
//! - The issuance coordinator ([`super::issuance::Coordinator`]) — to
//!   decide whether to run an ACME flow this tick.
//! - The renewal scheduler ([`super::renewal`]) — to enumerate hostnames
//!   whose certs are due for renewal.
//! - The operator interface — to surface per-hostname state on the
//!   certificates page.
//!
//! Sharing the computation means the spec promise that "what the operator
//! sees is what the machinery will do" is structural, not aspirational:
//! any divergence is impossible because there is only one decision
//! function. The functions here are pure over a [`Snapshot`] of DB
//! state; the snapshot is loaded once and reused per call.

use jiff::Timestamp;
use seedling_protocol::names::AppName;

use super::{
    AttemptOutcome, TlsCertAttempt, TlsCertForceRetry, TlsCertOrigin, TlsCertRetryBlock,
    TlsCertState, TlsCertificate, TlsPolicy, TlsPolicyRow, TlsSettings, pattern_matches,
    pattern_specificity, store,
};
use crate::defs::resource::Resource;
use crate::runtime::apps::{AppPhase, AppRegistry};
use crate::runtime::db::Db;

/// Default minimum interval between auto-retries of a failed background
/// issuance for the same hostname. Operator-driven force-retries bypass
/// this; manual issue calls (`tls.cert.issue-acme-dns`) bypass it too.
pub const AUTO_RETRY_DEBOUNCE_SECS: i64 = 60 * 60;

/// Fixed-fraction renewal threshold used when the CA hasn't supplied
/// ARI advice. Renew when remaining lifetime drops below this fraction
/// of the total lifetime — copes equally with 90-day and 6-day
/// certificate profiles.
// r[impl tls.cert.ari]
pub const RENEW_AT_FRACTION_NUM: i64 = 1;
pub const RENEW_AT_FRACTION_DEN: i64 = 3;

/// One TLS-terminating ingress the runtime is currently managing: the
/// app it belongs to, the ingress resource's name, and the hostname
/// that ingress claims. Produced by [`managed_ingresses`] from the
/// app registry; the issuance coordinator (via the reconciler), the
/// expiring-cert sweep, and the OI rollup all walk the same list so
/// they cannot disagree about which hostnames the runtime is acting
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedIngress {
    pub app: AppName,
    pub ingress_name: String,
    pub hostname: String,
}

/// Walk the app registry and return every TLS-terminating ingress the
/// runtime is currently managing. Skips apps in `NotInstalled` phase
/// (mirroring the reconciler's [`snapshot_all_apps`] filter): those
/// apps have no live ingresses, no certs being driven for them, and
/// surfacing them anywhere — coordinator, renewal, OI rollup, expiry
/// sweep — would misrepresent the runtime's actual scope.
///
/// All three call sites consume the *same* function so the set of
/// in-scope hostnames is structurally identical, not coincidentally
/// matched.
// r[impl tls.cert.hostname-view]
pub fn managed_ingresses(registry: &AppRegistry) -> Vec<ManagedIngress> {
    let mut out = Vec::new();
    for entry in registry.iter() {
        if *entry.phase.lock() == AppPhase::NotInstalled {
            continue;
        }
        let def = entry.app.def.load();
        for resource in def.resources.values() {
            if let Resource::Ingress(ing) = resource {
                let ing_def = ing.def.lock();
                if ing_def.tls {
                    out.push(ManagedIngress {
                        app: entry.name.clone(),
                        ingress_name: ing.name.as_str().to_owned(),
                        hostname: ing_def.hostname.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Snapshot of every DB row that drives a hostname's decision. Loaded
/// once per OI rollup or coordinator run; reused across hostnames.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub policies: Vec<TlsPolicyRow>,
    pub certificates: Vec<TlsCertificate>,
    /// All recent attempts, newest first. The decision logic only needs
    /// the latest entry per hostname; bulk-loading keeps the OI rollup a
    /// single sweep.
    pub attempts: Vec<TlsCertAttempt>,
    pub retry_blocks: Vec<TlsCertRetryBlock>,
    pub force_retries: Vec<TlsCertForceRetry>,
    pub settings: TlsSettings,
    pub now: i64,
}

impl Snapshot {
    /// Load a fresh snapshot of TLS state from the DB.
    pub fn load(db: &Db) -> rusqlite::Result<Self> {
        Ok(Self {
            policies: store::list_policies(db)?,
            certificates: store::list_certificates(db)?,
            attempts: store::list_attempts(db, None, 1000)?,
            retry_blocks: store::list_retry_blocks(db)?,
            force_retries: store::list_force_retries(db)?,
            settings: store::get_settings(db)?,
            now: Timestamp::now().as_second(),
        })
    }
}

/// Resolved per-hostname state. Borrows from the [`Snapshot`] it was
/// computed against, so callers that need to retain it past the snapshot's
/// lifetime should lift the fields they care about.
#[derive(Debug, Clone)]
pub struct HostnameState<'a> {
    pub hostname: String,
    /// Resolved policy (most-specific match) or `None` when no policy
    /// pattern covers the hostname.
    pub policy: Option<&'a TlsPolicyRow>,
    /// For ACME-DNS: the most-recent active cert with this hostname. For
    /// manual: the cert referenced by `policy.cert_id`. `None` when none
    /// is bound or stored.
    pub active_cert: Option<&'a TlsCertificate>,
    pub last_attempt: Option<&'a TlsCertAttempt>,
    pub last_success: Option<&'a TlsCertAttempt>,
    pub retry_block: Option<&'a TlsCertRetryBlock>,
    pub force_retry_at: Option<i64>,
    pub decision: Decision,
}

/// What the runtime should do (or refrain from doing) for this hostname.
///
/// One variant per branch of the decision tree. Callers map this onto
/// their own surface — the coordinator runs ACME for [`Decision::IssueNow`],
/// the OI maps it to a status chip, the renewal task picks hostnames where
/// `decision` is `IssueNow` with `IssueReason::Renewal`.
#[derive(Debug, Clone)]
pub enum Decision {
    /// No operator-driven policy bound — the runtime takes no action.
    /// A manual cert that auto-binds via SAN, or a proxy-managed cert,
    /// may still appear in `active_cert`; the runtime won't drive
    /// renewal of those unless a separate `acme_dns` policy applies.
    Default,
    /// Operator pause set. The runtime won't act until the block is
    /// cleared (`/tls/retry-blocks/clear`) or a force-retry is queued.
    Blocked { reason: Option<String> },
    /// ACME-DNS policy is bound but no global contact email is set, so
    /// account registration would fail. The runtime can't act.
    NoContactEmail,
    /// ACME-DNS, last attempt failed within [`AUTO_RETRY_DEBOUNCE_SECS`].
    /// The coordinator skips background runs until `until`; an operator
    /// force-retry or a `Manual` trigger bypasses this.
    Debounced { until: i64 },
    /// ACME-DNS, current cert is fine; renewal is scheduled for `next_at`.
    /// The coordinator does nothing until `next_at`.
    Scheduled { next_at: i64, source: NextSource },
    /// ACME-DNS, the runtime should issue (or renew) now. The
    /// coordinator runs an ACME flow in this state.
    IssueNow { reason: IssueReason },
}

#[derive(Debug, Clone)]
pub enum IssueReason {
    /// No active cert exists yet for this hostname.
    First,
    /// An operator-driven retry signal is queued. Bypasses debounce.
    ForceRetry,
    /// Existing cert is past its renewal trigger (ARI window or
    /// fallback); time to renew.
    Renewal {
        scheduled_at: i64,
        source: NextSource,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextSource {
    /// CA's RFC 9773 ARI suggested-window start.
    // r[impl tls.cert.ari]
    Ari,
    /// Runtime's lifetime-fraction fallback when ARI isn't available.
    // r[impl tls.cert.ari]
    Fallback,
}

impl NextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ari => "ari",
            Self::Fallback => "fallback",
        }
    }
}

/// Compute the unified state for `hostname` from a loaded snapshot.
///
/// Pure function: no DB access, no clocks (the snapshot carries `now`),
/// no side effects. Same inputs always yield the same `Decision`.
// r[impl tls.cert.hostname-view]
pub fn compute_state<'a>(snap: &'a Snapshot, hostname: &str) -> HostnameState<'a> {
    let policy = resolve_policy(&snap.policies, hostname);

    // SAN-aware lookup: matches ACME-DNS certs by hostname column
    // (fast path) and manual certs by SAN coverage (auto-bind).
    let active_cert = find_active_for_hostname(&snap.certificates, hostname);

    let mut last_attempt: Option<&TlsCertAttempt> = None;
    let mut last_success: Option<&TlsCertAttempt> = None;
    for att in &snap.attempts {
        if att.hostname == hostname {
            if last_attempt.is_none() {
                last_attempt = Some(att);
            }
            if last_success.is_none() && att.outcome == AttemptOutcome::Success {
                last_success = Some(att);
            }
            if last_attempt.is_some() && last_success.is_some() {
                break;
            }
        }
    }

    let retry_block = snap.retry_blocks.iter().find(|b| b.hostname == hostname);
    let force_retry_at = snap
        .force_retries
        .iter()
        .find(|f| f.hostname == hostname)
        .map(|f| f.requested_at);

    let decision = decide(
        snap.now,
        policy.map(|p| &p.policy),
        active_cert,
        last_attempt,
        retry_block,
        force_retry_at.is_some(),
        &snap.settings,
    );

    HostnameState {
        hostname: hostname.to_owned(),
        policy,
        active_cert,
        last_attempt,
        last_success,
        retry_block,
        force_retry_at,
        decision,
    }
}

fn resolve_policy<'a>(policies: &'a [TlsPolicyRow], hostname: &str) -> Option<&'a TlsPolicyRow> {
    let internal = is_caddy_internal(hostname);
    let mut best: Option<(u32, &TlsPolicyRow)> = None;
    for row in policies {
        if pattern_matches(&row.hostname, hostname) {
            // Wildcard policies do not auto-bind hostnames Caddy already
            // manages with its internal CA — `.localhost`, `.local`,
            // `.internal`, IP literals, single-label names. No public CA
            // will issue for them and DNS-01 has nowhere to put the
            // challenge record, so a wildcard catch would just churn
            // failed orders. An explicit (non-wildcard) policy still
            // wins, so an operator can deliberately override.
            if internal && row.hostname.contains('*') {
                continue;
            }
            let score = pattern_specificity(&row.hostname);
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, row));
            }
        }
    }
    best.map(|(_, row)| row)
}

/// Whether `hostname` is a name Caddy's automatic TLS handles with its
/// internal CA rather than ACME. Mirrors Caddy's own
/// `matchInternalHosts` heuristic for the cases we encounter:
///
/// - IP literals (v4 or v6).
/// - Single-label hostnames (no `.`), e.g. `internal-api`.
/// - `.localhost`, `.local`, `.internal` TLDs (and the bare TLDs
///   themselves).
///
/// Used by [`resolve_policy`] to skip wildcard auto-bindings on these
/// names; an explicit policy still applies.
pub fn is_caddy_internal(hostname: &str) -> bool {
    if hostname.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let lc = hostname.to_ascii_lowercase();
    if !lc.contains('.') {
        return true;
    }
    matches!(
        lc.rsplit('.').next(),
        Some("localhost" | "local" | "internal")
    )
}

/// Find the most-recent active certificate that covers `hostname`.
///
/// Match rules mirror [`super::store::find_active_for_hostname`]: a
/// cert whose primary `hostname` column equals the target wins (the
/// fast path for ACME-DNS rows). Otherwise, the first active cert
/// whose SAN list covers `hostname` per RFC 6125 wins, picked in
/// newest-first order.
fn find_active_for_hostname<'a>(
    certs: &'a [TlsCertificate],
    hostname: &str,
) -> Option<&'a TlsCertificate> {
    // Fast path: exact-hostname-column match.
    let exact = certs
        .iter()
        .filter(|c| c.state == TlsCertState::Active && c.hostname == hostname)
        .max_by_key(|c| c.created_at);
    if exact.is_some() {
        return exact;
    }

    // SAN-coverage scan, newest-first.
    let mut active: Vec<&TlsCertificate> = certs
        .iter()
        .filter(|c| c.state == TlsCertState::Active)
        .collect();
    active.sort_by_key(|c| std::cmp::Reverse(c.created_at));
    for cert in active {
        let Some(pem) = cert.cert_pem.as_deref() else {
            continue;
        };
        let Ok(parsed) = super::parse::parse_chain(pem) else {
            continue;
        };
        if super::parse::san_covers(&parsed.san_dns_names, hostname) {
            return Some(cert);
        }
    }
    None
}

fn decide(
    now: i64,
    policy: Option<&TlsPolicy>,
    active_cert: Option<&TlsCertificate>,
    last_attempt: Option<&TlsCertAttempt>,
    retry_block: Option<&TlsCertRetryBlock>,
    force_retry: bool,
    settings: &TlsSettings,
) -> Decision {
    match policy {
        // No operator policy bound: the runtime takes no action. If a
        // manual cert (or proxy-managed cert) happens to cover this
        // hostname, the rollup surfaces it via `active_cert`; the
        // runtime doesn't drive issuance because nothing has asked it
        // to. Manual-near-expiry-via-acme-dns is handled separately by
        // [`decide_acme_dns`] when an acme_dns policy *also* applies.
        None => Decision::Default,
        Some(TlsPolicy::AcmeDns { .. }) => decide_acme_dns(
            now,
            active_cert,
            last_attempt,
            retry_block,
            force_retry,
            settings,
        ),
    }
}

fn decide_acme_dns(
    now: i64,
    active_cert: Option<&TlsCertificate>,
    last_attempt: Option<&TlsCertAttempt>,
    retry_block: Option<&TlsCertRetryBlock>,
    force_retry: bool,
    settings: &TlsSettings,
) -> Decision {
    if let Some(b) = retry_block {
        return Decision::Blocked {
            reason: b.reason.clone(),
        };
    }
    if settings.contact_email.is_empty() {
        return Decision::NoContactEmail;
    }
    if force_retry {
        return Decision::IssueNow {
            reason: IssueReason::ForceRetry,
        };
    }

    let next = active_cert.map(next_renewal_at);
    match (active_cert, next) {
        (None, _) => match debounce_until(now, last_attempt) {
            Some(until) => Decision::Debounced { until },
            None => Decision::IssueNow {
                reason: IssueReason::First,
            },
        },
        (Some(_), Some((next_at, source))) => {
            if now >= next_at {
                match debounce_until(now, last_attempt) {
                    Some(until) => Decision::Debounced { until },
                    None => Decision::IssueNow {
                        reason: IssueReason::Renewal {
                            scheduled_at: next_at,
                            source,
                        },
                    },
                }
            } else {
                Decision::Scheduled { next_at, source }
            }
        }
        // An active cert with no parseable validity window is unusual but
        // possible (manual upload without dates, or a corrupted row). Skip
        // automatic action and surface as Scheduled-far-future.
        (Some(_), None) => Decision::Scheduled {
            next_at: i64::MAX,
            source: NextSource::Fallback,
        },
    }
}

/// Compute the renewal trigger time for `cert`. Prefer the CA's ARI
/// suggested-window start; fall back to the 1/3-of-lifetime mark.
// r[impl tls.cert.ari]
pub fn next_renewal_at(cert: &TlsCertificate) -> (i64, NextSource) {
    if let Some(start) = cert.ari_window_start {
        return (start, NextSource::Ari);
    }
    let (Some(nb), Some(na)) = (cert.not_before, cert.not_after) else {
        // Caller filters this case; returning a far-future time keeps
        // the decision logic from accidentally firing.
        return (i64::MAX, NextSource::Fallback);
    };
    if na <= nb {
        return (i64::MAX, NextSource::Fallback);
    }
    let lifetime = na - nb;
    let due =
        nb + lifetime * (RENEW_AT_FRACTION_DEN - RENEW_AT_FRACTION_NUM) / RENEW_AT_FRACTION_DEN;
    (due, NextSource::Fallback)
}

/// If the most recent attempt was a failure within the debounce window,
/// returns the timestamp at which the debounce expires. Otherwise `None`.
fn debounce_until(now: i64, last_attempt: Option<&TlsCertAttempt>) -> Option<i64> {
    let last = last_attempt?;
    if last.outcome != AttemptOutcome::Failure {
        return None;
    }
    let finished = last.finished_at.unwrap_or(last.started_at);
    let until = finished + AUTO_RETRY_DEBOUNCE_SECS;
    if until > now { Some(until) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tls::{
        AttemptOutcome, AttemptTrigger, KeyType, RetryBlockSource, TlsCertOrigin, TlsCertState,
        TlsPolicy,
    };

    fn fake_cert(hostname: &str, id: i64, not_before: i64, not_after: i64) -> TlsCertificate {
        TlsCertificate {
            id,
            hostname: hostname.to_owned(),
            state: TlsCertState::Active,
            origin: TlsCertOrigin::AcmeDns,
            cert_pem: Some("PEM".to_owned()),
            csr_pem: None,
            key_ciphertext: vec![0u8; 32],
            key_type: KeyType::EcdsaP256,
            issuer: Some("Let's Encrypt".to_owned()),
            not_before: Some(not_before),
            not_after: Some(not_after),
            serial: Some("01".to_owned()),
            self_signed: false,
            note: None,
            acme_account_id: None,
            ari_window_start: None,
            ari_window_end: None,
            ari_polled_at: None,
            created_at: not_before,
            updated_at: not_before,
        }
    }

    fn snap(
        policies: Vec<TlsPolicyRow>,
        certs: Vec<TlsCertificate>,
        attempts: Vec<TlsCertAttempt>,
        blocks: Vec<TlsCertRetryBlock>,
        force_retries: Vec<TlsCertForceRetry>,
        contact_email: &str,
        now: i64,
    ) -> Snapshot {
        Snapshot {
            policies,
            certificates: certs,
            attempts,
            retry_blocks: blocks,
            force_retries,
            settings: TlsSettings {
                contact_email: contact_email.to_owned(),
                cert_profile: None,
                updated_at: now,
            },
            now,
        }
    }

    fn policy_acme(hostname: &str, provider: &str) -> TlsPolicyRow {
        TlsPolicyRow {
            hostname: hostname.to_owned(),
            policy: TlsPolicy::AcmeDns {
                dns_provider: provider.to_owned(),
            },
            updated_at: 0,
        }
    }

    fn attempt(hostname: &str, outcome: AttemptOutcome, finished_at: i64) -> TlsCertAttempt {
        TlsCertAttempt {
            id: 1,
            hostname: hostname.to_owned(),
            triggered_by: AttemptTrigger::OnDemand,
            started_at: finished_at - 1,
            finished_at: Some(finished_at),
            outcome,
            cert_id: None,
            error: if outcome == AttemptOutcome::Failure {
                Some("boom".to_owned())
            } else {
                None
            },
        }
    }

    #[test]
    fn no_policy_yields_default() {
        let s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        let st = compute_state(&s, "host.example.com");
        assert!(matches!(st.decision, Decision::Default));
    }

    #[test]
    fn manual_cert_auto_binds_without_policy() {
        // No policy bound, but a manual cert exists for this hostname.
        // compute_state should surface the cert via active_cert and
        // keep the decision at Default (runtime takes no action; the
        // cert just covers what its SANs say).
        let mut cert = fake_cert("host.example.com", 7, 0, 100);
        cert.origin = TlsCertOrigin::Manual;
        let s = snap(vec![], vec![cert], vec![], vec![], vec![], "ops@x", 50);
        let st = compute_state(&s, "host.example.com");
        assert!(matches!(st.decision, Decision::Default));
        assert_eq!(st.active_cert.map(|c| c.id), Some(7));
    }

    #[test]
    fn acme_dns_first_issuance_when_no_cert() {
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            1000,
        );
        let st = compute_state(&s, "host.example.com");
        assert!(matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::First
            }
        ));
    }

    #[test]
    fn no_contact_email_blocks_acme_dns() {
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![],
            vec![],
            vec![],
            "",
            1000,
        );
        let st = compute_state(&s, "host.example.com");
        assert!(matches!(st.decision, Decision::NoContactEmail));
    }

    #[test]
    fn retry_block_supersedes_other_signals() {
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![],
            vec![TlsCertRetryBlock {
                hostname: "host.example.com".to_owned(),
                set_at: 100,
                set_by: RetryBlockSource::Operator,
                reason: Some("paused".to_owned()),
            }],
            vec![TlsCertForceRetry {
                hostname: "host.example.com".to_owned(),
                requested_at: 200,
            }],
            "ops@x",
            1000,
        );
        let st = compute_state(&s, "host.example.com");
        match st.decision {
            Decision::Blocked { reason } => assert_eq!(reason.as_deref(), Some("paused")),
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn force_retry_overrides_debounce() {
        let now = 1_000_000;
        let last_failed_at = now - 60;
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![attempt(
                "host.example.com",
                AttemptOutcome::Failure,
                last_failed_at,
            )],
            vec![],
            vec![TlsCertForceRetry {
                hostname: "host.example.com".to_owned(),
                requested_at: now - 1,
            }],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        assert!(matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::ForceRetry
            }
        ));
    }

    #[test]
    fn recent_failure_debounces_first_issuance() {
        let now = 1_000_000;
        let last_failed_at = now - 60; // well within 1h debounce
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![attempt(
                "host.example.com",
                AttemptOutcome::Failure,
                last_failed_at,
            )],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        match st.decision {
            Decision::Debounced { until } => {
                assert_eq!(until, last_failed_at + AUTO_RETRY_DEBOUNCE_SECS);
            }
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn cert_within_first_two_thirds_of_lifetime_is_scheduled() {
        let now = 1_000_000;
        // 90-day cert, 80 days remain → well below 1/3 mark, just renewed.
        let cert = fake_cert("host.example.com", 1, now - 10 * 86400, now + 80 * 86400);
        let s = snap(
            vec![policy_acme("host.example.com", "p")],
            vec![cert],
            vec![],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        match st.decision {
            Decision::Scheduled { next_at, source } => {
                assert_eq!(source, NextSource::Fallback);
                assert!(next_at > now);
            }
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn cert_past_one_third_remaining_is_due_for_renewal() {
        let now = 1_000_000;
        // 90-day cert, 25 days remain → past the 1/3 mark, renew.
        let cert = fake_cert("host.example.com", 1, now - 65 * 86400, now + 25 * 86400);
        let s = snap(
            vec![policy_acme("host.example.com", "p")],
            vec![cert],
            vec![],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        match st.decision {
            Decision::IssueNow {
                reason: IssueReason::Renewal { source, .. },
            } => {
                assert_eq!(source, NextSource::Fallback);
            }
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn wildcard_policy_skips_caddy_internal_hostnames() {
        // A `*` wildcard catches every name in seedling's policy
        // resolver. For `.localhost` etc. that catch must be ignored so
        // Caddy's internal CA still serves the cert.
        let cases = [
            "servedir.localhost",
            "foo.local",
            "bar.internal",
            "single-label",
            "192.168.1.1",
            "::1",
        ];
        for host in cases {
            let s = snap(
                vec![policy_acme("*", "p")],
                vec![],
                vec![],
                vec![],
                vec![],
                "ops@x",
                1000,
            );
            let st = compute_state(&s, host);
            assert!(
                matches!(st.decision, Decision::Default),
                "{host} should fall through to default (got {:?})",
                st.decision
            );
            assert!(st.policy.is_none(), "{host} policy should be unresolved");
        }
    }

    #[test]
    fn explicit_policy_overrides_caddy_internal_skip() {
        // Pinning an exact hostname forces ACME-DNS even for a name
        // Caddy would otherwise handle internally.
        let s = snap(
            vec![policy_acme("servedir.localhost", "p")],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            1000,
        );
        let st = compute_state(&s, "servedir.localhost");
        assert!(matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::First
            }
        ));
    }

    #[test]
    fn public_hostname_still_matched_by_wildcard() {
        // Sanity-check the inverse: a public hostname still resolves
        // through the wildcard. The skip is internal-only.
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            1000,
        );
        let st = compute_state(&s, "app.example.com");
        assert!(matches!(
            st.decision,
            Decision::IssueNow {
                reason: IssueReason::First
            }
        ));
    }

    #[test]
    fn ari_window_takes_precedence_over_fallback() {
        let now = 1_000_000;
        // ARI says renew at now+1 day even though the cert is fresh.
        let mut cert = fake_cert("host.example.com", 1, now - 1 * 86400, now + 89 * 86400);
        cert.ari_window_start = Some(now + 86400);
        let s = snap(
            vec![policy_acme("host.example.com", "p")],
            vec![cert],
            vec![],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        match st.decision {
            Decision::Scheduled { next_at, source } => {
                assert_eq!(source, NextSource::Ari);
                assert_eq!(next_at, now + 86400);
            }
            d => panic!("unexpected: {d:?}"),
        }
    }
}
