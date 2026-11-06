//! Gathering the facts about the world that a hostname's strategy chain
//! depends on and the database does not hold.
//!
//! One function, so the issuance paths, the operator interface and the
//! certificate endpoint cannot each assemble a slightly different picture of
//! what Canopy and the proxy are doing and so disagree about which strategy is
//! responsible.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::state::{ChainEnv, ManagedIngress, ProxyCert, ProxyHolding};
use crate::runtime::{canopy::store as canopy_store, db::Db};
use crate::system::caddy::CertRead;

/// Where the facts a chain depends on come from, for callers that do not hold
/// the daemon's state themselves: the certificate endpoint, and the issuance
/// coordinator that serves it.
pub trait Environment: Send + Sync {
    /// The TLS-terminating ingresses, and the facts a chain depends on for
    /// them. `only` narrows both to the ingresses declaring one hostname.
    ///
    /// `None` when they could not be gathered. That is not an empty answer, and
    /// no caller may act on it as one.
    fn gather(&self, only: Option<&str>) -> Option<(Vec<ManagedIngress>, ChainEnv)>;
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
        let holding = match crate::system::caddy::read_caddy_cert_state(path, hostname) {
            CertRead::Present(view) => ProxyHolding::Holds(ProxyCert {
                not_before: view.not_before,
                not_after: view.not_after,
                issuer_kind: view.issuer_kind,
                issuer: view.issuer,
                self_signed: view.self_signed,
            }),
            CertRead::Absent => ProxyHolding::Nothing,
            CertRead::Unreadable => ProxyHolding::Unreadable,
        };
        proxy.insert(hostname.to_owned(), holding);
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
        offer_live,
        canopy_types,
        proxy,
    })
}

/// What the runtime itself holds for some hostnames.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RuntimeHoldings {
    /// Hostnames a certificate the runtime stores can be served for right now.
    pub served: BTreeSet<String>,
    /// Hostnames with a Canopy order open.
    pub ordering: BTreeSet<String>,
}

/// Which of `hostnames` the runtime serves a certificate for, and which have an
/// order open.
///
/// The proxy's own cache cannot answer the first: a certificate the runtime
/// serves to the proxy is never written there. A hostname the store could not be
/// read for appears in neither set, since not knowing is not the same as holding
/// nothing, and a caller treats it as not yet seen rather than as failed.
// r[impl tls.canopy.serve]
pub fn runtime_holdings(db: &Db, hostnames: &[String]) -> RuntimeHoldings {
    let mut holdings = RuntimeHoldings::default();
    for hostname in hostnames {
        match super::store::find_active_for_hostname(db, hostname) {
            Ok(Some(_)) => {
                holdings.served.insert(hostname.clone());
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(%hostname, error = %e, "cannot read the certificate store");
            }
        }
        match super::store::get_chain_row(db, hostname) {
            Ok(Some(row)) if row.row.open_order_since.is_some() => {
                holdings.ordering.insert(hostname.clone());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(%hostname, error = %e, "cannot read the chain state"),
        }
    }
    holdings
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
                requested_hostname: None,
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
        let holdings = runtime_holdings(&db, &hostnames);
        assert_eq!(
            holdings.served,
            BTreeSet::from(["ok.example.com".to_owned()]),
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
        let holdings = runtime_holdings(&db, &hostnames);
        assert_eq!(
            holdings.ordering,
            BTreeSet::from(["ordering.example.com".to_owned()])
        );
    }
}
