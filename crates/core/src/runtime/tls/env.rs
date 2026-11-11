//! Gathering the facts about the world that a hostname's strategy chain
//! depends on and the database does not hold.
//!
//! One function, so the issuance paths, the operator interface and the
//! certificate endpoint cannot each assemble a slightly different picture of
//! what Canopy and the proxy are doing and so disagree about which strategy is
//! responsible.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Arc,
};

use parking_lot::Mutex;

use super::state::{ChainEnv, ManagedIngress, ProxyCert, ProxyHolding};
use crate::runtime::{canopy::store as canopy_store, db::Db};
use crate::system::caddy::CertRead;

/// Where the facts a chain depends on come from, for callers that do not hold
/// the daemon's state themselves: the certificate endpoint, and the issuance
/// coordinator that serves it.
pub trait Environment: Send + Sync {
    /// Every TLS-terminating ingress, and the facts a chain depends on for them.
    ///
    /// For all of them at once, never for one: what is gathered is shared by
    /// every decision made against it, and one narrowed to a single hostname
    /// would say of every other that no app declares it.
    ///
    /// `None` when they could not be gathered. That is not an empty answer, and
    /// no caller may act on it as one.
    fn gather(&self) -> Option<(Vec<ManagedIngress>, ChainEnv)>;

    /// What the proxy holds for one hostname, read now.
    ///
    /// The one fact the gate cannot take from the gathered facts: those are as
    /// old as the moment they were gathered, and the proxy may have taken a
    /// certificate for the hostname since. Answering anything but a
    /// fall-through for a hostname it holds one for fails that hostname's
    /// handshakes, so this is read at the moment it is acted on.
    fn holding(&self, hostname: &str) -> ProxyHolding;
}

/// What the proxy's cache holds for one hostname.
///
/// `caddy_data` is where the cache lives, and `None` when it could not be
/// located, which is not the proxy holding nothing.
pub fn proxy_holding(caddy_data: Option<&Path>, hostname: &str) -> ProxyHolding {
    let Some(path) = caddy_data else {
        return ProxyHolding::Unreadable;
    };
    match crate::system::caddy::read_caddy_cert_state(path, hostname) {
        CertRead::Present(view) => ProxyHolding::Holds(ProxyCert {
            not_before: view.not_before,
            not_after: view.not_after,
            issuer_kind: view.issuer_kind,
            issuer: view.issuer,
            self_signed: view.self_signed,
        }),
        CertRead::Absent => ProxyHolding::Nothing,
        CertRead::Unreadable => ProxyHolding::Unreadable,
    }
}

/// Read what the proxy holds for each of `managed`, from its certificate cache.
///
/// Disk work and nothing else, so it is done off the database thread: that
/// thread serialises every database call in the daemon, and a directory walk and
/// a certificate parse per hostname on it would be paid by every unrelated
/// query. `caddy_data` is where the cache lives, and `None` when it could not be
/// located, which leaves every hostname out of the map: not looked at, which is
/// not the same as the proxy holding nothing.
pub fn proxy_holdings(
    managed: &[ManagedIngress],
    caddy_data: Option<&Path>,
) -> BTreeMap<String, ProxyHolding> {
    let mut proxy = BTreeMap::new();
    let Some(path) = caddy_data else {
        return proxy;
    };
    let hostnames: BTreeSet<&str> = managed.iter().map(|m| m.hostname.as_str()).collect();
    for hostname in hostnames {
        proxy.insert(hostname.to_owned(), proxy_holding(Some(path), hostname));
    }
    proxy
}

/// Build the [`ChainEnv`] for `managed` ingresses.
///
/// `offer_live` is whether Canopy access is enabled and a client is offering to
/// carry requests. `proxy` is what [`proxy_holdings`] read, which the caller does
/// before entering the database thread.
pub fn collect(
    db: &Db,
    managed: &[ManagedIngress],
    offer_live: bool,
    proxy: BTreeMap<String, ProxyHolding>,
) -> rusqlite::Result<ChainEnv> {
    let types_by_app: BTreeMap<String, String> = canopy_store::list_apps(db)?
        .into_iter()
        .map(|a| (a.app, a.canopy_type))
        .collect();

    let mut canopy_types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ingress in managed {
        let entry = canopy_types.entry(ingress.hostname.clone()).or_default();
        if let Some(canopy_type) = types_by_app.get(ingress.app.as_str()) {
            entry.insert(canopy_type.clone());
        }
    }

    Ok(ChainEnv {
        known: true,
        offer_live,
        canopy_types,
        proxy,
    })
}

/// What the runtime itself holds for some hostnames.
///
/// Each part is `None` when the store could not be read for it. That is not an
/// empty set: not knowing is not holding nothing, and a caller must treat it as
/// not yet seen rather than as failed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RuntimeHoldings {
    /// Hostnames a certificate the runtime stores can be served for right now.
    pub served: Option<BTreeSet<String>>,
    /// Hostnames with a Canopy order open.
    pub ordering: Option<BTreeSet<String>>,
}

/// The facts read from stored certificates, kept from one read to the next.
///
/// A certificate is parsed once and not on every reconciler tick: nearly all of
/// what is stored is the same at the next tick as it was at this one. An entry
/// is trusted only while the PEM it was read from is the PEM stored, since a
/// renewal replaces a Canopy certificate's chain under the same id.
#[derive(Clone, Default)]
pub struct LeafFactsCache(Arc<Mutex<HashMap<i64, CachedFacts>>>);

type CachedFacts = (String, Option<Arc<super::parse::LeafFacts>>);

impl LeafFactsCache {
    /// The facts of the certificate `id` stored as `pem`, or `None` when it
    /// cannot be read. An unreadable certificate is remembered as such too, so
    /// it is not parsed again on every tick.
    fn facts(&self, id: i64, pem: &str) -> Option<Arc<super::parse::LeafFacts>> {
        let mut entries = self.0.lock();
        if let Some((cached_pem, facts)) = entries.get(&id)
            && cached_pem == pem
        {
            return facts.clone();
        }
        let facts = super::parse::leaf_facts(pem).ok().map(Arc::new);
        entries.insert(id, (pem.to_owned(), facts.clone()));
        facts
    }

    /// Forget every certificate not in `ids`, so what is no longer stored does
    /// not stay.
    fn keep_only(&self, ids: &BTreeSet<i64>) {
        self.0.lock().retain(|id, _| ids.contains(id));
    }
}

/// Which of `hostnames` the runtime serves a certificate for, and which have an
/// order open.
///
/// The proxy's own cache cannot answer the first: a certificate the runtime
/// serves to the proxy is never written there.
// r[impl tls.canopy.serve]
pub fn runtime_holdings(db: &Db, hostnames: &[String], cache: &LeafFactsCache) -> RuntimeHoldings {
    let now = jiff::Timestamp::now().as_second();

    // One pass over the certificates that could be served, ranking each against
    // every hostname at once: the same ranking the serving lookup applies, so a
    // hostname counts as served here exactly when the proxy would be handed
    // something for it.
    let served = match super::store::list_servable(db, now) {
        Ok(certificates) => {
            cache.keep_only(&certificates.iter().map(|c| c.id).collect());
            let mut served = BTreeSet::new();
            for cert in certificates {
                let Some(facts) = cache.facts(cert.id, &cert.cert_pem) else {
                    // Unreadable is not "covers nothing", and the serving
                    // lookup reports it; nothing more is said here.
                    continue;
                };
                for hostname in hostnames {
                    if served.contains(hostname) {
                        continue;
                    }
                    if super::resolve::rank(
                        &facts.san_dns_names,
                        hostname,
                        facts.self_issued,
                        cert.not_after,
                        cert.created_at,
                        cert.id,
                        now,
                    )
                    .is_some()
                    {
                        served.insert(hostname.clone());
                    }
                }
            }
            Some(served)
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot read the certificate store");
            None
        }
    };

    let ordering = match super::store::get_chain_rows(db, hostnames) {
        Ok(rows) => Some(
            rows.into_iter()
                .filter(|(_, row)| row.row.open_order_since.is_some())
                .map(|(hostname, _)| hostname)
                .collect(),
        ),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read the chain state");
            None
        }
    };

    RuntimeHoldings { served, ordering }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tls::{
        KeyType, TlsCertOrigin, TlsCertState, chain,
        store::{self, CertMetadata, NewCertificate},
        test_support,
    };

    fn insert(db: &Db, hostname: &str, state: TlsCertState, not_after: i64) {
        store::insert_certificate(
            db,
            NewCertificate {
                hostname,
                requested_hostnames: None,
                state,
                origin: TlsCertOrigin::Canopy,
                cert_pem: Some(&test_support::self_signed_pem(&[hostname])),
                csr_pem: None,
                key_ciphertext: &[0u8; 8],
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata {
                    not_before: Some(0),
                    not_after: Some(not_after),
                    ..Default::default()
                },
                note: None,
                acme_account_id: None,
                renews: None,
            },
        )
        .unwrap();
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn a_stored_certificate_in_its_window_is_a_certificate_the_hostname_has() {
        let db = Db::open_in_memory().unwrap();
        let future = jiff::Timestamp::now().as_second() + 86_400;
        insert(&db, "ok.example.com", TlsCertState::Active, future);
        insert(&db, "expired.example.com", TlsCertState::Active, 100);
        insert(&db, "revoked.example.com", TlsCertState::Revoked, future);
        insert(&db, "waiting.example.com", TlsCertState::CsrPending, future);

        let hostnames: Vec<String> = [
            "ok.example.com",
            "expired.example.com",
            "revoked.example.com",
            "waiting.example.com",
            "none.example.com",
        ]
        .map(String::from)
        .into();
        let holdings = runtime_holdings(&db, &hostnames, &LeafFactsCache::default());
        assert_eq!(
            holdings.served,
            Some(BTreeSet::from(["ok.example.com".to_owned()])),
            "an expired, revoked or not-yet-delivered certificate does not satisfy a barrier"
        );
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn an_open_order_is_seen_as_one() {
        let db = Db::open_in_memory().unwrap();
        store::put_chain_row(
            &db,
            "ordering.example.com",
            &chain::Row {
                identity: chain::Identity::NONE,
                step: 0,
                since: 1,
                open_order_since: Some(1),
                retry_not_before: None,
            },
            None,
        )
        .unwrap();
        store::put_chain_row(
            &db,
            "refused.example.com",
            &chain::Row {
                identity: chain::Identity::NONE,
                step: 0,
                since: 1,
                open_order_since: None,
                retry_not_before: Some(5),
            },
            Some("no"),
        )
        .unwrap();
        let hostnames = vec![
            "ordering.example.com".to_owned(),
            "refused.example.com".to_owned(),
        ];
        let holdings = runtime_holdings(&db, &hostnames, &LeafFactsCache::default());
        assert_eq!(
            holdings.ordering,
            Some(BTreeSet::from(["ordering.example.com".to_owned()]))
        );
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn one_pass_answers_for_every_hostname_at_once() {
        let db = Db::open_in_memory().unwrap();
        let future = jiff::Timestamp::now().as_second() + 86_400;
        insert(&db, "a.example.com", TlsCertState::Active, future);
        insert(&db, "b.example.com", TlsCertState::Active, future);
        insert(&db, "stale.example.com", TlsCertState::Active, 100);

        let hostnames: Vec<String> = [
            "a.example.com",
            "b.example.com",
            "stale.example.com",
            "none.example.com",
        ]
        .map(String::from)
        .into();
        let holdings = runtime_holdings(&db, &hostnames, &LeafFactsCache::default());
        assert_eq!(
            holdings.served,
            Some(BTreeSet::from([
                "a.example.com".to_owned(),
                "b.example.com".to_owned()
            ])),
            "each hostname is answered by the one pass, and an expired one is not served"
        );
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn a_wildcard_certificate_serves_the_hostnames_it_covers() {
        let db = Db::open_in_memory().unwrap();
        let future = jiff::Timestamp::now().as_second() + 86_400;
        store::insert_certificate(
            &db,
            NewCertificate {
                hostname: "*.example.com",
                requested_hostnames: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::Canopy,
                cert_pem: Some(&test_support::self_signed_pem(&["*.example.com"])),
                csr_pem: None,
                key_ciphertext: &[0u8; 8],
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata {
                    not_before: Some(0),
                    not_after: Some(future),
                    ..Default::default()
                },
                note: None,
                acme_account_id: None,
                renews: None,
            },
        )
        .unwrap();

        let hostnames = vec!["one.example.com".to_owned(), "two.example.org".to_owned()];
        let holdings = runtime_holdings(&db, &hostnames, &LeafFactsCache::default());
        assert_eq!(
            holdings.served,
            Some(BTreeSet::from(["one.example.com".to_owned()])),
            "coverage is what binds a certificate to a hostname, not its label"
        );
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn a_renewed_chain_under_the_same_id_is_read_again() {
        let cache = LeafFactsCache::default();
        let one = test_support::self_signed_pem(&["one.example.com"]);
        let two = test_support::self_signed_pem(&["two.example.com"]);
        assert_eq!(
            cache.facts(1, &one).unwrap().san_dns_names,
            ["one.example.com"]
        );
        assert_eq!(
            cache.facts(1, &two).unwrap().san_dns_names,
            ["two.example.com"],
            "what was read from other PEM is not what the stored one says"
        );
        cache.keep_only(&BTreeSet::new());
        assert!(cache.0.lock().is_empty());
    }

    // r[verify tls.canopy.serve]
    #[test]
    fn a_store_that_cannot_be_read_is_not_a_store_that_holds_nothing() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute_batch("DROP TABLE tls_certificates")
            .unwrap();
        let holdings = runtime_holdings(
            &db,
            &["a.example.com".to_owned()],
            &LeafFactsCache::default(),
        );
        assert_eq!(holdings.served, None);
        assert_eq!(holdings.ordering, Some(BTreeSet::new()));
    }
}
