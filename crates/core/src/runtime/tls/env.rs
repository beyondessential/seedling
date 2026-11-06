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

/// Build the [`ChainEnv`] for `managed` ingresses.
///
/// `offer_live` is whether Canopy access is enabled and a client is offering to
/// carry requests. `caddy_data` is where the proxy's certificate cache lives;
/// `None` when it cannot be located, which leaves every hostname out of
/// [`ChainEnv::proxy`]: not looked at, which is not the same as holding nothing.
pub fn collect(
    db: &Db,
    managed: &[ManagedIngress],
    offer_live: bool,
    caddy_data: Option<&Path>,
) -> rusqlite::Result<ChainEnv> {
    let types_by_app: BTreeMap<String, String> = canopy_store::list_apps(db)?
        .into_iter()
        .map(|a| (a.app, a.canopy_type))
        .collect();

    let mut canopy_types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut hostnames: BTreeSet<&str> = BTreeSet::new();
    for ingress in managed {
        hostnames.insert(ingress.hostname.as_str());
        let entry = canopy_types.entry(ingress.hostname.clone()).or_default();
        if let Some(canopy_type) = types_by_app.get(ingress.app.as_str()) {
            entry.insert(canopy_type.clone());
        }
    }

    let mut proxy = BTreeMap::new();
    if let Some(path) = caddy_data {
        for hostname in hostnames {
            let holding = match crate::system::caddy::read_caddy_cert_state(path, hostname) {
                CertRead::Present(view) => ProxyHolding::Holds(ProxyCert {
                    not_before: view.not_before,
                    not_after: view.not_after,
                }),
                CertRead::Absent => ProxyHolding::Nothing,
                CertRead::Unreadable => ProxyHolding::Unreadable,
            };
            proxy.insert(hostname.to_owned(), holding);
        }
    }

    Ok(ChainEnv {
        offer_live,
        canopy_types,
        proxy,
    })
}
