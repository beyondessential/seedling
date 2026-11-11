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

use std::collections::HashSet;

use seedling_protocol::names::AppName;

use super::{
    ContainerFilter, ContainerRuntime, is_infra_network,
    types::{APP_LABEL, SESSION_LABEL},
};

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
        rt.create_network(name, "fd5e:ed00:0:1::/64".parse().unwrap(), None)
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
}
