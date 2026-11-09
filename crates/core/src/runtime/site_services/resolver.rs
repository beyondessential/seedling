//! Background DNS resolver for site service endpoints.
//!
//! Maintains a `host → {AAAA, A}` cache, refreshing entries at TTL or on a
//! short maximum cadence. Reads `/etc/resolv.conf` via hickory-resolver so the
//! daemon picks up split-DNS, MagicDNS, and corporate resolvers configured on
//! the host. Cache reads happen at every reconcile tick through the
//! [`HostnameLookup`] trait — no per-lookup DNS query — and the resolver
//! kicks the reconciler when a host's resolved set changes so route /
//! nftables state catches up promptly.
//
// r[impl service.site.address]

use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::{Duration, Instant},
};

use hickory_resolver::{
    TokioResolver,
    net::{DnsError, NetError},
    proto::rr::RData,
};
use parking_lot::{Mutex, RwLock};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};
use tracing::{debug, warn};

use crate::runtime::{
    db::DbHandle,
    site_services::{
        self,
        resolve::{HostKind, HostnameLookup, HostnameLookupResult},
    },
};

/// Maximum interval between refreshes of a single host's records, even when
/// the underlying TTL is longer. Keeps the cache from growing very stale.
const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
/// Floor on a successful entry's lifetime, capping the floor that very-low-TTL
/// records would impose on our refresh loop.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How long a failed lookup is considered "still failing" before a retry is
/// allowed.
const FAILURE_BACKOFF: Duration = Duration::from_secs(5);
/// Number of consecutive failures after which the resolver flags the host as
/// `failed` — the reconciler turns that into a fault.
const FAULT_AFTER_FAILURES: u32 = 5;
/// How often the background loop wakes up to look for entries due for refresh.
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// A DNS address family, named by the record type that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// IPv6 addresses, from AAAA records.
    Aaaa,
    /// IPv4 addresses, from A records.
    A,
}

impl Family {
    /// The record type's name, as it appears in logs and on the status route.
    pub fn record_type(self) -> &'static str {
        match self {
            Self::Aaaa => "AAAA",
            Self::A => "A",
        }
    }
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.record_type())
    }
}

/// Single-host cache record. Two never-yet-resolved hostnames look the same
/// to consumers as `lookup() -> None`; only entries that have completed at
/// least one query (success or failure) appear in the map.
#[derive(Debug, Clone)]
struct CacheEntry {
    aaaa: Vec<Ipv6Addr>,
    a: Vec<Ipv4Addr>,
    fetched_at: Instant,
    expires_at: Instant,
    /// `true` when the most recent attempt errored. The reconciler turns a
    /// `failed` entry with no records into an `Unresolved::LookupFailed`
    /// outcome.
    last_attempt_failed: bool,
    /// The address family whose last lookup failed while the other family
    /// answered, so that family's records are its previous answer rather
    /// than a fresh one.
    degraded_family: Option<Family>,
}

impl CacheEntry {
    fn is_empty(&self) -> bool {
        self.aaaa.is_empty() && self.a.is_empty()
    }
}

/// Public-facing snapshot of one entry, exposed via the OI resolver-status
/// route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverStatusEntry {
    pub host: String,
    pub aaaa: Vec<Ipv6Addr>,
    pub a: Vec<Ipv4Addr>,
    pub last_attempt_failed: bool,
    /// The family whose lookup is failing while the other answers. The
    /// records of that family are the last it returned, so a name with a
    /// degraded family is resolving on less than it appears to.
    pub degraded_family: Option<Family>,
    pub age: Duration,
    pub ttl_remaining: Duration,
}

struct ResolverInner {
    db: DbHandle,
    cache: RwLock<HashMap<String, CacheEntry>>,
    failure_streak: Mutex<HashMap<String, u32>>,
    fault_hosts: RwLock<HashSet<String>>,
    kick_tx: mpsc::Sender<()>,
    tick_notify: Arc<Notify>,
}

impl ResolverInner {
    fn record_success(
        &self,
        host: &str,
        aaaa: Vec<Ipv6Addr>,
        a: Vec<Ipv4Addr>,
        ttl: Duration,
    ) -> bool {
        self.store(host, aaaa, a, ttl, None)
    }

    /// Records a lookup in which one family's query failed. The family that
    /// failed keeps its previously cached answer, since a failed query says
    /// nothing about what the family holds. Returns whether the resolved set
    /// changed and whether the host has newly become degraded.
    fn record_partial(&self, host: &str, partial: &PartialLookup) -> (bool, bool) {
        let failed_family = partial.failed_family();
        let prev = self.cache.read().get(host).cloned();
        let newly_degraded = prev
            .as_ref()
            .is_none_or(|p| p.degraded_family != Some(failed_family));
        let (prev_aaaa, prev_a) = prev.map(|p| (p.aaaa, p.a)).unwrap_or_default();
        let (aaaa, a) = match partial {
            PartialLookup::AaaaFailed { a, .. } => (prev_aaaa, a.clone()),
            PartialLookup::AFailed { aaaa, .. } => (aaaa.clone(), prev_a),
        };
        let changed = self.store(host, aaaa, a, FAILURE_BACKOFF, Some(failed_family));
        (changed, newly_degraded)
    }

    fn store(
        &self,
        host: &str,
        aaaa: Vec<Ipv6Addr>,
        a: Vec<Ipv4Addr>,
        ttl: Duration,
        degraded_family: Option<Family>,
    ) -> bool {
        let ttl = ttl.clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL);
        let now = Instant::now();

        let prev_changed = {
            let mut cache = self.cache.write();
            let prev = cache.get(host).cloned();
            cache.insert(
                host.to_owned(),
                CacheEntry {
                    aaaa: aaaa.clone(),
                    a: a.clone(),
                    fetched_at: now,
                    expires_at: now + ttl,
                    last_attempt_failed: false,
                    degraded_family,
                },
            );
            match prev {
                Some(p) => p.aaaa != aaaa || p.a != a,
                None => true,
            }
        };

        self.failure_streak.lock().remove(host);
        self.fault_hosts.write().remove(host);
        prev_changed
    }

    fn record_failure(&self, host: &str) -> (u32, bool) {
        let now = Instant::now();
        let mut streak = self.failure_streak.lock();
        let count = streak.entry(host.to_owned()).or_insert(0);
        *count += 1;
        let count = *count;
        drop(streak);

        let mut cache = self.cache.write();
        let entry = cache.entry(host.to_owned()).or_insert(CacheEntry {
            aaaa: Vec::new(),
            a: Vec::new(),
            fetched_at: now,
            expires_at: now + FAILURE_BACKOFF,
            last_attempt_failed: false,
            degraded_family: None,
        });
        entry.fetched_at = now;
        entry.expires_at = now + FAILURE_BACKOFF;
        entry.last_attempt_failed = true;
        // The whole lookup failed, so nothing is known about either family
        // and a previous "this one family is failing" no longer holds.
        entry.degraded_family = None;
        let became_unresolved_change = count == FAULT_AFTER_FAILURES && entry.is_empty();
        if became_unresolved_change {
            self.fault_hosts.write().insert(host.to_owned());
        }
        (count, became_unresolved_change)
    }
}

impl HostnameLookup for ResolverInner {
    fn lookup(&self, host: &str) -> Option<HostnameLookupResult> {
        self.cache.read().get(host).map(|e| HostnameLookupResult {
            aaaa: e.aaaa.clone(),
            a: e.a.clone(),
            failed: e.last_attempt_failed,
        })
    }
}

pub struct SiteServiceResolver {
    inner: Arc<ResolverInner>,
}

impl SiteServiceResolver {
    /// Create the resolver and spawn its background task. Returns the
    /// resolver and the join handle so the daemon can shut it down cleanly.
    pub fn spawn(db: DbHandle, tick_notify: Arc<Notify>) -> (Arc<Self>, JoinHandle<()>) {
        let (kick_tx, kick_rx) = mpsc::channel(8);
        let inner = Arc::new(ResolverInner {
            db,
            cache: RwLock::new(HashMap::new()),
            failure_streak: Mutex::new(HashMap::new()),
            fault_hosts: RwLock::new(HashSet::new()),
            kick_tx,
            tick_notify,
        });
        let resolver = match build_resolver() {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(
                    error = %e,
                    "site-service resolver: failed to read system DNS config; \
                     site-service DNS endpoints will remain unresolved"
                );
                None
            }
        };
        let inner_for_task = Arc::clone(&inner);
        let handle = tokio::spawn(async move {
            run_loop(inner_for_task, resolver, kick_rx).await;
        });

        (Arc::new(Self { inner }), handle)
    }

    /// Test-only constructor that doesn't spawn a task and doesn't reach for
    /// the system DNS config. Tests populate the cache directly.
    #[cfg(test)]
    pub fn for_tests(db: DbHandle) -> Arc<Self> {
        let (kick_tx, _kick_rx) = mpsc::channel(8);
        let tick_notify = Arc::new(Notify::new());
        let inner = Arc::new(ResolverInner {
            db,
            cache: RwLock::new(HashMap::new()),
            failure_streak: Mutex::new(HashMap::new()),
            fault_hosts: RwLock::new(HashSet::new()),
            kick_tx,
            tick_notify,
        });
        Arc::new(Self { inner })
    }

    /// Force the background loop to refresh now. Called by the OI handlers
    /// when an endpoint is added so a freshly-typed hostname doesn't wait for
    /// the next periodic tick.
    pub fn kick(&self) {
        let _ = self.inner.kick_tx.try_send(());
    }

    /// Snapshot of the current cache for the OI resolver-status route.
    pub fn status(&self) -> Vec<ResolverStatusEntry> {
        let now = Instant::now();
        let cache = self.inner.cache.read();
        cache
            .iter()
            .map(|(host, entry)| {
                let age = now.saturating_duration_since(entry.fetched_at);
                let ttl_remaining = entry.expires_at.saturating_duration_since(now);
                ResolverStatusEntry {
                    host: host.clone(),
                    aaaa: entry.aaaa.clone(),
                    a: entry.a.clone(),
                    last_attempt_failed: entry.last_attempt_failed,
                    // r[impl service.site.address.partial-answer]
                    degraded_family: entry.degraded_family,
                    age,
                    ttl_remaining,
                }
            })
            .collect()
    }

    /// The set of hostnames that have crossed the failure threshold and
    /// should be reported as `site_service_endpoint_unresolvable` faults.
    pub fn unresolved_hosts(&self) -> HashSet<String> {
        self.inner.fault_hosts.read().clone()
    }

    /// Test helper: install a fully-populated cache entry without hitting
    /// the network. Mirrors what a successful lookup would have stored.
    #[cfg(test)]
    pub fn seed_success(&self, host: &str, aaaa: &[&str], a: &[&str], ttl: Duration) -> bool {
        self.inner.record_success(
            host,
            aaaa.iter().map(|s| s.parse().unwrap()).collect(),
            a.iter().map(|s| s.parse().unwrap()).collect(),
            ttl,
        )
    }

    /// Test helper: simulate one DNS failure and return the resulting
    /// `(streak, crossed_threshold)` so tests can assert fault behaviour.
    #[cfg(test)]
    pub fn seed_failure(&self, host: &str) -> (u32, bool) {
        self.inner.record_failure(host)
    }
}

impl HostnameLookup for SiteServiceResolver {
    fn lookup(&self, host: &str) -> Option<HostnameLookupResult> {
        self.inner.lookup(host)
    }
}

fn build_resolver() -> Result<TokioResolver, NetError> {
    TokioResolver::builder_tokio().map(|b| b.build())?
}

async fn run_loop(
    inner: Arc<ResolverInner>,
    resolver: Option<TokioResolver>,
    mut kick_rx: mpsc::Receiver<()>,
) {
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Initial bootstrap: populate the work set from the DB.
    refresh_due_hosts(&inner, resolver.as_ref()).await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                refresh_due_hosts(&inner, resolver.as_ref()).await;
            }
            recv = kick_rx.recv() => {
                if recv.is_none() {
                    debug!("site-service resolver: kick channel closed; exiting");
                    return;
                }
                refresh_due_hosts(&inner, resolver.as_ref()).await;
            }
        }
    }
}

async fn refresh_due_hosts(inner: &Arc<ResolverInner>, resolver: Option<&TokioResolver>) {
    // Collect the distinct DNS-named hosts referenced by site service rows.
    let endpoints = match inner.db.call(site_services::list) {
        Ok(svcs) => svcs,
        Err(e) => {
            warn!(error = %e, "site-service resolver: failed to list site services");
            return;
        }
    };

    let mut wanted: HashSet<String> = HashSet::new();
    for svc in &endpoints {
        for ep in &svc.endpoints {
            if matches!(HostKind::classify(&ep.remote_host), HostKind::Dns(_)) {
                wanted.insert(ep.remote_host.clone());
            }
        }
    }

    // GC: drop cache entries whose host is no longer referenced and clear
    // any associated fault flag.
    {
        let mut cache = inner.cache.write();
        cache.retain(|host, _| wanted.contains(host));
        let mut streaks = inner.failure_streak.lock();
        streaks.retain(|host, _| wanted.contains(host));
        let mut faults = inner.fault_hosts.write();
        faults.retain(|host| wanted.contains(host));
    }

    let now = Instant::now();
    let due: Vec<String> = wanted
        .into_iter()
        .filter(|host| {
            let cache = inner.cache.read();
            match cache.get(host) {
                Some(entry) => entry.expires_at <= now,
                None => true,
            }
        })
        .collect();

    let Some(resolver) = resolver else {
        if !due.is_empty() {
            warn!(
                count = due.len(),
                "site-service resolver: no system DNS config; skipping refresh"
            );
        }
        return;
    };

    let mut any_change = false;
    for host in due {
        match lookup_records(resolver, &host).await {
            Ok(Lookup::Complete { aaaa, a, ttl }) => {
                if inner.record_success(&host, aaaa, a, ttl) {
                    any_change = true;
                }
            }
            Ok(Lookup::Partial(partial)) => {
                let (changed, newly_degraded) = inner.record_partial(&host, &partial);
                if changed {
                    any_change = true;
                }
                let family = partial.failed_family();
                let error = partial.error();
                if newly_degraded {
                    warn!(
                        host = %host,
                        %family,
                        "site-service resolver: {family} lookup failed; \
                         resolving from the other family: {error}"
                    );
                } else {
                    debug!(host = %host, %family, "site-service {family} lookup still failing: {error}");
                }
            }
            Err(e) => {
                let (streak, became_failed) = inner.record_failure(&host);
                if became_failed {
                    any_change = true;
                }
                if streak == FAULT_AFTER_FAILURES {
                    warn!(
                        host = %host,
                        attempt = streak,
                        "site-service resolver: lookup keeps failing: {e}"
                    );
                } else {
                    debug!(host = %host, attempt = streak, "site-service lookup failed: {e}");
                }
            }
        }
    }

    if any_change {
        inner.tick_notify.notify_one();
    }
}

/// The outcome of looking up both address families of a name.
enum Lookup {
    /// Both queries completed, whether or not they returned records.
    Complete {
        aaaa: Vec<Ipv6Addr>,
        a: Vec<Ipv4Addr>,
        ttl: Duration,
    },
    /// One query failed while the other returned records.
    Partial(PartialLookup),
}

/// A lookup where one family's query errored while the other returned
/// records. The variant names the family that failed and carries what the
/// family that answered returned, so the two cannot disagree.
enum PartialLookup {
    /// The AAAA query failed; the A query returned these records.
    AaaaFailed { a: Vec<Ipv4Addr>, error: String },
    /// The A query failed; the AAAA query returned these records.
    AFailed { aaaa: Vec<Ipv6Addr>, error: String },
}

impl PartialLookup {
    fn failed_family(&self) -> Family {
        match self {
            Self::AaaaFailed { .. } => Family::Aaaa,
            Self::AFailed { .. } => Family::A,
        }
    }

    fn error(&self) -> &str {
        match self {
            Self::AaaaFailed { error, .. } | Self::AFailed { error, .. } => error,
        }
    }
}

/// One family's query result: its records and the instant they stop being
/// valid, or the error that stopped the query.
type FamilyResult<T, E> = Result<(Vec<T>, Option<Instant>), E>;

async fn lookup_records(resolver: &TokioResolver, host: &str) -> Result<Lookup, NetError> {
    let now = Instant::now();
    let aaaa = match resolver.ipv6_lookup(host).await {
        Ok(lookup) => {
            let v6: Vec<Ipv6Addr> = lookup
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::AAAA(a) => Some(a.0),
                    _ => None,
                })
                .collect();
            Ok((v6, Some(lookup.valid_until())))
        }
        Err(NetError::Dns(DnsError::NoRecordsFound(_))) => Ok((Vec::new(), None)),
        Err(other) => Err(other),
    };

    let a = match resolver.ipv4_lookup(host).await {
        Ok(lookup) => {
            let v4: Vec<Ipv4Addr> = lookup
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::A(addr) => Some(addr.0),
                    _ => None,
                })
                .collect();
            Ok((v4, Some(lookup.valid_until())))
        }
        Err(NetError::Dns(DnsError::NoRecordsFound(_))) => Ok((Vec::new(), None)),
        Err(other) => Err(other),
    };

    combine_families(aaaa, a, now)
}

// r[impl service.site.address.partial-answer]
fn combine_families<E: std::fmt::Display>(
    aaaa: FamilyResult<Ipv6Addr, E>,
    a: FamilyResult<Ipv4Addr, E>,
    now: Instant,
) -> Result<Lookup, E> {
    match (aaaa, a) {
        (Ok(aaaa), Ok(a)) => {
            let valid = [aaaa.1, a.1]
                .into_iter()
                .flatten()
                .min()
                .map(|deadline| deadline.saturating_duration_since(now))
                .unwrap_or(MAX_REFRESH_INTERVAL);

            if aaaa.0.is_empty() && a.0.is_empty() {
                // Both queries succeeded with empty answer sections: treat as
                // "resolved to no records", not a transport failure. Cache it for
                // a short period so we don't hammer DNS.
                return Ok(Lookup::Complete {
                    aaaa: Vec::new(),
                    a: Vec::new(),
                    ttl: MIN_REFRESH_INTERVAL,
                });
            }

            Ok(Lookup::Complete {
                aaaa: aaaa.0,
                a: a.0,
                ttl: valid,
            })
        }
        (Err(e), Ok((a, _))) if !a.is_empty() => Ok(Lookup::Partial(PartialLookup::AaaaFailed {
            a,
            error: e.to_string(),
        })),
        (Ok((aaaa, _)), Err(e)) if !aaaa.is_empty() => {
            Ok(Lookup::Partial(PartialLookup::AFailed {
                aaaa,
                error: e.to_string(),
            }))
        }
        // A failure with nothing from the other family to stand in for it
        // leaves the name's addresses unknown, which is a failed lookup and
        // not an answer of "no records".
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::site_services::resolve::{
        HostnameLookup, ResolveCtx, ResolveOutcome, resolve_endpoint,
    };

    fn mkresolver() -> Arc<SiteServiceResolver> {
        let handle = DbHandle::open_in_memory().expect("open in-memory db");
        SiteServiceResolver::for_tests(handle)
    }

    #[test]
    fn seed_success_populates_cache_and_reports_change() {
        let r = mkresolver();
        let changed = r.seed_success(
            "db.example.com",
            &["2001:db8::1"],
            &["10.0.0.1"],
            Duration::from_secs(60),
        );
        assert!(changed);

        let res = r.lookup("db.example.com").expect("entry present");
        assert_eq!(res.aaaa, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
        assert_eq!(res.a, vec!["10.0.0.1".parse::<Ipv4Addr>().unwrap()]);
        assert!(!res.failed);
    }

    #[test]
    fn seed_success_again_with_same_data_is_not_a_change() {
        let r = mkresolver();
        r.seed_success(
            "db.example.com",
            &["2001:db8::1"],
            &[],
            Duration::from_secs(60),
        );
        let changed = r.seed_success(
            "db.example.com",
            &["2001:db8::1"],
            &[],
            Duration::from_secs(60),
        );
        assert!(!changed);
    }

    #[test]
    fn seed_failure_files_fault_after_threshold() {
        let r = mkresolver();
        for i in 1..FAULT_AFTER_FAILURES {
            let (count, fault) = r.seed_failure("missing.example");
            assert_eq!(count, i);
            assert!(!fault);
            assert!(!r.unresolved_hosts().contains("missing.example"));
        }
        let (count, fault) = r.seed_failure("missing.example");
        assert_eq!(count, FAULT_AFTER_FAILURES);
        assert!(fault);
        assert!(r.unresolved_hosts().contains("missing.example"));
    }

    #[test]
    fn success_after_failure_clears_streak_and_fault() {
        let r = mkresolver();
        for _ in 0..FAULT_AFTER_FAILURES {
            r.seed_failure("flapping.example");
        }
        assert!(r.unresolved_hosts().contains("flapping.example"));

        r.seed_success(
            "flapping.example",
            &["2001:db8::1"],
            &[],
            Duration::from_secs(60),
        );
        assert!(!r.unresolved_hosts().contains("flapping.example"));

        let res = r.lookup("flapping.example").unwrap();
        assert!(!res.failed);
    }

    type Fam<T> = FamilyResult<T, String>;

    fn v6(addrs: &[&str]) -> Fam<Ipv6Addr> {
        Ok((addrs.iter().map(|s| s.parse().unwrap()).collect(), None))
    }

    fn v4(addrs: &[&str]) -> Fam<Ipv4Addr> {
        Ok((addrs.iter().map(|s| s.parse().unwrap()).collect(), None))
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn failed_aaaa_with_a_records_is_a_partial_lookup() {
        let out = combine_families(
            Err("servfail".to_owned()),
            v4(&["10.0.0.1"]),
            Instant::now(),
        );
        match out {
            Ok(Lookup::Partial(PartialLookup::AaaaFailed { a, .. })) => {
                assert_eq!(a, vec!["10.0.0.1".parse::<Ipv4Addr>().unwrap()]);
            }
            _ => panic!("expected the AAAA family to have failed"),
        }
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn failed_a_with_aaaa_records_is_a_partial_lookup() {
        let out = combine_families(
            v6(&["2001:db8::1"]),
            Err("refused".to_owned()),
            Instant::now(),
        );
        match out {
            Ok(Lookup::Partial(PartialLookup::AFailed { aaaa, .. })) => {
                assert_eq!(aaaa, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
            }
            _ => panic!("expected the A family to have failed"),
        }
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn failed_family_with_nothing_from_the_other_is_a_failure() {
        let out = combine_families(Err("servfail".to_owned()), v4(&[]), Instant::now());
        assert!(out.is_err());
        let out = combine_families(v6(&[]), Err("servfail".to_owned()), Instant::now());
        assert!(out.is_err());
        let out =
            combine_families::<String>(Err("a".to_owned()), Err("b".to_owned()), Instant::now());
        assert!(out.is_err());
    }

    #[test]
    fn both_families_empty_is_resolved_to_no_records() {
        match combine_families(v6(&[]), v4(&[]), Instant::now()) {
            Ok(Lookup::Complete { aaaa, a, ttl }) => {
                assert!(aaaa.is_empty() && a.is_empty());
                assert_eq!(ttl, MIN_REFRESH_INTERVAL);
            }
            _ => panic!("expected a complete empty lookup"),
        }
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn partial_lookup_keeps_failed_family_answer_and_clears_failure_state() {
        let r = mkresolver();
        r.seed_success(
            "dual.example",
            &["2001:db8::1"],
            &["10.0.0.1"],
            Duration::from_secs(60),
        );
        for _ in 0..FAULT_AFTER_FAILURES {
            r.seed_failure("dual.example");
        }

        let (changed, newly_degraded) = r.inner.record_partial(
            "dual.example",
            &PartialLookup::AaaaFailed {
                a: vec!["10.0.0.2".parse().unwrap()],
                error: "servfail".to_owned(),
            },
        );
        assert!(changed);
        assert!(newly_degraded);
        assert!(!r.unresolved_hosts().contains("dual.example"));

        let res = r.lookup("dual.example").unwrap();
        assert_eq!(res.aaaa, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
        assert_eq!(res.a, vec!["10.0.0.2".parse::<Ipv4Addr>().unwrap()]);

        let (_, newly_degraded) = r.inner.record_partial(
            "dual.example",
            &PartialLookup::AaaaFailed {
                a: vec!["10.0.0.2".parse().unwrap()],
                error: "servfail".to_owned(),
            },
        );
        assert!(!newly_degraded);
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn status_names_the_degraded_family_until_the_lookup_recovers() {
        let r = mkresolver();
        r.inner.record_partial(
            "dual.example",
            &PartialLookup::AaaaFailed {
                a: vec!["10.0.0.1".parse().unwrap()],
                error: "servfail".to_owned(),
            },
        );
        let entry = |r: &SiteServiceResolver| {
            r.status()
                .into_iter()
                .find(|e| e.host == "dual.example")
                .expect("host in status")
        };
        assert_eq!(entry(&r).degraded_family, Some(Family::Aaaa));

        // A lookup that answers on both families is no longer degraded.
        r.seed_success(
            "dual.example",
            &["2001:db8::1"],
            &["10.0.0.1"],
            Duration::from_secs(60),
        );
        assert_eq!(entry(&r).degraded_family, None);
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn a_wholly_failed_lookup_clears_the_degraded_family() {
        // Both families failing says nothing about either, so the claim that
        // one of them alone is at fault must not outlive it.
        let r = mkresolver();
        r.inner.record_partial(
            "dual.example",
            &PartialLookup::AaaaFailed {
                a: vec!["10.0.0.1".parse().unwrap()],
                error: "servfail".to_owned(),
            },
        );
        r.seed_failure("dual.example");
        let entry = r
            .status()
            .into_iter()
            .find(|e| e.host == "dual.example")
            .expect("host in status");
        assert_eq!(entry.degraded_family, None);
        assert!(entry.last_attempt_failed);
    }

    // r[verify service.site.address.partial-answer]
    #[test]
    fn partial_lookup_for_new_host_leaves_failed_family_empty() {
        let r = mkresolver();
        r.inner.record_partial(
            "v4only.example",
            &PartialLookup::AaaaFailed {
                a: vec!["10.0.0.1".parse().unwrap()],
                error: "servfail".to_owned(),
            },
        );
        let res = r.lookup("v4only.example").unwrap();
        assert!(res.aaaa.is_empty());
        assert_eq!(res.a.len(), 1);
        assert!(!res.failed);
    }

    #[test]
    fn resolve_endpoint_consumes_resolver_cache() {
        // End-to-end: drive resolve_endpoint through the resolver's
        // HostnameLookup impl with both seeded entries and missing names.
        let r = mkresolver();
        r.seed_success("v6.example", &["2001:db8::1"], &[], Duration::from_secs(60));
        r.seed_success("v4.example", &[], &["10.0.0.1"], Duration::from_secs(60));

        let lookup: &dyn HostnameLookup = r.as_ref();
        let ctx = ResolveCtx {
            nat64_active: true,
            has_ipv6_egress: true,
            resolver: lookup,
        };

        match resolve_endpoint("v6.example", &ctx) {
            ResolveOutcome::Routable(addrs) => {
                assert_eq!(addrs, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
            }
            other => panic!("expected Routable, got {other:?}"),
        }

        match resolve_endpoint("v4.example", &ctx) {
            ResolveOutcome::Routable(addrs) => {
                assert_eq!(addrs, vec!["64:ff9b::a00:1".parse::<Ipv6Addr>().unwrap()]);
            }
            other => panic!("expected Routable, got {other:?}"),
        }

        match resolve_endpoint("missing.example", &ctx) {
            ResolveOutcome::Unresolved { .. } => {}
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }
}
