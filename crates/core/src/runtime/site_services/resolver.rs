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
// spec: SITE#endpoint-addressing

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
use tracing::{debug, error, warn};

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
/// The longest the background loop sleeps before it looks for entries due for
/// refresh.
const TICK_INTERVAL: Duration = Duration::from_secs(5);
/// The shortest it sleeps, so an entry that is already due does not spin it.
const MIN_WAKE: Duration = Duration::from_millis(500);

/// A DNS address family, named by the record type that carries it.
///
/// Ordered as the record types' names are, which is the order the status
/// method lists failed families in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    /// IPv4 addresses, from A records.
    A,
    /// IPv6 addresses, from AAAA records.
    Aaaa,
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
    /// `true` when the most recent lookup returned no records and a family's
    /// query failed. The reconciler turns a `failed` entry with no records
    /// into an `Unresolved::LookupFailed` outcome.
    last_attempt_failed: bool,
    /// The families whose most recent query failed, so that family's records
    /// are its previous answer rather than a fresh one.
    failed_families: Vec<Family>,
}

/// Public-facing snapshot of one entry, exposed via the OI resolver-status
/// route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverStatusEntry {
    pub host: String,
    pub aaaa: Vec<Ipv6Addr>,
    pub a: Vec<Ipv4Addr>,
    pub last_attempt_failed: bool,
    /// The families whose most recent lookup failed, in the order of their
    /// record types' names. The records of such a family are the last it
    /// returned, so a name with a failed family is resolving on less than it
    /// appears to.
    pub failed_families: Vec<Family>,
    pub age: Duration,
    pub ttl_remaining: Duration,
}

/// What a lookup of both address families gave, one family at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lookup {
    /// The AAAA records, or why the query failed. A query that found no
    /// records is `Ok` of none.
    aaaa: Result<Vec<Ipv6Addr>, String>,
    /// The A records, or why the query failed.
    a: Result<Vec<Ipv4Addr>, String>,
    /// How long the records that were returned stay valid, `None` where none
    /// were.
    valid_for: Option<Duration>,
}

impl Lookup {
    /// Both queries failed for one reason, as when there is no way to query.
    fn failed_outright(error: &str) -> Self {
        Self {
            aaaa: Err(error.to_owned()),
            a: Err(error.to_owned()),
            valid_for: None,
        }
    }

    fn failed_families(&self) -> Vec<Family> {
        let mut failed = Vec::new();
        if self.a.is_err() {
            failed.push(Family::A);
        }
        if self.aaaa.is_err() {
            failed.push(Family::Aaaa);
        }
        failed
    }

    /// A lookup is unanswered when it returned no records from either family,
    /// whether a family's query failed or both found none.
    // spec: SITE#endpoint-addressing
    fn unanswered(&self) -> bool {
        self.aaaa.as_ref().map_or(true, Vec::is_empty) && self.a.as_ref().map_or(true, Vec::is_empty)
    }

    fn first_error(&self) -> Option<&str> {
        self.aaaa
            .as_ref()
            .err()
            .or_else(|| self.a.as_ref().err())
            .map(String::as_str)
    }
}

/// What recording a lookup changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Recorded {
    /// Something the reconciler reads moved: the records, whether the lookup
    /// failed, or whether the name is unresolvable.
    pub changed: bool,
    /// How many lookups in a row have been unanswered, counting this one.
    pub unanswered_streak: u32,
    /// Whether a family has newly started failing.
    pub newly_failing: bool,
}

struct ResolverInner {
    db: DbHandle,
    cache: RwLock<HashMap<String, CacheEntry>>,
    unanswered_streak: Mutex<HashMap<String, u32>>,
    fault_hosts: RwLock<HashSet<String>>,
    kick_tx: mpsc::Sender<()>,
    tick_notify: Arc<Notify>,
}

impl ResolverInner {
    /// Record what a lookup of `host` gave.
    ///
    /// A family whose query failed keeps its previous answer, since a failed
    /// query says nothing about what the family holds; a family whose query
    /// succeeded takes the new one, empty or not. A name is unresolvable once
    /// five lookups in a row have been unanswered, and a lookup that returned
    /// records ends that run.
    // spec: SITE#partial-dns-answers
    // spec: SITE#endpoint-addressing
    fn record(&self, host: &str, lookup: &Lookup) -> Recorded {
        let ttl = if !lookup.failed_families().is_empty() {
            FAILURE_BACKOFF
        } else {
            lookup
                .valid_for
                .unwrap_or(MIN_REFRESH_INTERVAL)
                .clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL)
        };
        let failed_families = lookup.failed_families();
        let unanswered = lookup.unanswered();
        let last_attempt_failed = unanswered && !failed_families.is_empty();
        let now = Instant::now();

        let (resolved_changed, newly_failing) = {
            let mut cache = self.cache.write();
            let prev = cache.get(host).cloned();
            let (prev_aaaa, prev_a) = prev
                .as_ref()
                .map(|p| (p.aaaa.clone(), p.a.clone()))
                .unwrap_or_default();
            let aaaa = lookup.aaaa.clone().unwrap_or(prev_aaaa);
            let a = lookup.a.clone().unwrap_or(prev_a);
            let changed = match &prev {
                Some(p) => {
                    p.aaaa != aaaa || p.a != a || p.last_attempt_failed != last_attempt_failed
                }
                None => true,
            };
            let newly_failing = failed_families
                .iter()
                .any(|f| prev.as_ref().is_none_or(|p| !p.failed_families.contains(f)));
            cache.insert(
                host.to_owned(),
                CacheEntry {
                    aaaa,
                    a,
                    fetched_at: now,
                    expires_at: now + ttl,
                    last_attempt_failed,
                    failed_families,
                },
            );
            (changed, newly_failing)
        };

        let streak = {
            let mut streaks = self.unanswered_streak.lock();
            if unanswered {
                let count = streaks.entry(host.to_owned()).or_insert(0);
                *count += 1;
                *count
            } else {
                streaks.remove(host);
                0
            }
        };
        let fault_changed = {
            let mut faults = self.fault_hosts.write();
            if streak >= FAULT_AFTER_FAILURES {
                faults.insert(host.to_owned())
            } else {
                faults.remove(host)
            }
        };

        Recorded {
            changed: resolved_changed || fault_changed,
            unanswered_streak: streak,
            newly_failing,
        }
    }

    /// How long until the soonest entry is due for another lookup, or `None`
    /// when nothing is cached.
    fn until_next_due(&self) -> Option<Duration> {
        let now = Instant::now();
        self.cache
            .read()
            .values()
            .map(|e| e.expires_at.saturating_duration_since(now))
            .min()
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
    ///
    /// A system DNS configuration that cannot be read is looked for again on
    /// each refresh; until it is read, every lookup is unanswered, so a name
    /// that stays so is reported unresolvable.
    pub fn spawn(db: DbHandle, tick_notify: Arc<Notify>) -> (Arc<Self>, JoinHandle<()>) {
        let (kick_tx, kick_rx) = mpsc::channel(8);
        let inner = Arc::new(ResolverInner {
            db,
            cache: RwLock::new(HashMap::new()),
            unanswered_streak: Mutex::new(HashMap::new()),
            fault_hosts: RwLock::new(HashSet::new()),
            kick_tx,
            tick_notify,
        });
        let inner_for_task = Arc::clone(&inner);
        let handle = tokio::spawn(async move {
            run_loop(inner_for_task, kick_rx).await;
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
            unanswered_streak: Mutex::new(HashMap::new()),
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

    /// Snapshot of the current cache for the OI resolver-status route,
    /// ordered by host.
    pub fn status(&self) -> Vec<ResolverStatusEntry> {
        let now = Instant::now();
        let cache = self.inner.cache.read();
        let mut entries: Vec<ResolverStatusEntry> = cache
            .iter()
            .map(|(host, entry)| {
                let age = now.saturating_duration_since(entry.fetched_at);
                let ttl_remaining = entry.expires_at.saturating_duration_since(now);
                ResolverStatusEntry {
                    host: host.clone(),
                    aaaa: entry.aaaa.clone(),
                    a: entry.a.clone(),
                    last_attempt_failed: entry.last_attempt_failed,
                    // spec: SITE#partial-dns-answers
                    failed_families: entry.failed_families.clone(),
                    age,
                    ttl_remaining,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.host.cmp(&b.host));
        entries
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
        self.inner
            .record(
                host,
                &Lookup {
                    aaaa: Ok(aaaa.iter().map(|s| s.parse().unwrap()).collect()),
                    a: Ok(a.iter().map(|s| s.parse().unwrap()).collect()),
                    valid_for: Some(ttl),
                },
            )
            .changed
    }

    /// Test helper: simulate one lookup in which both queries failed and
    /// return the number of unanswered lookups in a row, and whether the name
    /// has become unresolvable.
    #[cfg(test)]
    pub fn seed_failure(&self, host: &str) -> (u32, bool) {
        let recorded = self.inner.record(host, &Lookup::failed_outright("test"));
        (
            recorded.unanswered_streak,
            recorded.unanswered_streak == FAULT_AFTER_FAILURES,
        )
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

async fn run_loop(inner: Arc<ResolverInner>, mut kick_rx: mpsc::Receiver<()>) {
    let mut resolver = Resolver::default();

    // Initial bootstrap: populate the work set from the DB.
    refresh_due_hosts(&inner, &mut resolver).await;

    loop {
        // Wake for the soonest entry that falls due, so a name whose lookup
        // partly failed is looked up again within the time the spec allows
        // rather than at the next fixed tick.
        let wait = inner
            .until_next_due()
            .map_or(TICK_INTERVAL, |due| due.min(TICK_INTERVAL))
            .max(MIN_WAKE);
        tokio::select! {
            () = tokio::time::sleep(wait) => {
                refresh_due_hosts(&inner, &mut resolver).await;
            }
            recv = kick_rx.recv() => {
                if recv.is_none() {
                    debug!("site-service resolver: kick channel closed; exiting");
                    return;
                }
                refresh_due_hosts(&inner, &mut resolver).await;
            }
        }
    }
}

/// The system resolver, built the first time it can be and looked for again
/// until then.
#[derive(Default)]
struct Resolver {
    built: Option<TokioResolver>,
    /// The reason the last attempt to build one failed, so a failure that
    /// persists is reported once and not on every refresh.
    unbuildable: Option<String>,
}

impl Resolver {
    fn get(&mut self) -> Result<&TokioResolver, String> {
        if self.built.is_none() {
            match build_resolver() {
                Ok(r) => {
                    if self.unbuildable.take().is_some() {
                        warn!("site-service resolver: the system DNS config can be read again");
                    }
                    self.built = Some(r);
                }
                Err(e) => {
                    let why = e.to_string();
                    if self.unbuildable.as_deref() != Some(why.as_str()) {
                        warn!(
                            error = %why,
                            "site-service resolver: failed to read the system DNS config; \
                             looking again on each refresh, and site-service DNS endpoints \
                             stay unresolved until it can be read"
                        );
                    }
                    self.unbuildable = Some(why.clone());
                    return Err(why);
                }
            }
        }
        Ok(self.built.as_ref().expect("built above"))
    }
}

async fn refresh_due_hosts(inner: &Arc<ResolverInner>, resolver: &mut Resolver) {
    // Collect the distinct DNS-named hosts referenced by site service rows.
    let endpoints = match inner.db.call(site_services::list) {
        Ok(svcs) => svcs,
        Err(e) => {
            error!(
                error = %e,
                "site-service resolver: failed to list site services; \
                 not refreshing, and looking again at the next wake"
            );
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
        let mut streaks = inner.unanswered_streak.lock();
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
    if due.is_empty() {
        return;
    }

    let mut any_change = false;
    for host in due {
        let lookup = match resolver.get() {
            Ok(r) => lookup_records(r, &host).await,
            Err(why) => Lookup::failed_outright(&why),
        };
        let recorded = inner.record(&host, &lookup);
        any_change |= recorded.changed;
        report(&host, &lookup, recorded);
    }

    if any_change {
        inner.tick_notify.notify_one();
    }
}

fn report(host: &str, lookup: &Lookup, recorded: Recorded) {
    let failed = lookup.failed_families();
    let Some(error) = lookup.first_error() else {
        if recorded.unanswered_streak == FAULT_AFTER_FAILURES {
            warn!(
                host = %host,
                attempt = recorded.unanswered_streak,
                "site-service resolver: lookups keep returning no records"
            );
        }
        return;
    };
    let families = failed
        .iter()
        .map(|f| f.record_type())
        .collect::<Vec<_>>()
        .join(" and ");
    if lookup.unanswered() {
        if recorded.unanswered_streak == FAULT_AFTER_FAILURES {
            warn!(
                host = %host,
                attempt = recorded.unanswered_streak,
                "site-service resolver: lookup keeps failing: {error}"
            );
        } else {
            debug!(
                host = %host,
                attempt = recorded.unanswered_streak,
                "site-service lookup failed: {error}"
            );
        }
    } else if recorded.newly_failing {
        warn!(
            host = %host,
            "site-service resolver: {families} lookup failed; \
             resolving from the other family: {error}"
        );
    } else {
        debug!(host = %host, "site-service {families} lookup still failing: {error}");
    }
}

/// Look up both address families of `host`, at once: neither feeds the other,
/// and a family that is failing usually costs a full DNS timeout, which would
/// otherwise be added to the other's, in a refresh that goes through its due
/// hosts one after another.
// spec: SITE#partial-dns-answers
async fn lookup_records(resolver: &TokioResolver, host: &str) -> Lookup {
    let now = Instant::now();
    let (aaaa, a) = tokio::join!(
        async {
            match resolver.ipv6_lookup(host).await {
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
                Err(other) => Err(other.to_string()),
            }
        },
        async {
            match resolver.ipv4_lookup(host).await {
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
                Err(other) => Err(other.to_string()),
            }
        },
    );

    combine_families(aaaa, a, now)
}

type FamilyResult<T> = Result<(Vec<T>, Option<Instant>), String>;

/// Join the two families' results into a [`Lookup`], whose validity is the
/// shortest of those of the records that were returned.
// spec: SITE#partial-dns-answers
fn combine_families(
    aaaa: FamilyResult<Ipv6Addr>,
    a: FamilyResult<Ipv4Addr>,
    now: Instant,
) -> Lookup {
    let valid_until = [
        aaaa.as_ref().ok().and_then(|(r, v)| (!r.is_empty()).then_some(*v)).flatten(),
        a.as_ref().ok().and_then(|(r, v)| (!r.is_empty()).then_some(*v)).flatten(),
    ]
    .into_iter()
    .flatten()
    .min();
    Lookup {
        aaaa: aaaa.map(|(records, _)| records),
        a: a.map(|(records, _)| records),
        valid_for: valid_until.map(|deadline| deadline.saturating_duration_since(now)),
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

    fn v6(addrs: &[&str]) -> FamilyResult<Ipv6Addr> {
        Ok((addrs.iter().map(|s| s.parse().unwrap()).collect(), None))
    }

    fn v4(addrs: &[&str]) -> FamilyResult<Ipv4Addr> {
        Ok((addrs.iter().map(|s| s.parse().unwrap()).collect(), None))
    }

    fn lookup(aaaa: FamilyResult<Ipv6Addr>, a: FamilyResult<Ipv4Addr>) -> Lookup {
        combine_families(aaaa, a, Instant::now())
    }

    fn entry(r: &SiteServiceResolver, host: &str) -> ResolverStatusEntry {
        r.status()
            .into_iter()
            .find(|e| e.host == host)
            .expect("host in status")
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn a_failed_family_is_named_whatever_the_other_returned() {
        let out = lookup(Err("servfail".to_owned()), v4(&["10.0.0.1"]));
        assert_eq!(out.failed_families(), [Family::Aaaa]);
        assert!(!out.unanswered());

        let out = lookup(v6(&["2001:db8::1"]), Err("refused".to_owned()));
        assert_eq!(out.failed_families(), [Family::A]);
        assert!(!out.unanswered());

        // The other family returning nothing leaves the lookup unanswered,
        // but still says which family failed.
        let out = lookup(Err("servfail".to_owned()), v4(&[]));
        assert_eq!(out.failed_families(), [Family::Aaaa]);
        assert!(out.unanswered());

        let out = lookup(Err("a".to_owned()), Err("b".to_owned()));
        assert_eq!(out.failed_families(), [Family::A, Family::Aaaa]);
        assert!(out.unanswered());
    }

    // spec: SITE#endpoint-addressing
    #[test]
    fn a_lookup_that_found_no_records_is_unanswered_though_nothing_failed() {
        let out = lookup(v6(&[]), v4(&[]));
        assert!(out.failed_families().is_empty());
        assert!(out.unanswered());
        assert_eq!(out.valid_for, None);
    }

    // spec: SITE#endpoint-addressing
    #[test]
    fn five_lookups_in_a_row_with_no_records_make_a_name_unresolvable() {
        let r = mkresolver();
        let empty = lookup(v6(&[]), v4(&[]));
        for i in 1..FAULT_AFTER_FAILURES {
            assert_eq!(r.inner.record("none.example", &empty).unanswered_streak, i);
            assert!(!r.unresolved_hosts().contains("none.example"));
        }
        r.inner.record("none.example", &empty);
        assert!(r.unresolved_hosts().contains("none.example"));

        // One lookup that returns records ends the run.
        r.seed_success("none.example", &[], &["10.0.0.1"], Duration::from_secs(60));
        assert!(!r.unresolved_hosts().contains("none.example"));
        assert_eq!(r.inner.record("none.example", &empty).unanswered_streak, 1);
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn a_partial_answer_ends_a_run_of_unanswered_lookups() {
        let r = mkresolver();
        for _ in 0..FAULT_AFTER_FAILURES - 1 {
            r.seed_failure("flaky.example");
        }
        let partial = lookup(Err("servfail".to_owned()), v4(&["10.0.0.1"]));
        let recorded = r.inner.record("flaky.example", &partial);
        assert_eq!(recorded.unanswered_streak, 0);
        r.seed_failure("flaky.example");
        assert!(!r.unresolved_hosts().contains("flaky.example"));
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn a_failed_family_keeps_its_last_answer_and_the_other_takes_the_new_one() {
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

        let partial = lookup(Err("servfail".to_owned()), v4(&["10.0.0.2"]));
        let recorded = r.inner.record("dual.example", &partial);
        assert!(recorded.changed);
        assert!(recorded.newly_failing);
        assert!(!r.unresolved_hosts().contains("dual.example"));

        let res = r.lookup("dual.example").unwrap();
        assert_eq!(res.aaaa, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
        assert_eq!(res.a, vec!["10.0.0.2".parse::<Ipv4Addr>().unwrap()]);

        assert!(!r.inner.record("dual.example", &partial).newly_failing);
    }

    // spec: SITE#partial-dns-answers
    // A failed lookup is not the family having no records, and a family that
    // answered with none says so, though the other family failed.
    #[test]
    fn a_failed_lookup_keeps_the_last_answer_of_the_family_that_failed_only() {
        let r = mkresolver();
        r.seed_success(
            "dual.example",
            &["2001:db8::1"],
            &["10.0.0.1"],
            Duration::from_secs(60),
        );
        r.inner
            .record("dual.example", &lookup(Err("servfail".to_owned()), v4(&[])));
        let res = r.lookup("dual.example").unwrap();
        assert_eq!(res.aaaa, vec!["2001:db8::1".parse::<Ipv6Addr>().unwrap()]);
        assert!(res.a.is_empty(), "the A query answered, with nothing");
        assert!(res.failed);
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn status_names_each_failed_family_until_the_lookup_recovers() {
        let r = mkresolver();
        r.inner.record(
            "dual.example",
            &lookup(Err("servfail".to_owned()), v4(&["10.0.0.1"])),
        );
        assert_eq!(entry(&r, "dual.example").failed_families, [Family::Aaaa]);
        assert!(!entry(&r, "dual.example").last_attempt_failed);

        // Both failing names both, in the order of the record types' names.
        r.seed_failure("dual.example");
        let failed = entry(&r, "dual.example");
        assert_eq!(failed.failed_families, [Family::A, Family::Aaaa]);
        assert!(failed.last_attempt_failed);

        r.seed_success(
            "dual.example",
            &["2001:db8::1"],
            &["10.0.0.1"],
            Duration::from_secs(60),
        );
        assert!(entry(&r, "dual.example").failed_families.is_empty());
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn a_name_whose_family_failed_is_looked_up_again_within_ten_seconds() {
        let r = mkresolver();
        r.inner.record(
            "dual.example",
            &lookup(Err("servfail".to_owned()), v4(&["10.0.0.1"])),
        );
        let wait = r.inner.until_next_due().unwrap();
        assert!(wait <= Duration::from_secs(10), "{wait:?}");
        // Whatever lifetime the records came with.
        let long = Lookup {
            aaaa: Err("servfail".to_owned()),
            a: Ok(vec!["10.0.0.1".parse().unwrap()]),
            valid_for: Some(Duration::from_secs(3600)),
        };
        r.inner.record("long.example", &long);
        assert!(r.inner.until_next_due().unwrap() <= Duration::from_secs(5));
        let at_least = r
            .status()
            .into_iter()
            .find(|e| e.host == "long.example")
            .unwrap()
            .ttl_remaining;
        assert!(at_least <= Duration::from_secs(5));
    }

    // spec: SITE#partial-dns-answers
    #[test]
    fn partial_lookup_for_new_host_leaves_failed_family_empty() {
        let r = mkresolver();
        r.inner.record(
            "v4only.example",
            &lookup(Err("servfail".to_owned()), v4(&["10.0.0.1"])),
        );
        let res = r.lookup("v4only.example").unwrap();
        assert!(res.aaaa.is_empty());
        assert_eq!(res.a.len(), 1);
        assert!(!res.failed);
    }

    // spec: SITE#endpoint-addressing
    // A system DNS configuration that cannot be read makes every lookup
    // unanswered, so a name that stays so is reported unresolvable.
    #[test]
    fn lookups_with_no_way_to_query_count_as_unanswered() {
        let r = mkresolver();
        for _ in 0..FAULT_AFTER_FAILURES {
            r.inner
                .record("db.example", &Lookup::failed_outright("no resolv.conf"));
        }
        assert!(r.unresolved_hosts().contains("db.example"));
        assert_eq!(
            entry(&r, "db.example").failed_families,
            [Family::A, Family::Aaaa]
        );
    }

    #[test]
    fn status_is_ordered_by_host() {
        let r = mkresolver();
        for host in ["b.example", "c.example", "a.example"] {
            r.seed_success(host, &[], &["10.0.0.1"], Duration::from_secs(60));
        }
        let hosts: Vec<_> = r.status().into_iter().map(|e| e.host).collect();
        assert_eq!(hosts, ["a.example", "b.example", "c.example"]);
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
