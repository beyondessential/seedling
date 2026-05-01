use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::runtime::nat64_prefix::synth_v4;

/// Classification of a `remote_host` string.
///
/// Site service endpoints accept three shapes; everything past validation in
/// the OI/CLI layer falls into one of these. Anything else has been rejected
/// upstream — but we still treat unparseable input as `Dns` of one final
/// failed lookup, because the DB column carries no shape constraint and a
/// migrated-in row could otherwise crash reconcile.
// r[impl service.site.address]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    Ipv6(Ipv6Addr),
    Ipv4(Ipv4Addr),
    Dns(String),
}

impl HostKind {
    /// Classify a `remote_host` string. IPv6 and IPv4 literals are recognised;
    /// anything else is treated as a DNS name (further validity is an OI/CLI
    /// concern, not the reconciler's).
    pub fn classify(host: &str) -> Self {
        if let Ok(v6) = host.parse::<Ipv6Addr>() {
            Self::Ipv6(v6)
        } else if let Ok(v4) = host.parse::<Ipv4Addr>() {
            Self::Ipv4(v4)
        } else {
            Self::Dns(host.to_owned())
        }
    }
}

/// Lookup result for a DNS-named endpoint. The resolver task populates this
/// per host; consumers read it through [`HostnameLookup`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostnameLookupResult {
    pub aaaa: Vec<Ipv6Addr>,
    pub a: Vec<Ipv4Addr>,
    /// True when the most recent lookup attempt failed and we have no
    /// previously cached records to fall back to. The reconciler treats this
    /// as `Unresolved`.
    pub failed: bool,
}

/// Read-only access to the resolver cache. The reconciler builds a
/// [`ResolveCtx`] each tick that holds a borrow of the live cache; tests use
/// the simple [`StaticHostnameLookup`] in this module.
pub trait HostnameLookup {
    fn lookup(&self, host: &str) -> Option<HostnameLookupResult>;
}

/// Always-empty `HostnameLookup`. Used when the daemon failed to read system
/// DNS config and the live resolver cache isn't available — every name
/// resolves to `Unresolved::NotInCache`.
pub struct EmptyLookup;

impl HostnameLookup for EmptyLookup {
    fn lookup(&self, _host: &str) -> Option<HostnameLookupResult> {
        None
    }
}

/// Per-tick resolution inputs. Built in the reconciler from the active NAT64
/// flag, the host's IPv6 egress probe result, and a borrow of the resolver
/// cache.
pub struct ResolveCtx<'a> {
    pub nat64_active: bool,
    pub has_ipv6_egress: bool,
    pub resolver: &'a dyn HostnameLookup,
}

/// Outcome of resolving a single endpoint's `remote_host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// One or more IPv6-routable addresses suitable for the data plane. The
    /// addresses are either native AAAA or the NAT64 synthesis of A records;
    /// the data plane treats both identically.
    Routable(Vec<Ipv6Addr>),
    /// `remote_host` parses but routing is impossible from this host: NAT64
    /// is required (v4 literal or A-only DNS) but unavailable.
    Unroutable { reason: UnroutableReason },
    /// DNS resolution has not produced any usable records yet, or has failed.
    Unresolved { reason: UnresolvedReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnroutableReason {
    /// IPv4 literal or A-only DNS name on a host where NAT64 is not active.
    NeedsNat64ButDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// The hostname has not yet been queried (resolver hasn't run).
    NotInCache,
    /// The most recent lookup failed and there is no prior cached result to
    /// fall back to.
    LookupFailed,
}

/// Resolve a single `remote_host` string into the data-plane address list (or
/// a structured failure). See `r[service.site.address]`.
// r[impl service.site.address]
pub fn resolve_endpoint(host: &str, ctx: &ResolveCtx<'_>) -> ResolveOutcome {
    match HostKind::classify(host) {
        HostKind::Ipv6(v6) => ResolveOutcome::Routable(vec![v6]),
        HostKind::Ipv4(v4) => {
            if ctx.nat64_active {
                ResolveOutcome::Routable(vec![synth_v4(v4)])
            } else {
                ResolveOutcome::Unroutable {
                    reason: UnroutableReason::NeedsNat64ButDisabled,
                }
            }
        }
        HostKind::Dns(name) => resolve_dns(&name, ctx),
    }
}

fn resolve_dns(name: &str, ctx: &ResolveCtx<'_>) -> ResolveOutcome {
    let Some(result) = ctx.resolver.lookup(name) else {
        return ResolveOutcome::Unresolved {
            reason: UnresolvedReason::NotInCache,
        };
    };

    // AAAA first when the host has IPv6 egress: prefer native v6 over
    // NAT64-synthesised v6 so we don't push dual-stack traffic through the
    // translator unnecessarily. On hosts without v6 egress, AAAA records
    // would route to a dead path — ignore them and use NAT64 for any A
    // records instead.
    if ctx.has_ipv6_egress && !result.aaaa.is_empty() {
        return ResolveOutcome::Routable(result.aaaa.clone());
    }

    if !result.a.is_empty() {
        if ctx.nat64_active {
            return ResolveOutcome::Routable(result.a.iter().copied().map(synth_v4).collect());
        }
        return ResolveOutcome::Unroutable {
            reason: UnroutableReason::NeedsNat64ButDisabled,
        };
    }

    // No A records, and either no AAAA records at all or none we can reach.
    if !result.aaaa.is_empty() {
        // We have AAAA but no v6 egress: there is no IPv4 fallback either,
        // so the endpoint is effectively unroutable.
        return ResolveOutcome::Unroutable {
            reason: UnroutableReason::NeedsNat64ButDisabled,
        };
    }

    if result.failed {
        ResolveOutcome::Unresolved {
            reason: UnresolvedReason::LookupFailed,
        }
    } else {
        ResolveOutcome::Unresolved {
            reason: UnresolvedReason::NotInCache,
        }
    }
}

/// Convenience: also surface the IpAddr form for callers that compose into
/// `(Ipv6Addr, port)` tuples for the data plane.
pub fn into_ip_addrs(outcome: &ResolveOutcome) -> Vec<IpAddr> {
    match outcome {
        ResolveOutcome::Routable(addrs) => addrs.iter().copied().map(IpAddr::V6).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
pub use tests::StaticHostnameLookup;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Test-only `HostnameLookup` backed by a `HashMap`. Hosts not in the map
    /// return `None` (i.e. `NotInCache`).
    #[derive(Debug, Default, Clone)]
    pub struct StaticHostnameLookup {
        pub entries: HashMap<String, HostnameLookupResult>,
    }

    impl StaticHostnameLookup {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn insert(&mut self, host: &str, aaaa: &[&str], a: &[&str], failed: bool) -> &mut Self {
            self.entries.insert(
                host.to_owned(),
                HostnameLookupResult {
                    aaaa: aaaa.iter().map(|s| s.parse().unwrap()).collect(),
                    a: a.iter().map(|s| s.parse().unwrap()).collect(),
                    failed,
                },
            );
            self
        }
    }

    impl HostnameLookup for StaticHostnameLookup {
        fn lookup(&self, host: &str) -> Option<HostnameLookupResult> {
            self.entries.get(host).cloned()
        }
    }

    fn ctx<'a>(nat64: bool, v6_egress: bool, resolver: &'a dyn HostnameLookup) -> ResolveCtx<'a> {
        ResolveCtx {
            nat64_active: nat64,
            has_ipv6_egress: v6_egress,
            resolver,
        }
    }

    #[test]
    fn ipv6_literal_routes_natively_regardless_of_nat64_state() {
        let r = StaticHostnameLookup::new();
        for nat64 in [true, false] {
            for v6 in [true, false] {
                let outcome = resolve_endpoint("2001:db8::1", &ctx(nat64, v6, &r));
                assert_eq!(
                    outcome,
                    ResolveOutcome::Routable(vec!["2001:db8::1".parse().unwrap()])
                );
            }
        }
    }

    #[test]
    fn ipv4_literal_synthesised_when_nat64_active() {
        let r = StaticHostnameLookup::new();
        let outcome = resolve_endpoint("192.0.2.10", &ctx(true, false, &r));
        assert_eq!(
            outcome,
            ResolveOutcome::Routable(vec!["64:ff9b::c000:20a".parse().unwrap()])
        );
    }

    #[test]
    fn ipv4_literal_unroutable_without_nat64() {
        let r = StaticHostnameLookup::new();
        let outcome = resolve_endpoint("192.0.2.10", &ctx(false, false, &r));
        assert!(matches!(
            outcome,
            ResolveOutcome::Unroutable {
                reason: UnroutableReason::NeedsNat64ButDisabled
            }
        ));
    }

    #[test]
    fn dns_unknown_host_returns_not_in_cache() {
        let r = StaticHostnameLookup::new();
        let outcome = resolve_endpoint("example.com", &ctx(true, true, &r));
        assert!(matches!(
            outcome,
            ResolveOutcome::Unresolved {
                reason: UnresolvedReason::NotInCache
            }
        ));
    }

    #[test]
    fn dns_aaaa_preferred_when_v6_egress_present() {
        let mut r = StaticHostnameLookup::new();
        r.insert(
            "db.example.com",
            &["2001:db8::5", "2001:db8::6"],
            &["10.0.0.1"],
            false,
        );
        let outcome = resolve_endpoint("db.example.com", &ctx(true, true, &r));
        match outcome {
            ResolveOutcome::Routable(addrs) => {
                let mut sorted = addrs;
                sorted.sort();
                assert_eq!(
                    sorted,
                    vec![
                        "2001:db8::5".parse::<Ipv6Addr>().unwrap(),
                        "2001:db8::6".parse::<Ipv6Addr>().unwrap()
                    ]
                );
            }
            other => panic!("expected Routable, got {other:?}"),
        }
    }

    #[test]
    fn dns_a_only_with_nat64_synthesises() {
        let mut r = StaticHostnameLookup::new();
        r.insert("db.example.com", &[], &["10.0.0.1", "10.0.0.2"], false);
        let outcome = resolve_endpoint("db.example.com", &ctx(true, true, &r));
        match outcome {
            ResolveOutcome::Routable(addrs) => {
                let mut sorted = addrs;
                sorted.sort();
                assert_eq!(
                    sorted,
                    vec![
                        "64:ff9b::a00:1".parse::<Ipv6Addr>().unwrap(),
                        "64:ff9b::a00:2".parse::<Ipv6Addr>().unwrap()
                    ]
                );
            }
            other => panic!("expected Routable, got {other:?}"),
        }
    }

    #[test]
    fn dns_dual_stack_without_v6_egress_uses_a_via_nat64() {
        let mut r = StaticHostnameLookup::new();
        r.insert("db.example.com", &["2001:db8::5"], &["10.0.0.1"], false);
        let outcome = resolve_endpoint("db.example.com", &ctx(true, false, &r));
        assert_eq!(
            outcome,
            ResolveOutcome::Routable(vec!["64:ff9b::a00:1".parse().unwrap()])
        );
    }

    #[test]
    fn dns_a_only_without_nat64_unroutable() {
        let mut r = StaticHostnameLookup::new();
        r.insert("db.example.com", &[], &["10.0.0.1"], false);
        let outcome = resolve_endpoint("db.example.com", &ctx(false, true, &r));
        assert!(matches!(
            outcome,
            ResolveOutcome::Unroutable {
                reason: UnroutableReason::NeedsNat64ButDisabled
            }
        ));
    }

    #[test]
    fn dns_aaaa_only_without_v6_egress_is_unroutable() {
        let mut r = StaticHostnameLookup::new();
        r.insert("db.example.com", &["2001:db8::5"], &[], false);
        // No A fallback and no v6 egress → no path works.
        let outcome = resolve_endpoint("db.example.com", &ctx(true, false, &r));
        assert!(matches!(
            outcome,
            ResolveOutcome::Unroutable {
                reason: UnroutableReason::NeedsNat64ButDisabled
            }
        ));
    }

    #[test]
    fn dns_failed_with_no_records_is_unresolved() {
        let mut r = StaticHostnameLookup::new();
        r.insert("missing.example.com", &[], &[], true);
        let outcome = resolve_endpoint("missing.example.com", &ctx(true, true, &r));
        assert!(matches!(
            outcome,
            ResolveOutcome::Unresolved {
                reason: UnresolvedReason::LookupFailed
            }
        ));
    }
}
