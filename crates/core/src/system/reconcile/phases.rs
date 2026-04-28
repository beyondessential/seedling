use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
};

use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use super::{AppSnapshot, RunningPod, pods, proxy, routes, rules, site_proxy, volumes};
use crate::{
    runtime::{
        AppPhase, InstanceRegistry, db::DbHandle,
        external_service_mappings::ExternalServiceSnapshot, identity::InstanceId,
    },
    system::{
        System, actuator::Actuator, observer::Observer, translate::proxy::build_proxy_config,
        types::DataPlaneRules,
    },
};

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
    started_jobs: &HashSet<InstanceId>,
    completed_jobs: &HashSet<InstanceId>,
) -> Vec<(AppName, pods::PodActuationUpdate)> {
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
                started_jobs,
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
) -> Vec<(AppName, volumes::VolumeActuationUpdate)> {
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
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    ext_snapshot: &ExternalServiceSnapshot,
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
            running_pods_by_app,
            ext_snapshot,
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

/// Output of [`compute_nftables_rules`]: the rule set and the per-app set of
/// services that fell back to "anything running" because no healthy backend
/// was available. Callers file `service_degraded` faults from the latter.
pub(super) struct NftablesBuild {
    pub rules: DataPlaneRules,
    pub degraded_services_by_app: HashMap<AppName, std::collections::BTreeSet<String>>,
}

pub(super) fn compute_nftables_rules(
    apps: &[AppSnapshot],
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    caddy_ip: std::net::Ipv6Addr,
    caddy_v4_addr: Option<Ipv4Addr>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    ext_snapshot: &ExternalServiceSnapshot,
) -> NftablesBuild {
    let backends_by_app = rules::collect_backends_by_app(running_pods_by_app);

    let mut all_ingress = Vec::new();
    let mut all_mounts = Vec::new();
    let mut all_service_dnat = Vec::new();
    let mut degraded_by_app: HashMap<AppName, std::collections::BTreeSet<String>> = HashMap::new();
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
        match rules::build_service_dnat_rules(
            node_prefix,
            registry,
            running,
            &app.name,
            ext_snapshot,
            &backends_by_app,
        ) {
            Ok(dnat) => {
                all_service_dnat.extend(dnat.rules);
                if !dnat.degraded_services.is_empty() {
                    degraded_by_app.insert(app.name.clone(), dnat.degraded_services);
                }
            }
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "nftables: registry lookup failed for app; skipping");
                continue;
            }
        }
    }
    NftablesBuild {
        rules: DataPlaneRules {
            ingress: all_ingress,
            mounts: all_mounts,
            service_dnat: all_service_dnat,
        },
        degraded_services_by_app: degraded_by_app,
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
    /// Site-ingress / app-ingress collisions on the same `(hostname, port)`
    /// detected this tick. The reconciler files faults on both parties for
    /// new conflicts and clears them when a `(host, port)` drops out of the
    /// set. Both sides are dropped from the proxy config.
    // r[impl ingress.site.conflict]
    pub conflicts: super::site_proxy::ConflictReport,
    /// Site-ingress attachments that couldn't be resolved this tick (target
    /// app/service missing, unsupported protocol, etc). Each entry produces
    /// a `site_ingress_target_missing` fault under the `_system` app.
    pub unresolved_site_attachments: Vec<super::site_proxy::UnresolvedAttachment>,
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
    site_snapshot: &site_proxy::SiteIngressSnapshot,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    cert_endpoint_url: Option<&str>,
) -> ProxyBuildResult {
    let mut all_pairs: Vec<(
        AppName,
        crate::defs::ingress::IngressDef,
        crate::system::translate::proxy::ServiceUpstream,
    )> = Vec::new();
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
        for (def, upstream) in build.pairs {
            all_pairs.push((app.name.clone(), def, upstream));
        }
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

    // r[impl ingress.site] r[impl ingress.site.attachment]
    let site_data = site_proxy::collect(site_snapshot, apps, node_prefix, registry);

    // r[impl ingress.site.conflict]
    let conflicts = site_proxy::detect_conflicts(&all_pairs, &site_data);
    let surviving_app_pairs =
        site_proxy::drop_conflicting_app_pairs(all_pairs, &conflicts.conflicts);
    let surviving_site_data =
        site_proxy::drop_conflicting_site_data(site_data, &conflicts.conflicts);

    // build_proxy_config wants the (def, upstream) shape — the app name was
    // only carried along so the conflict report could attribute the apps.
    let app_proxy_pairs: Vec<_> = surviving_app_pairs
        .into_iter()
        .map(|(_app, def, up)| (def, up))
        .collect();
    let site_forward_pairs: Vec<_> = surviving_site_data
        .forwards
        .into_iter()
        .map(|(_name, def, up)| (def, up))
        .collect();
    let site_redirect_pairs: Vec<_> = surviving_site_data
        .redirects
        .into_iter()
        .map(|(_name, def, target)| (def, target))
        .collect();

    let mut combined_forwards = app_proxy_pairs;
    combined_forwards.extend(site_forward_pairs);
    let mut config = build_proxy_config(&combined_forwards, &site_redirect_pairs);
    config.l4_routes = all_l4_routes;
    // r[impl ingress.site.attachment]
    config.l4_routes.extend(
        surviving_site_data
            .l4_routes
            .into_iter()
            .map(|entry| entry.route),
    );
    crate::system::translate::proxy::augment_with_warm_certs(&mut config, all_warm);
    config.cert_endpoint_url = cert_endpoint_url.map(str::to_owned);
    let caddy_json = super::super::caddy::build_caddy_config(&config);

    ProxyBuildResult {
        config,
        caddy_json,
        observations,
        ready_observations,
        conflicts,
        unresolved_site_attachments: surviving_site_data.unresolved,
    }
}
