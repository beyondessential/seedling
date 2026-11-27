//! Workload containers and pod networks a previous run left behind.
//!
//! A pod network can outlive its container (podman `--rm` removed the
//! container before the reconciler cleaned up the network, or the daemon was
//! killed mid-operation). Networks carry no app label, so they are recovered
//! by correlating the name `seedling-<container>` against the containers that
//! still exist.
//!
//! That correlation is only as good as the container list it is given, which is
//! why the orphaned containers are removed first: a container of an app that is
//! no longer registered still exists until it is removed, and judging networks
//! before then classes its network as live and leaves nothing to revisit it.
//!
//! A network whose bridge is not named as the runtime names it now, as one
//! created before it named bridges is, goes too, so that it is created again
//! under that name.

use std::collections::{HashMap, HashSet};

use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use super::{
    ContainerFilter, ContainerRuntime, PROXY_BRIDGE, RESOLVER_BRIDGE,
    caddy::PROXY_NETWORK,
    is_infra_network, pod_bridge_name,
    resolver::RESOLVER_NETWORK,
    types::{APP_LABEL, SESSION_LABEL},
};

/// Remove each network whose bridge is not named as the runtime would name it,
/// with the containers on it, so that it is created again under that name.
///
/// `recorded` is the prefix recorded for each pod network, `None` when the
/// records could not be read. A pod network with no readable record is kept,
/// as what its bridge should be named cannot be told.
pub async fn replace_misnamed_networks(
    container: &dyn ContainerRuntime,
    recorded: Option<&HashMap<String, Ipv6Net>>,
) {
    let nets = match container.list_networks("seedling-").await {
        Ok(nets) => nets,
        Err(e) => {
            tracing::warn!(error = %e, "could not list networks; leaving their bridge names unchecked");
            return;
        }
    };
    for net in nets {
        let chosen = match net.name.as_str() {
            PROXY_NETWORK => PROXY_BRIDGE.to_owned(),
            RESOLVER_NETWORK => RESOLVER_BRIDGE.to_owned(),
            name => match recorded.and_then(|r| r.get(name)) {
                Some(prefix) => pod_bridge_name(prefix),
                None => continue,
            },
        };
        if net.bridge_name == chosen {
            continue;
        }
        tracing::warn!(
            network = %net.name,
            bridge = %net.bridge_name,
            %chosen,
            "network's bridge is not named as the runtime names it; replacing the network"
        );
        remove_network_with_containers(container, &net.name).await;
    }
}

/// Remove the network `name` and every container connected to it, which the
/// network cannot be removed without. A container that cannot be listed or
/// removed leaves the network in place.
async fn remove_network_with_containers(container: &dyn ContainerRuntime, name: &str) {
    let on_it = match container
        .list(ContainerFilter {
            network: Some(name),
            ..Default::default()
        })
        .await
    {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(network = %name, error = %e, "could not list the network's containers; leaving it in place");
            return;
        }
    };
    for c in on_it {
        tracing::info!(network = %name, container = %c.name, "removing container to replace its network");
        if let Err(e) = container.remove_container(&c.name, true).await {
            tracing::warn!(network = %name, container = %c.name, error = %e, "failed to remove container; leaving its network in place");
            return;
        }
    }
    if let Err(e) = container.remove_network(name).await {
        tracing::warn!(network = %name, error = %e, "failed to remove network");
    }
}

/// Remove containers of unregistered apps, then the pod networks that nothing
/// live is using.
// spec: INFRA#pod-network-cleanup
pub async fn sweep_orphaned_workloads(
    container: &dyn ContainerRuntime,
    known_apps: &HashSet<AppName>,
) {
    remove_orphaned_containers(container, known_apps).await;
    remove_orphaned_networks(container).await;
}

/// Free the prefix of each pod network that no longer exists.
///
/// A network removed while the daemon was not running leaves its allocation
/// held. A listing that cannot be read frees nothing: a network that could not
/// be seen is not one that is gone.
// spec: INFRA#pod-networks
pub async fn forget_removed_pod_networks(
    container: &dyn ContainerRuntime,
    db: &crate::runtime::db::DbHandle,
) {
    let existing: Vec<String> = match container.list_networks("seedling-").await {
        Ok(nets) => nets.into_iter().map(|n| n.name).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "could not list pod networks; keeping every recorded prefix");
            return;
        }
    };
    if let Err(e) = db.call(move |db| crate::runtime::pod_networks::retain_only(db, &existing)) {
        tracing::warn!(error = %e, "could not free the prefixes of removed pod networks");
    }
}

async fn remove_orphaned_containers(
    container: &dyn ContainerRuntime,
    known_apps: &HashSet<AppName>,
) {
    let found = match container
        .list(ContainerFilter {
            label_key: Some(APP_LABEL),
            ..Default::default()
        })
        .await
    {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "could not list seedling containers; leaving orphaned containers in place");
            return;
        }
    };

    let orphans: Vec<_> = found
        .iter()
        .filter(|c| {
            c.labels
                .get(APP_LABEL)
                .is_none_or(|app| !known_apps.contains(app.as_str()))
        })
        .collect();
    if orphans.is_empty() {
        return;
    }

    tracing::warn!(
        count = orphans.len(),
        "found orphaned seedling containers; removing"
    );
    for c in orphans {
        tracing::info!(
            container = %c.name,
            app = c.labels.get(APP_LABEL).map(String::as_str).unwrap_or("?"),
            "removing orphaned container"
        );
        if let Err(e) = container.remove_container(&c.name, true).await {
            tracing::warn!(container = %c.name, error = %e, "failed to remove orphaned container");
        }
    }
}

/// Containers that exist now, by either of our two labels: `seedling.app`
/// (reconciler-spawned Jobs and Deployments) and `seedling.session`
/// (operator-opened shell sessions, including volume shells that carry no app
/// label). `None` when either listing failed, since a partial set would class
/// the networks of the containers it missed as orphans.
async fn live_container_names(container: &dyn ContainerRuntime) -> Option<HashSet<String>> {
    let mut names = HashSet::new();
    for label in [APP_LABEL, SESSION_LABEL] {
        match container
            .list(ContainerFilter {
                label_key: Some(label),
                ..Default::default()
            })
            .await
        {
            Ok(found) => names.extend(found.into_iter().map(|c| c.name)),
            Err(e) => {
                tracing::warn!(%label, error = %e, "could not list containers; leaving pod networks in place");
                return None;
            }
        }
    }
    Some(names)
}

/// Remove `seedling-*` pod networks whose container is gone.
///
/// The infra networks are excluded: they are managed by their own subsystem's
/// `ensure_*_running` path, and are recreated on the first reconciler tick
/// after this runs, so name correlation would take them for orphans.
async fn remove_orphaned_networks(container: &dyn ContainerRuntime) {
    let nets = match container.list_networks("seedling-").await {
        Ok(nets) => nets,
        Err(e) => {
            tracing::warn!(error = %e, "could not list pod networks; skipping orphan scan");
            return;
        }
    };
    let Some(live) = live_container_names(container).await else {
        return;
    };

    let orphans: Vec<String> = nets
        .into_iter()
        .map(|n| n.name)
        .filter(|name| !is_infra_network(name))
        .filter(|name| {
            // The network is "seedling-<container name>".
            let display = name.strip_prefix("seedling-").unwrap_or(name);
            !live.contains(display)
        })
        .collect();
    if orphans.is_empty() {
        return;
    }

    tracing::warn!(
        count = orphans.len(),
        "found orphaned seedling pod networks; removing"
    );
    for name in &orphans {
        tracing::info!(network = %name, "removing orphaned pod network");
        if let Err(e) = container.remove_network(name).await {
            tracing::warn!(network = %name, error = %e, "failed to remove orphaned pod network");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::stub::StubContainerRuntime;

    fn runtime() -> StubContainerRuntime {
        StubContainerRuntime::new(std::env::temp_dir().join("seedling-startup-cleanup-tests"))
    }

    async fn network(rt: &StubContainerRuntime, name: &str) {
        rt.create_network(
            name,
            "sdp-0001",
            "fd5e:ed00:0:1::/64".parse().unwrap(),
            None,
        )
        .await
        .expect("create network");
    }

    async fn network_names(rt: &StubContainerRuntime) -> Vec<String> {
        let mut names: Vec<String> = rt
            .list_networks("seedling-")
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.name)
            .collect();
        names.sort();
        names
    }

    // spec: INFRA#pod-network-cleanup
    // The network scan used to run before the container scan, so a deregistered
    // app's container still existed when its network was judged, and the
    // network was kept for good.
    #[tokio::test]
    async fn a_deregistered_apps_network_goes_with_its_container() {
        let rt = runtime();
        rt.seed_container("gone-web", &[(APP_LABEL, "gone")]);
        rt.seed_container("kept-web", &[(APP_LABEL, "kept")]);
        network(&rt, "seedling-gone-web").await;
        network(&rt, "seedling-kept-web").await;

        let known = HashSet::from([AppName::new("kept").unwrap()]);
        sweep_orphaned_workloads(&rt, &known).await;

        assert_eq!(network_names(&rt).await, ["seedling-kept-web"]);
        let left = rt
            .list(ContainerFilter {
                label_key: Some(APP_LABEL),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "kept-web");
    }

    // spec: INFRA#pod-network-cleanup
    #[tokio::test]
    async fn a_live_operator_session_keeps_its_network() {
        let rt = runtime();
        rt.seed_container("shell-1", &[(SESSION_LABEL, "shell")]);
        network(&rt, "seedling-shell-1").await;
        network(&rt, "seedling-nothing-uses-this").await;

        sweep_orphaned_workloads(&rt, &HashSet::new()).await;

        assert_eq!(network_names(&rt).await, ["seedling-shell-1"]);
    }

    // spec: INFRA#pod-network-cleanup
    #[tokio::test]
    async fn infra_networks_are_never_orphans() {
        let rt = runtime();
        network(&rt, crate::system::caddy::PROXY_NETWORK).await;
        network(&rt, crate::system::resolver::RESOLVER_NETWORK).await;

        sweep_orphaned_workloads(&rt, &HashSet::new()).await;

        assert_eq!(network_names(&rt).await.len(), 2);
    }

    // spec: INFRA#pod-networks
    #[tokio::test]
    async fn prefixes_of_networks_that_are_gone_are_freed_and_an_unreadable_listing_frees_none() {
        let rt = runtime();
        let db = crate::runtime::db::DbHandle::open_in_memory().unwrap();
        db.call(|db| {
            crate::runtime::pod_networks::allocate(db, "seedling-kept", 4).unwrap();
            crate::runtime::pod_networks::allocate(db, "seedling-gone", 4).unwrap();
        });
        network(&rt, "seedling-kept").await;

        forget_removed_pod_networks(&rt, &db).await;
        let held = |name: &'static str| {
            db.call(move |db| {
                crate::runtime::pod_networks::recorded(db, name)
                    .unwrap()
                    .is_some()
            })
        };
        assert!(held("seedling-kept"));
        assert!(!held("seedling-gone"));

        // With the runtime unreachable nothing can be said to be gone.
        let down = crate::system::unavailable::UnavailableContainerRuntime;
        forget_removed_pod_networks(&down, &db).await;
        assert!(held("seedling-kept"));
    }

    async fn containers(rt: &StubContainerRuntime) -> Vec<String> {
        let mut names: Vec<String> = rt
            .list(ContainerFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        names.sort();
        names
    }

    fn recorded(entries: &[(&str, &str)]) -> HashMap<String, Ipv6Net> {
        entries
            .iter()
            .map(|(name, prefix)| ((*name).to_owned(), prefix.parse().unwrap()))
            .collect()
    }

    #[tokio::test]
    async fn a_pod_network_with_another_bridge_name_goes_with_its_containers() {
        let rt = runtime();
        rt.seed_network("seedling-old-web", "podman3");
        rt.seed_container_on("old-web", "seedling-old-web", &[(APP_LABEL, "old")]);
        rt.seed_network("seedling-new-web", "sdp-0401");
        rt.seed_container_on("new-web", "seedling-new-web", &[(APP_LABEL, "new")]);
        let recorded = recorded(&[
            ("seedling-old-web", "fd5e:ed00:0:0400::/64"),
            ("seedling-new-web", "fd5e:ed00:0:0401::/64"),
        ]);

        replace_misnamed_networks(&rt, Some(&recorded)).await;

        assert_eq!(network_names(&rt).await, ["seedling-new-web"]);
        assert_eq!(containers(&rt).await, ["new-web"]);
    }

    #[tokio::test]
    async fn infrastructure_networks_with_other_bridge_names_are_replaced() {
        let rt = runtime();
        rt.seed_network(PROXY_NETWORK, "podman1");
        rt.seed_container_on("seedling-caddy-blue", PROXY_NETWORK, &[]);
        rt.seed_network(RESOLVER_NETWORK, "podman2");
        rt.seed_container_on("seedling-resolver-blue", RESOLVER_NETWORK, &[]);

        replace_misnamed_networks(&rt, Some(&HashMap::new())).await;

        assert!(network_names(&rt).await.is_empty());
        assert!(containers(&rt).await.is_empty());
    }

    #[tokio::test]
    async fn infrastructure_networks_named_as_chosen_are_kept() {
        let rt = runtime();
        rt.seed_network(PROXY_NETWORK, PROXY_BRIDGE);
        rt.seed_container_on("seedling-caddy-blue", PROXY_NETWORK, &[]);
        rt.seed_network(RESOLVER_NETWORK, RESOLVER_BRIDGE);

        replace_misnamed_networks(&rt, Some(&HashMap::new())).await;

        assert_eq!(network_names(&rt).await.len(), 2);
        assert_eq!(containers(&rt).await, ["seedling-caddy-blue"]);
    }

    // A network with no record has no chosen bridge name, so its own name is
    // no mismatch.
    #[tokio::test]
    async fn a_pod_network_with_no_readable_record_is_kept() {
        let rt = runtime();
        rt.seed_network("seedling-unrecorded", "podman3");
        rt.seed_container_on("unrecorded", "seedling-unrecorded", &[(APP_LABEL, "a")]);

        replace_misnamed_networks(&rt, Some(&HashMap::new())).await;
        replace_misnamed_networks(&rt, None).await;

        assert_eq!(network_names(&rt).await, ["seedling-unrecorded"]);
        assert_eq!(containers(&rt).await, ["unrecorded"]);
    }

    #[tokio::test]
    async fn an_unreadable_network_listing_removes_nothing() {
        let rt = runtime();
        rt.seed_network(PROXY_NETWORK, "podman1");
        rt.seed_container_on("seedling-caddy-blue", PROXY_NETWORK, &[]);
        rt.refuse_network_listing(true);

        replace_misnamed_networks(&rt, Some(&HashMap::new())).await;

        rt.refuse_network_listing(false);
        assert_eq!(network_names(&rt).await, [PROXY_NETWORK]);
        assert_eq!(containers(&rt).await, ["seedling-caddy-blue"]);
    }
}
