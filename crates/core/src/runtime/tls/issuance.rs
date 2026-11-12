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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;
use seedling_protocol::names::AppName;
use snafu::Snafu;

use super::{
    AttemptOutcome, AttemptTrigger, TlsPolicy,
    acme::{self, IssueParams, Issued},
    env::Environment,
    state::{self, ChainAct, ChainDecision, Decision},
    store, tailscale_issuer,
};
use crate::runtime::{
    db::DbHandle,
    faults,
    secrets::Cipher,
    site_ingresses::{self, DiscoveryProvider, SiteIngressSource, TlsProvider as SiteTlsProvider},
};

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

    #[snafu(display("tailscale issuer error: {source}"))]
    Tailscale {
        source: tailscale_issuer::IssueError,
    },

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

/// How many issuances are running for each hostname.
///
/// A count rather than a set, and released only through the [`InFlightClaim`]
/// that took it: with a set, whichever of two overlapping issuances finished
/// first cleared the marker the other still relied on, and the reconciler went
/// on to start a duplicate flow against the CA.
#[derive(Default)]
struct InFlight(Mutex<HashMap<String, usize>>);

impl InFlight {
    /// Claim `hostname` unless an issuance for it is already running.
    fn claim_if_idle(self: &Arc<Self>, hostname: &str) -> Option<InFlightClaim> {
        let mut running = self.0.lock();
        if running.contains_key(hostname) {
            return None;
        }
        running.insert(hostname.to_owned(), 1);
        Some(InFlightClaim {
            in_flight: Arc::clone(self),
            hostname: hostname.to_owned(),
        })
    }

    /// Claim `hostname` whether or not an issuance for it is already running.
    fn claim(self: &Arc<Self>, hostname: &str) -> InFlightClaim {
        *self.0.lock().entry(hostname.to_owned()).or_default() += 1;
        InFlightClaim {
            in_flight: Arc::clone(self),
            hostname: hostname.to_owned(),
        }
    }

    #[cfg(test)]
    fn is_running(&self, hostname: &str) -> bool {
        self.0.lock().contains_key(hostname)
    }
}

/// One running issuance's share of [`InFlight`], given back when dropped, so
/// an issuance that errors, panics or is cancelled still releases it.
struct InFlightClaim {
    in_flight: Arc<InFlight>,
    hostname: String,
}

impl Drop for InFlightClaim {
    fn drop(&mut self) {
        let mut running = self.in_flight.0.lock();
        if let Some(count) = running.get_mut(&self.hostname) {
            *count -= 1;
            if *count == 0 {
                running.remove(&self.hostname);
            }
        }
    }
}

/// Runtime-managed issuance coordinator.
pub struct Coordinator {
    db: DbHandle,
    cipher: Arc<Cipher>,
    /// Hostnames for which an issuance task is currently running. Prevents
    /// the reconciler from queuing duplicates.
    in_flight: Arc<InFlight>,
    /// Process-wide issuance lock. A single ACME flow at a time keeps us
    /// well inside CA rate limits and avoids racing on the ACME account
    /// state during account bootstrap.
    issue_lock: Arc<tokio::sync::Mutex<()>>,
    /// Where the facts a hostname's strategy chain depends on come from. Set
    /// once the daemon's state exists, after the coordinator has to; until then
    /// no chain decision can be made.
    environment: std::sync::OnceLock<Arc<dyn Environment>>,
    /// The state chain decisions are made against, and when it was built. See
    /// [`Coordinator::chain_state`].
    chain_state: Mutex<Option<(std::time::Instant, Arc<ChainState>)>>,
}

/// How long one reading of the world serves every chain decision made against
/// it.
///
/// Long enough that an idle reconciler tick, every few seconds, costs nothing;
/// short enough that a certificate reaching its renewal point is acted on well
/// inside the ladder, whose steps are days. What an operator does is not left to
/// it: the writes that change a decision call
/// [`Coordinator::forget_chain_state`].
const CHAIN_STATE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Everything a chain decision is made from, for every hostname at once.
struct ChainState {
    managed: Vec<state::ManagedIngress>,
    snapshot: state::Snapshot,
}

impl Coordinator {
    pub fn new(db: DbHandle, cipher: Arc<Cipher>) -> Arc<Self> {
        Arc::new(Self {
            db,
            cipher,
            in_flight: Arc::default(),
            issue_lock: Arc::new(tokio::sync::Mutex::new(())),
            environment: std::sync::OnceLock::new(),
            chain_state: Mutex::new(None),
        })
    }

    /// Say where the facts a chain depends on come from. Takes effect once.
    pub fn set_environment(&self, environment: Arc<dyn Environment>) {
        let _ = self.environment.set(environment);
    }

    /// Start the cycle an operator retry asked for, where nothing else would.
    ///
    /// A retry for a hostname Canopy can be asked about is left queued for the
    /// report tick, which consumes it as part of asking. A hostname on a chain
    /// Canopy is not part of has nobody waiting for it — the proxy carries out
    /// every strategy there is — so the cycle is started here and the retry
    /// consumed, in the write that records it.
    ///
    /// Does nothing for a hostname on no chain: an ACME-DNS or Tailscale
    /// hostname's retry is the issuance coordinator's to take.
    // spec: TLS#retry-blocks-and-retries
    // spec: CHAIN#return-to-the-first-strategy
    pub fn settle_retry(&self, hostname: &str) {
        let Some(state) = self.chain_state() else {
            // Not knowing where the hostname stands is not knowing whether
            // anything will take the retry, so it is left where it is: the
            // report tick takes it if Canopy applies, and an operator can ask
            // again if nothing does.
            return;
        };
        let Some(Decision::Chain(c)) = state
            .managed
            .iter()
            .any(|m| m.hostname == hostname)
            .then(|| state::compute_state(&state.snapshot, hostname).decision)
        else {
            return;
        };
        // Left for the report tick only where that tick will in fact take it:
        // asking Canopy is what consumes a retry, and a hostname the chain is
        // not putting to Canopy right now would keep the retry queued, with the
        // operator's view promising work nothing was going to do.
        let canopy_will_take_it = matches!(
            c.act,
            ChainAct::AskCanopy | ChainAct::Debounced { .. } | ChainAct::Blocked { .. }
        ) || c.collect_canopy;
        if canopy_will_take_it {
            return;
        }
        let (replacing, now) = (c.replacing(), state.snapshot.now);
        let host = hostname.to_owned();
        match self
            .db
            .call(move |db| store::start_cycle_for_force_retry(db, &host, replacing, now))
        {
            Ok(_) => self.forget_chain_state(),
            Err(e) => {
                tracing::warn!(%hostname, error = %e, "cannot start the cycle an operator retry asked for");
            }
        }
    }

    /// What the proxy holds for each hostname, from the reading already taken.
    ///
    /// For a caller that wants the facts rather than the decisions, so that a
    /// request on an operator's screen does not walk the proxy's certificate
    /// cache itself. `None` when they could not be read, which is not an empty
    /// answer.
    pub fn proxy_holdings(&self) -> Option<BTreeMap<String, state::ProxyHolding>> {
        Some(self.chain_state()?.snapshot.env.proxy.clone())
    }

    /// What the proxy holds for one hostname, read now rather than from the
    /// state the decisions are made against.
    ///
    /// `None` when there is nothing to read it with, which is not the proxy
    /// holding nothing.
    // spec: CHAIN#proxy-strategies
    pub fn proxy_holding_now(&self, hostname: &str) -> Option<state::ProxyHolding> {
        Some(self.environment.get()?.holding(hostname))
    }

    /// Forget the state chain decisions are made against, so the next one is
    /// made afresh. For a caller that has just written something a decision
    /// rests on and wants it acted on now rather than within the window.
    pub fn forget_chain_state(&self) {
        *self.chain_state.lock() = None;
    }

    /// How long the proxy is to wait before renewing a certificate it holds, for
    /// each hostname it must be held back on.
    ///
    /// `None` when the decisions could not be made, which is not an empty map:
    /// an empty one tells the proxy it may renew whenever it likes.
    // spec: CHAIN#proxy-strategies
    pub fn proxy_renewal_ratios(&self) -> Option<BTreeMap<String, f64>> {
        Some(
            self.chain_decisions(None)?
                .into_iter()
                .filter_map(|(hostname, c)| Some((hostname, c.proxy_renewal_ratio()?)))
                .collect(),
        )
    }

    /// The chain decision for every TLS-terminating hostname on a chain, or for
    /// the one named.
    ///
    /// `None` when the decisions could not be made, which is not the same as
    /// no hostname being on a chain: a caller that withholds anything on the
    /// strength of a decision must withhold nothing on `None`.
    // spec: CHAIN#cycles
    pub fn chain_decisions(&self, only: Option<&str>) -> Option<BTreeMap<String, ChainDecision>> {
        let state = self.chain_state()?;
        let mut out = BTreeMap::new();
        for ingress in &state.managed {
            if only.is_some_and(|host| host != ingress.hostname)
                || out.contains_key(&ingress.hostname)
            {
                continue;
            }
            if let Decision::Chain(c) =
                state::compute_state(&state.snapshot, &ingress.hostname).decision
            {
                out.insert(ingress.hostname.clone(), *c);
            }
        }
        Some(out)
    }

    /// The state every chain decision is made against, built at most once per
    /// [`CHAIN_STATE_TTL`].
    ///
    /// One state for every hostname rather than one per hostname asked about:
    /// the proxy asks the certificate endpoint at each handshake it has no
    /// certificate for, so building it per hostname would rebuild the whole TLS
    /// state per name under traffic, on the one thread every database call in
    /// the daemon is serialised through.
    ///
    /// The state is shared and so is as old as the moment it was built, which is
    /// of the same order as the endpoint's own memory of what it answered: a
    /// policy set or a certificate stored is acted on within the window, not
    /// within the tick.
    fn chain_state(&self) -> Option<Arc<ChainState>> {
        // Held across the build, so a burst of handshakes that all find it
        // stale waits on the one rebuild instead of each starting its own and
        // contending on the database thread.
        let mut cached = self.chain_state.lock();
        if let Some((at, state)) = cached.as_ref()
            && at.elapsed() < CHAIN_STATE_TTL
        {
            return Some(Arc::clone(state));
        }
        let (managed, env) = self.environment.get()?.gather()?;
        let snapshot = match self.db.call(move |db| state::Snapshot::load(db, env)) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the TLS state to decide a chain");
                return None;
            }
        };
        let state = Arc::new(ChainState { managed, snapshot });
        *cached = Some((std::time::Instant::now(), Arc::clone(&state)));
        Some(state)
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
        // spec: TLS#issuance
        let Some(claim) = self.in_flight.claim_if_idle(hostname) else {
            return;
        };
        let host = hostname.to_owned();
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let result = me.run(&host, AttemptTrigger::OnDemand).await;
            drop(claim);
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

    /// Returns `true` when `hostname` matches a non-stale, discovered
    /// Tailscale site ingress whose `tls_provider` is `Tailscale`. The
    /// Coordinator dispatches these hostnames to the Tailscale issuer
    /// instead of the ACME pipeline.
    // spec: TLS#tailscale-certificates
    fn dispatch_to_tailscale(&self, hostname: &str) -> Result<bool> {
        let host = hostname.to_owned();
        let matched = self
            .db
            .call(move |db| -> rusqlite::Result<bool> {
                for ing in site_ingresses::list(db)? {
                    if ing.hostname != host {
                        continue;
                    }
                    if ing.stale {
                        continue;
                    }
                    let is_tailscale = matches!(
                        &ing.source,
                        SiteIngressSource::Discovered {
                            provider: DiscoveryProvider::Tailscale,
                            ..
                        }
                    ) && matches!(ing.tls_provider, SiteTlsProvider::Tailscale);
                    if is_tailscale {
                        return Ok(true);
                    }
                }
                Ok(false)
            })
            .map_err(|source| CoordinatorError::Storage { source })?;
        Ok(matched)
    }

    /// Tailscale-side counterpart to the ACME flow: open an attempt row,
    /// call `tailscale_issuer::issue`, and finalise + emit faults the
    /// same way the ACME path does so the operator-visible cert state
    /// is consistent across origins.
    // spec: TLS#tailscale-certificates
    async fn run_tailscale(
        self: &Arc<Self>,
        hostname: &str,
        trigger: AttemptTrigger,
    ) -> Result<Issued> {
        // Per-tick `coord.ensure(hostname)` calls would fetch a fresh
        // cert from tailscaled every ten seconds without this guard.
        // Match the renewal threshold the ACME path uses (a third of
        // the cert's lifetime remaining); manual triggers always run
        // regardless. tailscaled handles upstream renewal; we just
        // need to fetch when our cached row is approaching expiry.
        if trigger != AttemptTrigger::Manual {
            let host_for_check = hostname.to_owned();
            let active = self
                .db
                .call(move |db| store::find_active_for_hostname(db, &host_for_check))
                .map_err(|source| CoordinatorError::Storage { source })?;
            if let Some(cert) = active
                && cert.origin == super::TlsCertOrigin::Tailscale
                && let Some(not_after) = cert.not_after
                && let Some(not_before) = cert.not_before
            {
                let now = jiff::Timestamp::now().as_second();
                let lifetime = (not_after - not_before).max(1);
                let remaining = (not_after - now).max(0);
                if remaining * state::RENEW_AT_FRACTION_DEN
                    > lifetime * state::RENEW_AT_FRACTION_NUM
                {
                    return Ok(Issued {
                        cert_id: cert.id,
                        not_after,
                    });
                }
            }
        }

        let host_for_open = hostname.to_owned();
        let attempt_id = self
            .db
            .call(move |db| store::insert_attempt(db, &host_for_open, trigger))
            .map_err(|source| CoordinatorError::Storage { source })?;

        tracing::info!(%hostname, ?trigger, "Tailscale issuance starting");
        let result = tailscale_issuer::issue(&self.db, &self.cipher, hostname, None).await;
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
                    not_after = issued.not_after.unwrap_or(0),
                    "Tailscale issuance succeeded"
                );
                Ok(Issued {
                    cert_id: issued.cert_id,
                    not_after: issued.not_after.unwrap_or(0),
                })
            }
            Err(e) => {
                let msg = format!("Tailscale issuance for {hostname} failed: {e}");
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
                tracing::warn!(%hostname, error = %e, "Tailscale issuance failed");
                Err(CoordinatorError::Tailscale { source: e })
            }
        }
    }

    /// Synchronous variant for CLI/OI callers that want the result back in
    /// the same RPC. Goes through the same gating and code path as
    /// [`Self::ensure`]; the only difference is the trigger label
    /// (`manual` vs `on_demand` in the attempt log) and that errors flow
    /// to the caller instead of being swallowed.
    pub async fn issue_now(self: &Arc<Self>, hostname: &str) -> Result<Issued> {
        // Joins rather than skips a background issuance already running: the
        // operator asked for this one and waits on its result. The two are
        // serialised by the issue lock, and the hostname stays in flight until
        // both have finished.
        // spec: TLS#issuance
        let _claim = self.in_flight.claim(hostname);
        self.run(hostname, AttemptTrigger::Manual).await
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
                // An ACME-DNS or Tailscale flow, which is on no chain.
                let snap = state::Snapshot::load(db, state::ChainEnv::none())?;
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

        // spec: TLS#tailscale-certificates
        // Tailscale-discovered hostnames bypass the ACME state machine
        // entirely: there's no operator policy to read, no CA to talk to, and
        // no ARI window. tailscale_issuer fetches a fresh cert+key pair from
        // the local tailscaled API, and the rest of the runtime serves it via
        // the same get_certificate path ACME certs flow through.
        //
        // spec: TLS#retry-blocks-and-retries — but the dispatch happens *after* the
        // decision, not before it. Dispatching first routed around the only
        // thing that paces issuance: while tailscaled was down, every
        // reconciler tick opened and finalised a failed attempt row, and the
        // 1000-row cap on Snapshot::load then evicted other hostnames'
        // last_attempt — dissolving their debounce too. A subsystem with a
        // central decision function admits no dispatch before the decision.
        if self.dispatch_to_tailscale(hostname)? {
            check_retry_gate(hostname, trigger, &owned)?;
            return self.run_tailscale(hostname, trigger).await;
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
            // Canopy requests go through the relay, not through this
            // coordinator.
            TlsPolicy::Canopy => None,
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

/// The subset of [`handle_non_issuing`] that paces retries, for providers
/// that have no ACME policy to consult but must still honour an operator
/// block and the failure debounce.
///
/// `Blocked` and `Debounced` are about *whether to attempt now*, and apply to
/// any issuer. The remaining decisions are about ACME policy — which DNS
/// provider, whether a current cert already covers the hostname — and mean
/// nothing to an issuer that fetches from a local API.
// spec: TLS#retry-blocks-and-retries
fn check_retry_gate(hostname: &str, trigger: AttemptTrigger, owned: &OwnedState) -> Result<()> {
    match &owned.decision {
        Decision::Blocked { reason } => Err(CoordinatorError::Paused {
            hostname: hostname.to_owned(),
            reason: reason
                .clone()
                .or_else(|| owned.retry_block_reason.clone())
                .unwrap_or_else(|| "no reason given".to_owned()),
        }),
        // A manual trigger bypasses the debounce, as it does for ACME.
        Decision::Debounced { until } if trigger != AttemptTrigger::Manual => {
            let now = jiff::Timestamp::now().as_second();
            Err(CoordinatorError::Debounced {
                hostname: hostname.to_owned(),
                remaining_s: (until - now).max(0),
            })
        }
        _ => Ok(()),
    }
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
        // A hostname on a strategy chain is carried out by the proxy or by
        // Canopy, never by an ACME-DNS flow, and has no policy to run one
        // from.
        Decision::Default | Decision::Chain(_) => Err(CoordinatorError::NoPolicy {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An operator issuance overlapping a background one, finishing first or
    /// last, must leave the hostname in flight until both are done.
    // spec: TLS#issuance
    #[test]
    fn a_hostname_stays_in_flight_until_every_issuance_for_it_finishes() {
        let in_flight = Arc::<InFlight>::default();

        let background = in_flight.claim_if_idle("foo.example.com").unwrap();
        assert!(in_flight.claim_if_idle("foo.example.com").is_none());

        let operator = in_flight.claim("foo.example.com");
        drop(operator);
        assert!(
            in_flight.claim_if_idle("foo.example.com").is_none(),
            "the operator's issuance finishing must not release the background one's claim"
        );

        let operator = in_flight.claim("foo.example.com");
        drop(background);
        assert!(in_flight.claim_if_idle("foo.example.com").is_none());
        drop(operator);
        assert!(!in_flight.is_running("foo.example.com"));
        assert!(in_flight.claim_if_idle("foo.example.com").is_some());
    }
}
