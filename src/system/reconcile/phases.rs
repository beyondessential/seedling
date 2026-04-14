use std::{collections::HashMap, net::Ipv4Addr};

use ipnet::Ipv6Net;

use crate::{
    runtime::{AppPhase, InstanceRegistry},
    system::{
        System, actuator::Actuator, observer::Observer, translate::proxy::build_proxy_config,
        types::DataPlaneRules,
    },
};

use super::{AppSnapshot, RunningPod, pods, proxy, routes, rules, volumes};

pub(super) async fn run_pods_phase(
    observer: &Observer,
    actuator: &Actuator,
    driver: &std::sync::Arc<System>,
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
) -> Vec<(String, pods::PodActuationUpdate)> {
    let futures: Vec<_> = apps
        .iter()
        .map(|app| async move {
            let update =
                pods::observe_and_actuate(observer, actuator, driver, &app.desired, node_prefix)
                    .await;
            (app.name.clone(), update)
        })
        .collect();
    futures_util::future::join_all(futures).await
}

pub(super) async fn run_volumes_phase(
    observer: &Observer,
    actuator: &Actuator,
    apps: &[AppSnapshot],
) -> Vec<(String, volumes::VolumeActuationUpdate)> {
    let futures: Vec<_> = apps
        .iter()
        .filter(|app| app.phase != AppPhase::Uninstalling)
        .map(|app| async move {
            let update = volumes::observe_and_actuate(observer, actuator, &app.desired).await;
            (app.name.clone(), update)
        })
        .collect();
    futures_util::future::join_all(futures).await
}

pub(super) fn compute_routes(
    apps: &[AppSnapshot],
    running_pods_by_app: &HashMap<String, Vec<RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> (
    Vec<crate::system::types::ServiceRoute>,
    Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
) {
    let mut all_routes = Vec::new();
    let mut all_obs = Vec::new();
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let running = running_pods_by_app
            .get(&app.name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let (routes, obs) = match routes::build(
            &app.desired,
            &app.app_def,
            node_prefix,
            registry,
            running,
            &app.name,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "routes: registry lookup failed for app; skipping");
                continue;
            }
        };
        all_routes.extend(routes);
        all_obs.extend(obs);
    }
    (all_routes, all_obs)
}

pub(super) fn compute_nftables_rules(
    apps: &[AppSnapshot],
    running_pods_by_app: &HashMap<String, Vec<RunningPod>>,
    caddy_ip: std::net::Ipv6Addr,
    caddy_v4_addr: Option<Ipv4Addr>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> DataPlaneRules {
    let mut all_ingress = Vec::new();
    let mut all_mounts = Vec::new();
    let mut all_service_dnat = Vec::new();
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let running = running_pods_by_app
            .get(&app.name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        all_ingress.extend(rules::build_ingress_rules(
            &app.app_def,
            caddy_ip,
            caddy_v4_addr,
        ));
        all_mounts.extend(rules::build_mount_rules(running));
        match rules::build_service_dnat_rules(node_prefix, registry, running, &app.name) {
            Ok(dnat) => all_service_dnat.extend(dnat),
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "nftables: registry lookup failed for app; skipping");
                continue;
            }
        }
    }
    DataPlaneRules {
        ingress: all_ingress,
        mounts: all_mounts,
        service_dnat: all_service_dnat,
    }
}

pub(super) struct ProxyBuildResult {
    pub config: crate::system::types::ProxyConfig,
    pub caddy_json: serde_json::Value,
    pub observations: Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
    pub ready_observations: Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
}

pub(super) fn compute_proxy_config(
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> ProxyBuildResult {
    let mut all_pairs = Vec::new();
    let mut all_l4_routes = Vec::new();
    let mut observations = Vec::new();
    let mut ready_observations = Vec::new();
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let build = match proxy::collect(
            &app.app_def,
            &app.desired,
            node_prefix,
            registry,
            &app.name,
        ) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "proxy: registry lookup failed for app; skipping");
                continue;
            }
        };
        all_pairs.extend(build.pairs);
        all_l4_routes.extend(build.l4_routes);
        observations.extend(build.observations);
        ready_observations.extend(build.ready_observations);
    }

    let mut config = build_proxy_config(&all_pairs);
    config.l4_routes = all_l4_routes;
    let caddy_json = super::super::caddy::build_caddy_config(&config);

    ProxyBuildResult {
        config,
        caddy_json,
        observations,
        ready_observations,
    }
}
