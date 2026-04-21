use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
};

use ipnet::Ipv6Net;

use crate::{
    runtime::{AppPhase, InstanceRegistry, db::DbHandle, identity::InstanceId},
    system::{
        System, actuator::Actuator, observer::Observer, translate::proxy::build_proxy_config,
        types::DataPlaneRules,
    },
};

use super::{AppSnapshot, RunningPod, pods, proxy, routes, rules, volumes};

#[expect(
    clippy::too_many_arguments,
    reason = "phase function fans tick state out to per-app concurrent futures"
)]
pub(super) async fn run_pods_phase(
    observer: &Observer,
    actuator: &Actuator,
    driver: &std::sync::Arc<System>,
    db: &DbHandle,
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
    written_obs: &HashSet<(InstanceId, &'static str)>,
    completed_jobs: &HashSet<InstanceId>,
) -> Vec<(String, pods::PodActuationUpdate)> {
    let futures: Vec<_> = apps
        .iter()
        .map(|app| async move {
            let update = pods::observe_and_actuate(
                observer,
                actuator,
                driver,
                db,
                &app.desired,
                node_prefix,
                written_obs,
                completed_jobs,
            )
            .await;
            (app.name.clone(), update)
        })
        .collect();
    futures_util::future::join_all(futures).await
}

pub(super) async fn run_volumes_phase(
    observer: &Observer,
    actuator: &Actuator,
    db: &DbHandle,
    apps: &[AppSnapshot],
) -> Vec<(String, volumes::VolumeActuationUpdate)> {
    let futures: Vec<_> = apps
        .iter()
        .filter(|app| app.phase != AppPhase::Uninstalling)
        .map(|app| async move {
            let update = volumes::observe_and_actuate(observer, actuator, db, &app.desired).await;
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

/// Resolve all warm-cert ingresses across snapshots into `(instance, hostname)`
/// pairs. Filters out non-TLS ingresses and ingresses already covered by a
/// routed vhost (since Caddy will acquire those via the standard server-block
/// driven path).
// r[impl actuate.ingress.warm-certs]
pub(super) fn warm_cert_targets(
    apps: &[AppSnapshot],
    registry: &dyn InstanceRegistry,
) -> Vec<(crate::runtime::identity::ResourceInstance, String)> {
    let mut out = Vec::new();
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        for ing_name in &app.warm_cert_hostnames {
            let resource_id = crate::defs::resource::ResourceId {
                kind: crate::defs::resource::ResourceKind::Ingress,
                name: std::sync::Arc::new(ing_name.clone()),
            };
            let Some(crate::defs::resource::Resource::Ingress(ing)) =
                app.app_def.resources.get(&resource_id)
            else {
                continue;
            };
            let hostname = {
                let ing_def = ing.def.lock();
                if !ing_def.tls {
                    continue;
                }
                ing_def.hostname.clone()
            };
            match registry.get_or_create_singleton(
                &app.name,
                crate::defs::resource::ResourceKind::Ingress,
                Some(ing_name.as_str()),
            ) {
                Ok(instance) => out.push((instance, hostname)),
                Err(e) => {
                    tracing::warn!(app = %app.name, ingress = %ing_name, error = %e, "warm_certs: registry lookup failed");
                }
            }
        }
    }
    out
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
    let mut all_warm: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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
        // r[impl actuate.ingress.warm-certs]
        // Translate ingress resource *names* in OperationProgress to ingress
        // *hostnames* by looking them up in the AppDef. Ignore non-TLS
        // ingresses; Caddy can't pre-warm a cert without TLS configured.
        for ing_name in &app.warm_cert_hostnames {
            if let Some(crate::defs::resource::Resource::Ingress(ing)) =
                app.app_def
                    .resources
                    .get(&crate::defs::resource::ResourceId {
                        kind: crate::defs::resource::ResourceKind::Ingress,
                        name: std::sync::Arc::new(ing_name.clone()),
                    })
            {
                let ing_def = ing.def.lock();
                if ing_def.tls {
                    all_warm.insert(ing_def.hostname.clone());
                }
            }
        }
    }

    let mut config = build_proxy_config(&all_pairs);
    config.l4_routes = all_l4_routes;
    crate::system::translate::proxy::augment_with_warm_certs(&mut config, all_warm);
    let caddy_json = super::super::caddy::build_caddy_config(&config);

    ProxyBuildResult {
        config,
        caddy_json,
        observations,
        ready_observations,
    }
}
