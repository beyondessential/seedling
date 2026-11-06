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

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use seedling_protocol::names::AppName;

use super::{
    AttemptOutcome, TlsCertAttempt, TlsCertForceRetry, TlsCertOrigin, TlsCertRetryBlock,
    TlsCertState, TlsCertificate, TlsChainRow, TlsPolicy, TlsPolicyRow, TlsSettings,
    chain::{self, Strategy},
    pattern_matches, pattern_specificity, store,
};
use crate::defs::resource::Resource;
use crate::runtime::apps::{AppPhase, AppRegistry};
use crate::runtime::db::Db;

/// Default minimum interval between auto-retries of a failed background
/// issuance for the same hostname. Operator-driven force-retries bypass
/// this; manual issue calls (`tls.cert.issue-acme-dns`) bypass it too.
pub const AUTO_RETRY_DEBOUNCE_SECS: i64 = 60 * 60;

/// How far ahead a stored-but-not-yet-valid certificate may stand in for a
/// servable one.
///
/// A certificate whose `notBefore` has not arrived is not serving, so a
/// hostname whose only cover is staged has no TLS right now. Treating that as
/// "covered" stops the runtime issuing one that would work. The case worth
/// suppressing is the narrow one: a certificate just issued whose `notBefore`
/// the local clock has not reached, where issuing again would only produce a
/// duplicate. Beyond that window — an operator staging a cutover weeks out —
/// the hostname needs a certificate now, and the staged one takes over when
/// its time comes.
///
/// Tied to the retry debounce because that is the soonest another attempt
/// would run anyway: inside it, issuing now buys nothing.
const STAGED_COVER_LEAD_SECS: i64 = AUTO_RETRY_DEBOUNCE_SECS;

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

/// A certificate the proxy holds for a hostname in its own cache. The runtime
/// has no row for it, only what its certificate says.
///
/// Carries what the operator's view shows as well as what the chain decides on,
/// so the cache is read once for both rather than once each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCert {
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    /// The issuer's name for its directory in the proxy's cache: `local` for its
    /// internal CA, otherwise the host of the ACME directory.
    pub issuer_kind: String,
    /// The issuer as the certificate itself gives it.
    pub issuer: Option<String>,
    pub self_signed: bool,
}

/// What the proxy's cache holds for a hostname, when it could be read.
///
/// Three answers, because they lead to different things: a hostname the proxy
/// holds nothing for may be withheld from it, one it holds a certificate for
/// must not be, and one whose cache could not be read must be left as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyHolding {
    Holds(ProxyCert),
    Nothing,
    Unreadable,
}

/// Facts about the world that decide a hostname's chain and are not in the
/// database. Gathered by the caller, so [`compute_state`] stays a pure
/// function of its inputs.
#[derive(Debug, Clone)]
pub struct ChainEnv {
    /// Whether a Canopy offer is live.
    pub offer_live: bool,
    /// The Canopy application types of the apps declaring each hostname.
    /// A hostname absent from the map, or mapped to an empty set, is declared
    /// by no app with a Canopy type.
    pub canopy_types: BTreeMap<String, BTreeSet<String>>,
    /// What the proxy holds, by hostname. A hostname missing from the map was
    /// not looked at, which is not the same as the proxy holding nothing for
    /// it.
    pub proxy: BTreeMap<String, ProxyHolding>,
}

impl ChainEnv {
    /// For a caller that holds none of these facts and acts on no chain
    /// decision: the expiring-certificate sweep and the renewal scheduler,
    /// which read a snapshot only for what is stored.
    ///
    /// Named rather than defaulted, because what it produces is not a neutral
    /// chain but one with no Canopy in it.
    pub fn none() -> Self {
        Self {
            offer_live: false,
            canopy_types: BTreeMap::new(),
            proxy: BTreeMap::new(),
        }
    }
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
    pub chain_rows: Vec<TlsChainRow>,
    pub settings: TlsSettings,
    /// What the database cannot say about a hostname's chain: whether an offer
    /// is live, which apps declare it, what the proxy holds for it. Supplied by
    /// the caller when the snapshot is loaded; see [`Snapshot::load`].
    pub env: ChainEnv,
    pub now: i64,
    /// SAN list per certificate id, parsed once when the snapshot is built.
    ///
    /// Resolution is per hostname and every caller loops over hostnames —
    /// renewal over due certs, the expiry sweep over ingress targets, the
    /// operator rollup over the whole hostname set — so parsing inside the
    /// matcher costs one X.509 parse per hostname per certificate. The
    /// snapshot is immutable for the length of those loops, so the parse
    /// belongs here.
    ///
    /// Failures are memoised as `None` alongside successes, so a certificate
    /// whose PEM will not parse is parsed and reported once per snapshot rather
    /// than once per hostname — the memo has to cover the rows it cannot read,
    /// or it fails precisely where it is needed and floods the log doing it.
    ///
    /// A memo, not a source of truth: a certificate absent from the map
    /// entirely is parsed on the spot rather than treated as covering nothing.
    /// A snapshot assembled without it is slower, never wrong — the same
    /// discipline this subsystem learned about the `hostname` label.
    sans: BTreeMap<i64, Option<Vec<String>>>,
}

impl Snapshot {
    /// Load a fresh snapshot of TLS state from the DB.
    ///
    /// `env` is required rather than filled in later, because its default is
    /// not a neutral value: it says no app has a Canopy type and no offer is
    /// live, which is a different chain for every hostname. A caller that holds
    /// no such facts says so with [`ChainEnv::none`], and the chain decisions of
    /// that snapshot are not to be acted on.
    pub fn load(db: &Db, env: ChainEnv) -> rusqlite::Result<Self> {
        let certificates = store::list_certificates(db)?;
        let sans = index_sans(&certificates);
        Ok(Self {
            policies: store::list_policies(db)?,
            certificates,
            attempts: store::list_attempts(db, None, 1000)?,
            retry_blocks: store::list_retry_blocks(db)?,
            force_retries: store::list_force_retries(db)?,
            chain_rows: store::list_chain_rows(db)?,
            settings: store::get_settings(db)?,
            env,
            now: Timestamp::now().as_second(),
            sans,
        })
    }

    /// The certificate's SAN list, from the memo when it is there and by
    /// parsing when it is not.
    /// Borrowed on a memo hit, so a hit costs nothing: the memo exists to take
    /// H×C work out of the per-tick loops, and cloning the list back per
    /// lookup would put the same shape straight back as allocations.
    fn sans_for(&self, cert: &TlsCertificate) -> Option<Cow<'_, [String]>> {
        if let Some(entry) = self.sans.get(&cert.id) {
            return entry.as_deref().map(Cow::Borrowed);
        }
        let pem = cert.cert_pem.as_deref()?;
        match super::parse::leaf_san_dns_names(pem) {
            Ok(sans) => Some(Cow::Owned(sans)),
            Err(e) => {
                tracing::warn!(
                    cert_id = cert.id,
                    error = %e,
                    "stored certificate could not be parsed; skipping it when resolving a hostname"
                );
                None
            }
        }
    }
}

/// Parse each active certificate's SAN list once, recording failures as `None`
/// so they are not retried per lookup. Reported here, once per snapshot,
/// because an active row the runtime cannot read is unservable and nothing else
/// will say so.
fn index_sans(certificates: &[TlsCertificate]) -> BTreeMap<i64, Option<Vec<String>>> {
    certificates
        .iter()
        .filter(|c| c.state == TlsCertState::Active)
        .map(|c| {
            let sans = match c.cert_pem.as_deref() {
                None => {
                    tracing::warn!(
                        cert_id = c.id,
                        "active certificate has no stored PEM; it can never be served"
                    );
                    None
                }
                Some(pem) => match super::parse::leaf_san_dns_names(pem) {
                    Ok(sans) => Some(sans),
                    Err(e) => {
                        tracing::warn!(
                            cert_id = c.id,
                            error = %e,
                            "stored certificate could not be parsed; it can never be served"
                        );
                        None
                    }
                },
            };
            (c.id, sans)
        })
        .collect()
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
    /// The hostname is on a strategy chain: no policy, or the Canopy-only
    /// policy.
    Chain(Box<ChainDecision>),
}

/// Where a hostname on a strategy chain stands, and what the runtime does about
/// it.
// r[impl tls.chain.cycle]
#[derive(Debug, Clone)]
pub struct ChainDecision {
    pub strategies: Vec<Strategy>,
    pub cycle: chain::Cycle,
    pub act: ChainAct,
    /// Canopy is to be asked whichever strategy is responsible: an order is
    /// open, or a Canopy certificate has reached its renewal point and Canopy
    /// carries the renewal out itself.
    // r[impl tls.canopy.collect]
    pub collect_canopy: bool,
    /// When the proxy may start ordering a certificate of its own.
    pub proxy_from: chain::ProxyFrom,
    pub canopy: CanopyStanding,
    /// What is serving the hostname, if anything usable is.
    pub serving: Option<ServingSource>,
    /// What the proxy's cache holds, when it was looked at.
    pub proxy: Option<ProxyHolding>,
    /// Why the responsible strategy last failed to produce a certificate.
    pub last_error: Option<String>,
}

/// The least of a certificate's lifetime the proxy is left to renew in, so that
/// it is never told to renew over a window of no time at all.
const MIN_RENEWAL_RATIO: f64 = 0.0001;

impl ChainDecision {
    /// How much of the proxy-held certificate's lifetime may be left when the
    /// proxy renews it, for a hostname the proxy must be held back on.
    ///
    /// `None` leaves the proxy to its own schedule: it holds no certificate,
    /// or it may order whenever it likes. The ratio is the fraction left at the
    /// moment the strategies ahead of the proxy run out of theirs, so the proxy
    /// renews over HTTP-01 from that point and not before.
    // r[impl tls.chain.proxy-gate]
    pub fn proxy_renewal_ratio(&self) -> Option<f64> {
        let Some(ProxyHolding::Holds(cert)) = &self.proxy else {
            return None;
        };
        let (not_before, not_after) = (cert.not_before?, cert.not_after?);
        let lifetime = not_after.checked_sub(not_before).filter(|l| *l > 0)? as f64;
        match self.proxy_from {
            chain::ProxyFrom::Always => None,
            chain::ProxyFrom::At(from) => {
                let left = (not_after - from) as f64 / lifetime;
                Some(left.clamp(MIN_RENEWAL_RATIO, 1.0))
            }
            // No chain strategy of the proxy's will ever be responsible, or no
            // clock says when one would be.
            chain::ProxyFrom::Never | chain::ProxyFrom::Unknown => Some(MIN_RENEWAL_RATIO),
        }
    }
}

/// What the runtime does for a hostname on a chain this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainAct {
    /// Nothing: no cycle is running, or the proxy carries out the strategy
    /// that is responsible.
    Wait,
    /// Canopy is responsible but an operator retry block pauses the runtime.
    Blocked { reason: Option<String> },
    /// Canopy is responsible: ask it on the next Canopy report tick.
    AskCanopy,
    /// Canopy is responsible but was refused or its request rejected, and is
    /// not asked again until `until`. An order already open is still asked
    /// about on every tick.
    Debounced { until: i64 },
    /// Canopy is responsible but cannot be asked about the hostname.
    CannotAskCanopy,
}

/// Whether Canopy can be asked about a hostname at all.
// r[impl tls.strategy.canopy]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanopyStanding {
    /// `app_type` is the application type to send: the one the typed apps
    /// declaring the hostname share, and none when they do not agree.
    Applies { app_type: Option<String> },
    /// No public CA can issue for the name.
    NotPublicName,
    /// No app declaring the hostname has a Canopy type.
    NoTypedApp,
}

impl CanopyStanding {
    pub fn applies(&self) -> bool {
        matches!(self, Self::Applies { .. })
    }
}

/// What is serving a hostname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServingSource {
    Store { cert_id: i64, origin: TlsCertOrigin },
    Proxy,
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

    // Coverage-only lookup, the same rule the serving path applies.
    let active_cert = find_active_for_hostname(snap, hostname, snap.now);

    // A certificate about to start serving is not absent, and reporting
    // absence would have the coordinator issue a replacement on every tick
    // until the window opened — a successful issuance is never debounced (see
    // `debounce_until`), so nothing else would stop it. This is not only the
    // uncovered case: a renewal whose `notBefore` the local clock has not
    // reached leaves the expiring incumbent active and still due, which reissues
    // just as hard. Only an imminent start counts, since a certificate staged
    // weeks out leaves the hostname with no TLS in the meantime.
    // r[impl tls.cert.serve]
    let staged_from = staged_start_for_hostname(snap, hostname, snap.now)
        .filter(|from| *from - snap.now <= STAGED_COVER_LEAD_SECS);

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

    let chain_ctx = ChainContext::gather(
        snap,
        hostname,
        active_cert,
        staged_from,
        retry_block,
        force_retry_at.is_some(),
    );

    let decision = decide(
        snap.now,
        policy.map(|p| &p.policy),
        active_cert,
        staged_from,
        last_attempt,
        retry_block,
        force_retry_at.is_some(),
        &snap.settings,
        &chain_ctx,
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

/// Find the active certificate that answers for `hostname`.
///
/// Ranking is [`super::resolve`]'s, shared with the serving lookup in
/// [`super::store::find_active_for_hostname`] so the two cannot disagree about
/// what a hostname is served.
///
/// A certificate staged ahead of its `notBefore` is skipped, because the
/// serving path will not hand it out either and reporting it as the hostname's
/// active certificate would say the hostname is covered while handshakes for
/// it fail. Expiry is deliberately *not* filtered, unlike in the serving
/// lookup: the renewal scheduler has to see an expiring cert in order to renew
/// it. That difference is the only one, and it lives in this pre-filter rather
/// than in the ranking.
// r[impl tls.strategy.manual]
// r[impl tls.cert.validation.san-coverage]
// r[impl tls.cert.serve]
fn find_active_for_hostname<'a>(
    snap: &'a Snapshot,
    hostname: &str,
    now: i64,
) -> Option<&'a TlsCertificate> {
    let mut best: Option<(super::resolve::Rank, &TlsCertificate)> = None;
    for cert in snap
        .certificates
        .iter()
        .filter(|c| c.state == TlsCertState::Active && !c.not_before.is_some_and(|nb| nb > now))
    {
        let Some(sans) = snap.sans_for(cert) else {
            continue;
        };
        if let Some(rank) = super::resolve::rank(
            &sans,
            hostname,
            cert.self_signed,
            cert.not_after,
            cert.created_at,
            cert.id,
            now,
        ) && best.as_ref().is_none_or(|(best_rank, _)| rank > *best_rank)
        {
            best = Some((rank, cert));
        }
    }
    best.map(|(_, cert)| cert)
}

/// The earliest moment a stored certificate covering `hostname` starts
/// serving, if one is staged ahead of its validity window.
///
/// The earliest rather than the best-ranked: the question is when the hostname
/// starts being served at all, and a later-starting certificate that would
/// outrank it once both are valid does not change that answer.
// r[impl tls.cert.serve]
fn staged_start_for_hostname(snap: &Snapshot, hostname: &str, now: i64) -> Option<i64> {
    let mut earliest: Option<i64> = None;
    for cert in snap
        .certificates
        .iter()
        .filter(|c| c.state == TlsCertState::Active && c.not_before.is_some_and(|nb| nb > now))
    {
        let Some(sans) = snap.sans_for(cert) else {
            continue;
        };
        // Coverage only — ranking decides which certificate wins once several
        // are servable, which is not the question here.
        if super::parse::san_covers(&sans, hostname) {
            let from = cert.not_before.unwrap_or(now);
            earliest = Some(earliest.map_or(from, |e: i64| e.min(from)));
        }
    }
    earliest
}

#[expect(
    clippy::too_many_arguments,
    reason = "one decision over the whole per-hostname snapshot; splitting it would \
              reintroduce the dispatch-before-the-decision this subsystem forbids"
)]
fn decide(
    now: i64,
    policy: Option<&TlsPolicy>,
    active_cert: Option<&TlsCertificate>,
    staged_from: Option<i64>,
    last_attempt: Option<&TlsCertAttempt>,
    retry_block: Option<&TlsCertRetryBlock>,
    force_retry: bool,
    settings: &TlsSettings,
    chain_ctx: &ChainContext,
) -> Decision {
    match policy {
        // No operator policy bound: the default chain. A stored certificate
        // that covers the hostname, or one the proxy holds, is left alone
        // until its renewal point (see [`chain::resolve`]).
        None => decide_chain(
            chain_ctx,
            chain::default_chain(chain_ctx.internal_name, chain_ctx.canopy.applies()),
        ),
        // r[impl tls.strategy.canopy-only]
        Some(TlsPolicy::Canopy) => decide_chain(chain_ctx, chain::CANOPY_ONLY.to_vec()),
        Some(TlsPolicy::AcmeDns { .. }) => decide_acme_dns(
            now,
            active_cert,
            staged_from,
            last_attempt,
            retry_block,
            force_retry,
            settings,
        ),
        // r[impl ingress.site.tailscale]
        // Tailscale-issued certs are managed by the local Tailscale
        // facility; the runtime fetches a fresh pair on demand and
        // doesn't run the ACME state machine for them. The Coordinator
        // dispatch path that does the actual fetch is wired separately;
        // this branch keeps the decision pipeline exhaustive.
        Some(TlsPolicy::Tailscale) => Decision::Default,
    }
}

/// Everything [`decide_chain`] needs about one hostname, gathered from the
/// snapshot before the decision so the decision itself is a plain function of
/// it.
struct ChainContext {
    now: i64,
    internal_name: bool,
    canopy: CanopyStanding,
    offer_live: bool,
    serving: Option<(chain::Serving, ServingSource)>,
    row: Option<chain::Row>,
    /// The runtime holds a key for this hostname that Canopy has not yet
    /// certified, so an order of its own is outstanding whichever strategy is
    /// responsible for serving the hostname meanwhile.
    awaiting_canopy: bool,
    last_error: Option<String>,
    proxy: Option<ProxyHolding>,
    staged_from: Option<i64>,
    retry_block: Option<Option<String>>,
    force: bool,
}

impl ChainContext {
    fn gather(
        snap: &Snapshot,
        hostname: &str,
        active_cert: Option<&TlsCertificate>,
        staged_from: Option<i64>,
        retry_block: Option<&TlsCertRetryBlock>,
        force: bool,
    ) -> Self {
        let now = snap.now;
        let internal_name = is_caddy_internal(hostname);
        let chain_row = snap.chain_rows.iter().find(|r| r.hostname == hostname);
        let types = snap.env.canopy_types.get(hostname);
        let canopy = if internal_name {
            CanopyStanding::NotPublicName
        } else {
            match types {
                Some(types) if !types.is_empty() => CanopyStanding::Applies {
                    // Several types on one hostname cannot be told apart by
                    // the request, so it carries none rather than guessing.
                    app_type: (types.len() == 1)
                        .then(|| types.iter().next().cloned())
                        .flatten(),
                },
                _ => CanopyStanding::NoTypedApp,
            }
        };

        // What Caddy serves for the hostname: the endpoint is consulted ahead
        // of its own cache, so a stored certificate wins over one it holds.
        let serving = active_cert
            .filter(|c| c.not_after.is_none_or(|na| na > now))
            .map(|c| {
                (
                    chain::Serving {
                        identity: chain::Identity {
                            cert_id: Some(c.id),
                            not_after: c.not_after,
                        },
                        renewal_point: next_renewal_at(c).0,
                    },
                    ServingSource::Store {
                        cert_id: c.id,
                        origin: c.origin,
                    },
                )
            })
            .or_else(|| {
                match snap.env.proxy.get(hostname) {
                    Some(ProxyHolding::Holds(c)) => Some(c),
                    Some(ProxyHolding::Nothing | ProxyHolding::Unreadable) | None => None,
                }
                .filter(|c| c.not_after.is_none_or(|na| na > now))
                .map(|c| {
                    (
                        chain::Serving {
                            identity: chain::Identity {
                                cert_id: None,
                                not_after: c.not_after,
                            },
                            renewal_point: renewal_point_for(c.not_before, c.not_after),
                        },
                        ServingSource::Proxy,
                    )
                })
            });

        Self {
            now,
            internal_name,
            canopy,
            offer_live: snap.env.offer_live,
            serving,
            row: chain_row.map(|r| r.row.clone()),
            awaiting_canopy: snap.certificates.iter().any(|c| {
                c.origin == TlsCertOrigin::Canopy
                    && c.state == TlsCertState::CsrPending
                    && c.hostname == hostname
            }),
            last_error: chain_row.and_then(|r| r.last_error.clone()),
            proxy: snap.env.proxy.get(hostname).cloned(),
            staged_from,
            retry_block: retry_block.map(|b| b.reason.clone()),
            force,
        }
    }
}

// r[impl tls.chain.cycle]
// r[impl tls.chain.ladder]
fn decide_chain(ctx: &ChainContext, strategies: Vec<Strategy>) -> Decision {
    let cycle = match (&ctx.serving, ctx.staged_from) {
        // A certificate about to start serving is not absent, and asking for
        // another now would only produce a duplicate.
        (None, Some(from)) if !ctx.force => chain::Cycle::Idle {
            next_cycle_at: from,
        },
        _ => chain::resolve(&chain::Inputs {
            chain: &strategies,
            now: ctx.now,
            serving: ctx.serving.map(|(s, _)| s),
            row: ctx.row.as_ref(),
            offer_live: ctx.offer_live,
            force: ctx.force,
        }),
    };

    let act = match &cycle {
        chain::Cycle::Running(r) if r.responsible == Strategy::Canopy => {
            if let Some(reason) = &ctx.retry_block {
                ChainAct::Blocked {
                    reason: reason.clone(),
                }
            } else if !ctx.canopy.applies() {
                ChainAct::CannotAskCanopy
            } else {
                // r[impl tls.chain.no-certificate]
                // A refusal is asked again at the after-failure cadence, not on
                // every tick. An open order is collected on every tick, and an
                // operator retry asks at once.
                match ctx
                    .row
                    .as_ref()
                    .filter(|row| row.identity == r.identity)
                    .and_then(|row| row.retry_not_before)
                {
                    Some(until) if until > ctx.now && !r.open_order && !ctx.force => {
                        ChainAct::Debounced { until }
                    }
                    _ => ChainAct::AskCanopy,
                }
            }
        }
        _ => ChainAct::Wait,
    };

    // r[impl tls.canopy.collect]
    // A refusal recorded for this cycle moved the hostname on, and it stays
    // with the later strategy until the next cycle: collecting a renewal Canopy
    // has refused would ask it again on every tick for nothing.
    let refused = match (&ctx.row, ctx.serving) {
        (Some(row), Some((serving, _))) => {
            row.identity == serving.identity && row.retry_not_before.is_some()
        }
        _ => false,
    };
    // The after-failure cadence holds for collection as much as for a first
    // request: an answer that refused or rejected what was asked is not asked
    // again sooner for being asked by the other path.
    let waiting = ctx
        .row
        .as_ref()
        .and_then(|row| row.retry_not_before)
        .is_some_and(|until| until > ctx.now)
        && !ctx.force;
    let collect_canopy = ctx.canopy.applies()
        && !waiting
        && match &cycle {
            chain::Cycle::Running(r) => {
                r.open_order
                    // An order Canopy has not answered for is outstanding
                    // whoever is serving the hostname meanwhile, so the key the
                    // runtime holds for it is asked about until one arrives.
                    // Without this a replacement for a revoked certificate that
                    // failed on its one attempt would never be asked for again.
                    // r[impl tls.canopy.revoked]
                    || ctx.awaiting_canopy
                    || (!refused
                        && matches!(
                            ctx.serving,
                            Some((
                                _,
                                ServingSource::Store {
                                    origin: TlsCertOrigin::Canopy,
                                    ..
                                }
                            ))
                        ))
            }
            chain::Cycle::Idle { .. } => false,
        };

    let proxy_from = chain::proxy_from(&strategies, ctx.serving.map(|(s, _)| s), &cycle);

    Decision::Chain(Box::new(ChainDecision {
        strategies,
        cycle,
        act,
        collect_canopy,
        proxy_from,
        canopy: ctx.canopy.clone(),
        serving: ctx.serving.map(|(_, source)| source),
        proxy: ctx.proxy.clone(),
        last_error: ctx.last_error.clone(),
    }))
}

fn decide_acme_dns(
    now: i64,
    active_cert: Option<&TlsCertificate>,
    staged_from: Option<i64>,
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
    // Issuing against a certificate that is already stored and about to start
    // serving produces a duplicate, so a staged start stands in for issuance
    // wherever issuance would otherwise happen. It does not stand in for an
    // incumbent's own renewal schedule: where nothing is due, that schedule is
    // still the right answer.
    let staged = |fallback: Decision| match staged_from {
        Some(from) => Decision::Scheduled {
            next_at: from,
            source: NextSource::Fallback,
        },
        None => fallback,
    };

    let next = active_cert.map(next_renewal_at);
    match (active_cert, next) {
        (None, _) => match debounce_until(now, last_attempt) {
            Some(until) => Decision::Debounced { until },
            None => staged(Decision::IssueNow {
                reason: IssueReason::First,
            }),
        },
        (Some(_), Some((next_at, source))) => {
            if now >= next_at {
                match debounce_until(now, last_attempt) {
                    Some(until) => Decision::Debounced { until },
                    None => staged(Decision::IssueNow {
                        reason: IssueReason::Renewal {
                            scheduled_at: next_at,
                            source,
                        },
                    }),
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
    (
        renewal_point_for(cert.not_before, cert.not_after),
        NextSource::Fallback,
    )
}

/// The moment a third of a certificate's lifetime remains, which is when a
/// certificate with no CA-suggested window is due for renewal. A certificate
/// whose validity is unrecorded or empty is never due: the caller filters the
/// case, and a far-future time keeps the decision logic from accidentally
/// firing.
// r[impl tls.chain.cycle]
pub fn renewal_point_for(not_before: Option<i64>, not_after: Option<i64>) -> i64 {
    let (Some(nb), Some(na)) = (not_before, not_after) else {
        return i64::MAX;
    };
    if na <= nb {
        return i64::MAX;
    }
    let lifetime = na - nb;
    nb + lifetime * (RENEW_AT_FRACTION_DEN - RENEW_AT_FRACTION_NUM) / RENEW_AT_FRACTION_DEN
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
            requested_hostname: None,
            state: TlsCertState::Active,
            origin: TlsCertOrigin::AcmeDns,
            cert_pem: Some(super::super::test_support::self_signed_pem(&[hostname])),
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
        let sans = index_sans(&certs);
        Snapshot {
            policies,
            certificates: certs,
            attempts,
            retry_blocks: blocks,
            force_retries,
            chain_rows: vec![],
            settings: TlsSettings {
                contact_email: contact_email.to_owned(),
                cert_profile: None,
                updated_at: now,
            },
            env: ChainEnv::none(),
            now,
            sans,
        }
    }

    /// A snapshot carrying only certificates, for the matcher tests. Goes
    /// through `snap` so the SAN memo is built the same way it is in
    /// production.
    fn certs_snap(certs: Vec<TlsCertificate>, now: i64) -> Snapshot {
        snap(vec![], certs, vec![], vec![], vec![], "ops@x", now)
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

    /// A certificate staged ahead of its validity window is not what the
    /// hostname is served, so the control plane must not report it as the
    /// hostname's active certificate either — otherwise the rollup calls the
    /// hostname covered, the scheduler concludes there is nothing to do, and
    /// handshakes fail with no signal anywhere.
    // r[verify tls.cert.serve]
    #[test]
    fn a_not_yet_valid_cert_is_not_the_active_cert() {
        let now = 1_000;
        let mut staged = fake_cert("example.com", 1, 0, i64::MAX);
        staged.not_before = Some(now + 86_400);

        assert!(
            find_active_for_hostname(&certs_snap(vec![staged.clone()], now), "example.com", now)
                .is_none(),
            "a staged cert must not be reported as serving",
        );

        // The certificate it was staged to replace is still the active one.
        let mut incumbent = fake_cert("example.com", 2, 0, i64::MAX);
        incumbent.created_at = now - 100;
        let certs = certs_snap(vec![staged, incumbent], now);
        assert_eq!(
            find_active_for_hostname(&certs, "example.com", now)
                .expect("the incumbent still covers the hostname")
                .id,
            2
        );
    }

    /// The in-memory matcher shares the store matcher's rules, including the
    /// exact-label fast path. A row written before the label was made to follow
    /// the certificate claims a name its certificate does not carry, and must
    /// not be served for it.
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn find_active_for_hostname_does_not_hand_out_a_non_covering_cert() {
        fn labelled(id: i64, label: &str, sans: &[&str], created_at: i64) -> TlsCertificate {
            let mut cert = fake_cert(label, id, 0, i64::MAX);
            cert.cert_pem = Some(super::super::test_support::self_signed_pem(sans));
            cert.created_at = created_at;
            cert
        }

        // Labelled www.example.com, carries only example.com.
        let mislabelled = vec![labelled(1, "www.example.com", &["example.com"], 200)];
        assert!(
            find_active_for_hostname(&certs_snap(mislabelled, 1_000), "www.example.com", 1_000)
                .is_none(),
            "the label must not stand in for coverage",
        );

        // The certificate that does cover the name wins, despite being older
        // and despite the mislabelled row matching the label exactly.
        let certs = certs_snap(
            vec![
                labelled(1, "www.example.com", &["example.com"], 200),
                labelled(2, "example.com", &["www.example.com"], 100),
            ],
            1_000,
        );
        let found = find_active_for_hostname(&certs, "www.example.com", 1_000)
            .expect("a certificate covers www.example.com");
        assert_eq!(found.id, 2);
    }

    fn chain_of<'a>(st: &'a HostnameState<'_>) -> &'a ChainDecision {
        match &st.decision {
            Decision::Chain(c) => c,
            other => panic!("expected a chain decision, got {other:?}"),
        }
    }

    // r[verify tls.strategy.default]
    #[test]
    fn no_policy_yields_the_default_chain() {
        let s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(
            c.strategies,
            vec![Strategy::Http01],
            "an app with no Canopy type is not given a Canopy window"
        );
        assert_eq!(c.act, ChainAct::Wait, "the proxy carries out HTTP-01");
    }

    // r[verify tls.cert.hostname-view]
    #[test]
    fn manual_cert_auto_binds_without_policy() {
        // No policy bound, but a manual cert exists for this hostname.
        // compute_state surfaces the cert via active_cert and leaves it
        // alone until its renewal point.
        let mut cert = fake_cert("host.example.com", 7, 0, 100);
        cert.origin = TlsCertOrigin::Manual;
        let s = snap(vec![], vec![cert], vec![], vec![], vec![], "ops@x", 50);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert!(matches!(c.cycle, chain::Cycle::Idle { next_cycle_at } if next_cycle_at == 66));
        assert_eq!(st.active_cert.map(|c| c.id), Some(7));
    }

    fn canopy_env(hostname: &str, types: &[&str], offer_live: bool) -> ChainEnv {
        ChainEnv {
            offer_live,
            canopy_types: BTreeMap::from([(
                hostname.to_owned(),
                types.iter().map(|t| (*t).to_owned()).collect(),
            )]),
            proxy: BTreeMap::new(),
        }
    }

    // r[verify tls.strategy.canopy]
    #[test]
    fn a_typed_app_puts_canopy_first_in_the_chain() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.strategies, vec![Strategy::Canopy, Strategy::Http01]);
        assert_eq!(c.act, ChainAct::AskCanopy);
        assert_eq!(
            c.canopy,
            CanopyStanding::Applies {
                app_type: Some("tamanu".to_owned())
            }
        );
    }

    // r[verify tls.canopy.request]
    #[test]
    fn apps_of_different_types_on_one_hostname_send_no_type() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        s.env = canopy_env("host.example.com", &["tamanu", "other"], true);
        let st = compute_state(&s, "host.example.com");
        assert_eq!(
            chain_of(&st).canopy,
            CanopyStanding::Applies { app_type: None }
        );
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn a_new_hostname_without_a_live_offer_goes_straight_to_the_proxy() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        s.env = canopy_env("host.example.com", &["tamanu"], false);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.act, ChainAct::Wait);
        assert!(matches!(&c.cycle, chain::Cycle::Running(r) if r.responsible == Strategy::Http01));
    }

    // r[verify tls.chain.cycle]
    #[test]
    fn a_proxy_held_certificate_is_left_alone_until_its_renewal_point() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 100);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.env.proxy.insert(
            "host.example.com".to_owned(),
            ProxyHolding::Holds(ProxyCert {
                not_before: Some(0),
                not_after: Some(300),
                issuer_kind: "local".to_owned(),
                issuer: None,
                self_signed: false,
            }),
        );
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.serving, Some(ServingSource::Proxy));
        assert!(
            matches!(c.cycle, chain::Cycle::Idle { next_cycle_at } if next_cycle_at == 200),
            "an offer connecting does not start a cycle for a working certificate"
        );
        assert_eq!(c.act, ChainAct::Wait);
    }

    // r[verify tls.chain.ladder]
    #[test]
    fn a_proxy_held_certificate_past_its_renewal_point_starts_with_canopy() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 200);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.env.proxy.insert(
            "host.example.com".to_owned(),
            ProxyHolding::Holds(ProxyCert {
                not_before: Some(0),
                not_after: Some(300),
                issuer_kind: "local".to_owned(),
                issuer: None,
                self_signed: false,
            }),
        );
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.act, ChainAct::AskCanopy);
        match &c.cycle {
            chain::Cycle::Running(r) => assert_eq!(r.fall_through_at, Some(250)),
            other => panic!("expected a running cycle, got {other:?}"),
        }
    }

    // r[verify tls.chain.cycle]
    #[test]
    fn an_unreadable_proxy_cache_is_not_a_certificate_and_not_an_absence() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 100);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.env
            .proxy
            .insert("host.example.com".to_owned(), ProxyHolding::Unreadable);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.serving, None);
        assert_eq!(c.proxy, Some(ProxyHolding::Unreadable));
    }

    // r[verify tls.strategy.canopy-only]
    #[test]
    fn a_canopy_only_policy_never_leaves_canopy() {
        let mut s = snap(
            vec![TlsPolicyRow {
                hostname: "host.example.com".to_owned(),
                policy: TlsPolicy::Canopy,
                updated_at: 0,
            }],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            1000,
        );
        s.env = canopy_env("host.example.com", &["tamanu"], false);
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.strategies, vec![Strategy::Canopy]);
        assert_eq!(c.act, ChainAct::AskCanopy);
    }

    // r[verify tls.canopy-only.untyped]
    #[test]
    fn a_canopy_only_hostname_with_no_typed_app_cannot_be_asked() {
        let s = snap(
            vec![TlsPolicyRow {
                hostname: "host.example.com".to_owned(),
                policy: TlsPolicy::Canopy,
                updated_at: 0,
            }],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            1000,
        );
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.act, ChainAct::CannotAskCanopy);
        assert_eq!(c.canopy, CanopyStanding::NoTypedApp);
    }

    // r[verify tls.cert.retry-block]
    #[test]
    fn a_retry_block_pauses_asking_canopy() {
        let mut s = snap(
            vec![],
            vec![],
            vec![],
            vec![TlsCertRetryBlock {
                hostname: "host.example.com".to_owned(),
                set_at: 0,
                set_by: RetryBlockSource::Operator,
                reason: Some("paused".to_owned()),
            }],
            vec![],
            "ops@x",
            1000,
        );
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        let st = compute_state(&s, "host.example.com");
        assert_eq!(
            chain_of(&st).act,
            ChainAct::Blocked {
                reason: Some("paused".to_owned())
            }
        );
    }

    fn refused_row(open_order_since: Option<i64>) -> TlsChainRow {
        TlsChainRow {
            hostname: "host.example.com".to_owned(),
            row: chain::Row {
                identity: chain::Identity::NONE,
                step: 0,
                since: 0,
                open_order_since,
                retry_not_before: Some(2000),
            },
            last_error: Some("refused".to_owned()),
            updated_at: 1000,
        }
    }

    // r[verify tls.chain.no-certificate]
    #[test]
    fn a_refused_hostname_is_asked_again_at_the_after_failure_cadence() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.chain_rows = vec![refused_row(None)];
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).act, ChainAct::Debounced { until: 2000 });

        s.now = 2000;
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).act, ChainAct::AskCanopy);
    }

    // r[verify tls.canopy.collect]
    #[test]
    fn an_open_order_is_asked_about_whatever_the_cadence() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 1000);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.chain_rows = vec![refused_row(Some(900))];
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).act, ChainAct::AskCanopy);
    }

    // r[verify tls.cert.force-retry]
    #[test]
    fn an_operator_retry_bypasses_the_cadence() {
        let mut s = snap(
            vec![],
            vec![],
            vec![],
            vec![],
            vec![TlsCertForceRetry {
                hostname: "host.example.com".to_owned(),
                requested_at: 990,
            }],
            "ops@x",
            1000,
        );
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.chain_rows = vec![refused_row(None)];
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).act, ChainAct::AskCanopy);
    }

    // r[verify tls.cert.force-retry]
    #[test]
    fn a_queued_retry_starts_a_cycle_for_a_working_certificate() {
        let mut cert = fake_cert("host.example.com", 7, 0, 300);
        cert.origin = TlsCertOrigin::Canopy;
        let mut s = snap(
            vec![],
            vec![cert],
            vec![],
            vec![],
            vec![TlsCertForceRetry {
                hostname: "host.example.com".to_owned(),
                requested_at: 90,
            }],
            "ops@x",
            100,
        );
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).act, ChainAct::AskCanopy);
    }

    // r[verify tls.chain.cycle]
    #[test]
    fn a_stored_certificate_outranks_one_the_proxy_holds() {
        let mut cert = fake_cert("host.example.com", 7, 0, 3000);
        cert.origin = TlsCertOrigin::Canopy;
        let mut s = snap(vec![], vec![cert], vec![], vec![], vec![], "ops@x", 100);
        s.env.proxy.insert(
            "host.example.com".to_owned(),
            ProxyHolding::Holds(ProxyCert {
                not_before: Some(0),
                not_after: Some(300),
                issuer_kind: "local".to_owned(),
                issuer: None,
                self_signed: false,
            }),
        );
        let st = compute_state(&s, "host.example.com");
        assert_eq!(
            chain_of(&st).serving,
            Some(ServingSource::Store {
                cert_id: 7,
                origin: TlsCertOrigin::Canopy
            }),
            "the endpoint is consulted ahead of the proxy's own cache"
        );
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

    /// A freshly issued certificate whose `notBefore` has not arrived on this
    /// host's clock is stored but not serving. Reporting it as absent would
    /// have the coordinator issue another every tick — a successful issuance
    /// is never debounced — burning CA rate limits against a certificate that
    /// already exists.
    // r[verify tls.cert.serve]
    #[test]
    fn a_staged_cert_is_scheduled_not_reissued() {
        let now = 1_000_000;
        let mut staged = fake_cert("host.example.com", 1, 0, now + 90 * 86_400);
        staged.not_before = Some(now + 3_600);
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![staged],
            vec![attempt(
                "host.example.com",
                AttemptOutcome::Success,
                now - 60,
            )],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        assert!(
            st.active_cert.is_none(),
            "a staged cert is not serving, so it is not the active cert",
        );
        match st.decision {
            Decision::Scheduled { next_at, .. } => assert_eq!(next_at, now + 3_600),
            d => panic!("expected the staged start, got: {d:?}"),
        }
    }

    /// A certificate staged weeks out is not cover: the hostname has no TLS in
    /// the meantime, so suppressing issuance for the whole staging period
    /// would be worse than the duplicate the short-circuit guards against.
    // r[verify tls.cert.serve]
    #[test]
    fn a_distantly_staged_cert_does_not_suppress_issuance() {
        let now = 1_000_000;
        let mut staged = fake_cert("host.example.com", 1, 0, now + 365 * 86_400);
        staged.not_before = Some(now + 30 * 86_400);
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![staged],
            vec![],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        match compute_state(&s, "host.example.com").decision {
            Decision::IssueNow { .. } => {}
            d => panic!("expected issuance for an uncovered hostname, got: {d:?}"),
        }
    }

    /// The renewal path reaches the same re-issuance loop: a renewal whose
    /// `notBefore` the local clock has not reached leaves the expiring
    /// incumbent active and still due, so gating the staged short-circuit on
    /// the hostname being uncovered misses it entirely.
    // r[verify tls.cert.serve]
    #[test]
    fn a_staged_renewal_suppresses_reissuance_while_the_incumbent_is_due() {
        let now = 1_000_000;
        // Incumbent well past its renewal threshold.
        let incumbent = fake_cert("host.example.com", 1, now - 89 * 86_400, now + 86_400);
        let mut staged = fake_cert("host.example.com", 2, 0, now + 90 * 86_400);
        staged.not_before = Some(now + 600);
        staged.created_at = now;
        let s = snap(
            vec![policy_acme("*", "p")],
            vec![incumbent, staged],
            vec![attempt(
                "host.example.com",
                AttemptOutcome::Success,
                now - 60,
            )],
            vec![],
            vec![],
            "ops@x",
            now,
        );
        let st = compute_state(&s, "host.example.com");
        assert!(
            st.active_cert.is_some(),
            "the incumbent is still what serves the hostname",
        );
        match st.decision {
            Decision::Scheduled { next_at, .. } => assert_eq!(next_at, now + 600),
            d => panic!("expected the staged start, got: {d:?}"),
        }
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
            let c = chain_of(&st);
            assert_eq!(
                c.strategies,
                vec![Strategy::InternalCa],
                "{host} is the proxy's internal CA alone"
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
        let mut cert = fake_cert("host.example.com", 1, now - 86400, now + 89 * 86400);
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

    fn held_by_proxy(not_before: i64, not_after: i64) -> ProxyHolding {
        ProxyHolding::Holds(ProxyCert {
            not_before: Some(not_before),
            not_after: Some(not_after),
            issuer_kind: "local".to_owned(),
            issuer: None,
            self_signed: false,
        })
    }

    // r[verify tls.chain.proxy-gate]
    #[test]
    fn the_proxy_is_held_to_the_point_canopy_falls_through() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 100);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.env
            .proxy
            .insert("host.example.com".to_owned(), held_by_proxy(0, 600));
        let st = compute_state(&s, "host.example.com");
        let ratio = chain_of(&st).proxy_renewal_ratio().expect("held back");
        // Renewal at 400, Canopy falls through at 500, a sixth of 600 left.
        assert!((ratio - 100.0 / 600.0).abs() < 1e-9);
    }

    // r[verify tls.chain.proxy-gate]
    #[test]
    fn a_proxy_that_may_order_keeps_its_own_schedule() {
        let mut s = snap(vec![], vec![], vec![], vec![], vec![], "ops@x", 100);
        s.env = canopy_env("host.example.com", &[], true);
        s.env
            .proxy
            .insert("host.example.com".to_owned(), held_by_proxy(0, 600));
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).proxy_renewal_ratio(), None);
    }

    // r[verify tls.strategy.canopy-only]
    #[test]
    fn a_canopy_only_hostname_holds_the_proxy_to_the_last_moment() {
        let mut s = snap(
            vec![TlsPolicyRow {
                hostname: "host.example.com".to_owned(),
                policy: TlsPolicy::Canopy,
                updated_at: 0,
            }],
            vec![],
            vec![],
            vec![],
            vec![],
            "ops@x",
            100,
        );
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.env
            .proxy
            .insert("host.example.com".to_owned(), held_by_proxy(0, 600));
        let st = compute_state(&s, "host.example.com");
        assert_eq!(chain_of(&st).proxy_renewal_ratio(), Some(MIN_RENEWAL_RATIO));
    }

    // r[verify tls.canopy.collect]
    // r[verify tls.chain.refusal]
    #[test]
    fn a_renewal_canopy_refused_is_not_collected_on_every_tick() {
        let mut cert = fake_cert("host.example.com", 7, 0, 300);
        cert.origin = TlsCertOrigin::Canopy;
        let mut s = snap(vec![], vec![cert], vec![], vec![], vec![], "ops@x", 250);
        s.env = canopy_env("host.example.com", &["tamanu"], true);

        // At the renewal point and inside Canopy's window, the held chain is
        // collected on every tick.
        let st = compute_state(&s, "host.example.com");
        assert!(chain_of(&st).collect_canopy);

        // Once Canopy refuses, the hostname has passed on and stays there.
        s.chain_rows = vec![TlsChainRow {
            hostname: "host.example.com".to_owned(),
            row: chain::Row {
                identity: chain::Identity {
                    cert_id: Some(7),
                    not_after: Some(300),
                },
                step: 1,
                since: 240,
                open_order_since: None,
                retry_not_before: Some(3840),
            },
            last_error: Some("refused".to_owned()),
            updated_at: 240,
        }];
        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert!(!c.collect_canopy);
        assert_eq!(c.act, ChainAct::Wait, "the proxy is responsible now");
    }

    // r[verify tls.canopy.revoked]
    #[test]
    fn a_replacement_canopy_has_not_answered_for_is_asked_about_again() {
        // What a revocation leaves behind: the revoked certificate recorded as
        // the one being replaced, the hostname passed to the proxy, and a key
        // waiting for its first certificate.
        let mut pending = fake_cert("host.example.com", 9, 0, 300);
        pending.origin = TlsCertOrigin::Canopy;
        pending.state = TlsCertState::CsrPending;
        let mut s = snap(vec![], vec![pending], vec![], vec![], vec![], "ops@x", 250);
        s.env = canopy_env("host.example.com", &["tamanu"], true);
        s.chain_rows = vec![TlsChainRow {
            hostname: "host.example.com".to_owned(),
            row: chain::Row {
                identity: chain::Identity {
                    cert_id: Some(7),
                    not_after: Some(300),
                },
                step: 1,
                since: 240,
                open_order_since: None,
                retry_not_before: None,
            },
            last_error: Some("revoked".to_owned()),
            updated_at: 240,
        }];

        let st = compute_state(&s, "host.example.com");
        let c = chain_of(&st);
        assert_eq!(c.act, ChainAct::Wait, "the proxy serves the hostname now");
        assert!(
            c.collect_canopy,
            "but the replacement Canopy has not answered for is still asked about"
        );

        // And not sooner than the cadence when the last answer refused it.
        s.chain_rows[0].row.retry_not_before = Some(3840);
        assert!(!chain_of(&compute_state(&s, "host.example.com")).collect_canopy);
    }
}
