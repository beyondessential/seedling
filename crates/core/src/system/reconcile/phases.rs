use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    net::Ipv4Addr,
};

use ipnet::Ipv6Net;
use seedling_protocol::names::{AppName, SiteServiceName};

use super::{AppSnapshot, Coverage, RunningPod, pods, proxy, routes, rules, site_proxy, volumes};
use crate::{
    runtime::{
        AppPhase, InstanceRegistry,
        db::DbHandle,
        external_service_mappings::ExternalServiceSnapshot,
        identity::InstanceId,
        site_services::resolve::{ResolveCtx, ResolveOutcome, UnroutableReason, resolve_endpoint},
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
    written_obs: &HashSet<(InstanceId, &'static str)>,
    started_jobs: &HashSet<InstanceId>,
    completed_jobs: &HashSet<InstanceId>,
    crash_looped: &HashSet<InstanceId>,
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
                app.app_priority,
                written_obs,
                started_jobs,
                completed_jobs,
                crash_looped,
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
        .map(|app| async move {
            // An uninstalling app's volumes are torn down by its own phase,
            // but its external volumes are held Unscheduled as its desired
            // state has them.
            let update = if app.phase == AppPhase::Uninstalling {
                volumes::VolumeActuationUpdate::external_volumes_only(&app.desired)
            } else {
                volumes::observe_and_actuate(observer, actuator, db, &app.desired).await
            };
            (app.name.clone(), update)
        })
        .collect();
    futures_util::future::join_all(futures).await
}

pub(super) struct RoutesBuild {
    pub routes: Vec<crate::system::types::ServiceRoute>,
    pub observations: Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
    /// `Partial` when an app's routes could not be built, or the site
    /// services and external service mappings could not be read. Their
    /// routes are absent from `routes`, which replaces the host's whole route
    /// set.
    pub coverage: Coverage,
}

pub(super) fn compute_routes(
    apps: &[AppSnapshot],
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    ext_snapshot: Option<&ExternalServiceSnapshot>,
    resolve_ctx: &ResolveCtx<'_>,
) -> RoutesBuild {
    let mut all_routes = Vec::new();
    let mut all_obs = Vec::new();
    let mut dropped = 0usize;
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
            resolve_ctx,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "routes: registry lookup failed for app; skipping");
                dropped += 1;
                continue;
            }
        };
        all_routes.extend(routes);
        all_obs.extend(obs);
    }
    RoutesBuild {
        routes: all_routes,
        observations: all_obs,
        // spec: SITE#unreadable-site-services
        coverage: Coverage::of(dropped).and(Coverage::of_read(ext_snapshot)),
    }
}

/// Per-tick classification of every site-service endpoint into the kinds of
/// faults the reconciler should file.
// spec: SITE#endpoint-addressing
#[derive(Debug, Default, Clone)]
pub(super) struct SiteServiceFaultSet {
    /// Site services with at least one DNS-named endpoint whose name has
    /// failed to resolve past the resolver's failure threshold. Hosts are
    /// stored sorted for deterministic fault descriptions.
    pub unresolvable: BTreeMap<SiteServiceName, Vec<String>>,
    /// Site services with at least one endpoint that cannot be routed from
    /// this host, each paired with why. The reason has to travel with the
    /// host: the two cases need different remedies, and reporting them
    /// alike told operators NAT64 was off when it was on and irrelevant.
    pub unroutable: BTreeMap<SiteServiceName, Vec<(String, UnroutableReason)>>,
}

/// Walk every site service endpoint and bucket the outcomes per service
/// into faultable categories. Hosts that resolve cleanly contribute
/// nothing.
// spec: SITE#endpoint-addressing
pub(super) fn classify_site_service_endpoints(
    snapshot: &ExternalServiceSnapshot,
    resolve_ctx: &ResolveCtx<'_>,
    unresolved_hosts: &HashSet<String>,
) -> SiteServiceFaultSet {
    let mut set = SiteServiceFaultSet::default();
    for (name, endpoints) in &snapshot.site_endpoints {
        let mut unresolvable: BTreeSet<String> = BTreeSet::new();
        let mut unroutable: BTreeSet<(String, UnroutableReason)> = BTreeSet::new();
        for ep in endpoints {
            // spec: SITE#endpoint-addressing
            // Five unanswered lookups in a row make an endpoint unresolvable
            // whether or not answers kept from before still route it; "not
            // yet in the cache" right after startup is not a fault.
            if unresolved_hosts.contains(&ep.remote_host) {
                unresolvable.insert(ep.remote_host.clone());
            }
            if let ResolveOutcome::Unroutable { reason } =
                resolve_endpoint(&ep.remote_host, resolve_ctx)
            {
                unroutable.insert((ep.remote_host.clone(), reason));
            }
        }
        if !unresolvable.is_empty() {
            set.unresolvable
                .insert(name.clone(), unresolvable.into_iter().collect());
        }
        if !unroutable.is_empty() {
            set.unroutable
                .insert(name.clone(), unroutable.into_iter().collect());
        }
    }
    set
}

/// Output of [`compute_nftables_rules`]: the rule set and the per-app set of
/// services that fell back to "anything running" because no healthy backend
/// was available. Callers file `service_degraded` faults from the latter.
pub(super) struct NftablesBuild {
    pub rules: DataPlaneRules,
    pub degraded_services_by_app: HashMap<AppName, std::collections::BTreeSet<String>>,
    /// `Partial` when an app's rules could not be built, or the site
    /// ingresses, or the site services and external service mappings, could
    /// not be read. Their rules are absent from `rules`, which is applied
    /// wholesale.
    pub coverage: Coverage,
}

#[expect(
    clippy::too_many_arguments,
    reason = "phase function fans tick state out to per-app rule builders"
)]
pub(super) fn compute_nftables_rules(
    apps: &[AppSnapshot],
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    caddy_ip: std::net::Ipv6Addr,
    caddy_v4_addr: Option<Ipv4Addr>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    ext_snapshot: Option<&ExternalServiceSnapshot>,
    resolve_ctx: &ResolveCtx<'_>,
    proxy_build: &ProxyBuildResult,
) -> NftablesBuild {
    let backends_by_app = rules::collect_backends_by_app(running_pods_by_app);

    let mut all_ingress = Vec::new();
    let mut all_mounts = Vec::new();
    let mut all_service_dnat = Vec::new();
    let mut degraded_by_app: HashMap<AppName, std::collections::BTreeSet<String>> = HashMap::new();
    let mut dropped = 0usize;
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let running = running_pods_by_app
            .get(&app.name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        all_mounts.extend(rules::build_mount_rules(
            running,
            &app.name,
            ext_snapshot,
            &backends_by_app,
            resolve_ctx,
        ));
        match rules::build_service_dnat_rules(
            node_prefix,
            registry,
            running,
            &app.name,
            ext_snapshot,
            &backends_by_app,
            resolve_ctx,
        ) {
            Ok(dnat) => {
                all_service_dnat.extend(dnat.rules);
                if !dnat.degraded_services.is_empty() {
                    degraded_by_app.insert(app.name.clone(), dnat.degraded_services);
                }
            }
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "nftables: registry lookup failed for app; skipping");
                dropped += 1;
                continue;
            }
        }
    }
    // spec: INFRA#ingress-redirection
    // spec: PRX#shared-ports
    // The ports the proxy is given to serve, and nothing declared that it
    // left out: from the same build, so the two cannot disagree about which
    // claims were served.
    all_ingress.extend(rules::build_ingress_rules(
        &proxy_build.config,
        caddy_ip,
        caddy_v4_addr,
    ));
    NftablesBuild {
        rules: DataPlaneRules {
            ingress: all_ingress,
            mounts: all_mounts,
            service_dnat: all_service_dnat,
        },
        degraded_services_by_app: degraded_by_app,
        // spec: SITE#unreadable-site-ingresses
        // spec: SITE#unreadable-site-services
        // The ingress rules are the proxy build's, which is partial when an
        // app's ingresses could not be built or the site ingresses could not
        // be read.
        coverage: Coverage::of(dropped)
            .and(proxy_build.coverage)
            .and(Coverage::of_read(ext_snapshot)),
    }
}

pub(super) struct ProxyBuildResult {
    pub config: crate::system::types::ProxyConfig,
    /// `Partial` when an app was held back from the tick, an app's ingresses
    /// could not be built, or the site ingresses could not be read. Their
    /// virtual hosts and L4 routes are absent from `config`, which replaces
    /// the proxy's whole configuration.
    pub coverage: Coverage,
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
    /// set. Both sides are dropped from the proxy config. `None` unless
    /// `coverage` is complete: an app or site ingress missing from the build
    /// is absent rather than unclaimed, so no conflict can be decided.
    // spec: SITE#ingress-conflicts
    // spec: SITE#unreadable-site-ingresses
    pub conflicts: Option<super::site_proxy::ConflictReport>,
    /// Site-ingress attachments that couldn't be resolved this tick (target
    /// app/service missing, unsupported protocol, etc). Each entry produces
    /// a `site_ingress_target_missing` fault under the `_system` app. `None`
    /// unless `coverage` is complete: the target of an attachment may be an
    /// app missing from the build.
    pub unresolved_site_attachments: Option<Vec<super::site_proxy::UnresolvedAttachment>>,
    /// The `(app, ingress)` of each DTLS ingress the proxy is given to serve,
    /// whose refused sessions are held as faults. `None` unless `coverage` is
    /// complete: an ingress missing from the build is absent, not removed.
    // spec: PRX#dtls-terminating-ingresses
    pub dtls_ingresses: Option<Vec<(AppName, String)>>,
}

/// The `(instance, hostname)` pairs whose certificates are observed: every
/// TLS-terminating ingress an app's desired state holds, and each ingress an
/// operation's `rt.warm_certs` names.
// spec: OBS#ingresses
// spec: RT#certificate-warming
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
                if !(ing_def.tls || ing_def.dtls) {
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
        for desired in &app.desired.resources {
            if desired.desired != crate::runtime::lifecycle::LifecycleState::Ready {
                continue;
            }
            let crate::defs::resource::Resource::Ingress(ing) = &desired.definition else {
                continue;
            };
            let hostname = {
                let ing_def = ing.def.lock();
                if !(ing_def.tls || ing_def.dtls) {
                    continue;
                }
                ing_def.hostname.clone()
            };
            if !out
                .iter()
                .any(|(instance, _)| instance.id == desired.instance.id)
            {
                out.push((desired.instance.clone(), hostname));
            }
        }
    }
    out
}

/// `app_coverage` says whether `apps` holds every app contributing to the
/// proxy, or the tick held some back.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one of the tick's snapshots the build reads"
)]
pub(super) fn compute_proxy_config(
    apps: &[AppSnapshot],
    app_coverage: Coverage,
    site_snapshot: Option<&site_proxy::SiteIngressSnapshot>,
    running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    cert_endpoint_url: Option<&str>,
    renewal_ratios: &std::collections::BTreeMap<String, f64>,
) -> ProxyBuildResult {
    let mut all_ingresses: Vec<site_proxy::AppIngress> = Vec::new();
    let mut observations = Vec::new();
    let mut ready_observations = Vec::new();
    let mut all_warm: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut dropped = 0usize;
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let empty: Vec<RunningPod> = Vec::new();
        let app_running_pods: &[RunningPod] = running_pods_by_app
            .get(&app.name)
            .map(Vec::as_slice)
            .unwrap_or(&empty);
        let build = match proxy::collect(
            &app.app_def,
            &app.desired,
            node_prefix,
            registry,
            &app.name,
            app_running_pods,
        ) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(app = %app.name, error = %e, "proxy: registry lookup failed for app; skipping");
                dropped += 1;
                continue;
            }
        };
        for (def, serve) in build.ingresses {
            all_ingresses.push(site_proxy::AppIngress {
                app: app.name.clone(),
                def,
                serve,
            });
        }
        // spec: ING#certificates
        all_warm.extend(build.certificate_hostnames);
        observations.extend(build.observations);
        ready_observations.extend(build.ready_observations);
        // spec: RT#certificate-warming
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

    // spec: ING#claims-across-apps
    let install_seqs: HashMap<&AppName, Option<i64>> =
        apps.iter().map(|a| (&a.name, a.install_seq)).collect();
    let (all_ingresses, lost_app_claims) = site_proxy::resolve_app_claims(all_ingresses, |app| {
        install_seqs.get(app).copied().flatten()
    });

    // spec: SITE#unreadable-site-ingresses
    // Without the site ingresses no conflict can be decided and no attachment
    // resolved, so neither is reported, and the build is partial: the config
    // lacks every site ingress, which applied would withdraw them.
    let Some(site_snapshot) = site_snapshot else {
        let served = ServedIngresses::from_app(all_ingresses);
        return ProxyBuildResult {
            config: finish_proxy_config(
                build_proxy_config(&served.http, &[]),
                served.l4,
                served.dtls,
                all_warm,
                cert_endpoint_url,
                renewal_ratios,
            ),
            coverage: Coverage::Partial,
            observations,
            ready_observations,
            conflicts: None,
            unresolved_site_attachments: None,
            dtls_ingresses: None,
        };
    };

    // spec: SITE#site-ingresses, SITE#attachments
    let site_data = site_proxy::collect(
        site_snapshot,
        apps,
        running_pods_by_app,
        node_prefix,
        registry,
    );

    // spec: SITE#ingress-conflicts
    let (site_data, lost_site_claims) = site_proxy::resolve_site_claims(site_data);

    // spec: SITE#ingress-conflicts
    let mut conflicts = site_proxy::detect_conflicts(&all_ingresses, &site_data);
    conflicts.app_claims = lost_app_claims;
    conflicts.site_claims = lost_site_claims;
    let (surviving_app, surviving_site_data) =
        site_proxy::drop_conflicting(all_ingresses, site_data, &conflicts);

    // build_proxy_config wants the (def, upstream) shape — the app name was
    // only carried along so the conflict report could attribute the apps.
    let mut served = ServedIngresses::from_app(surviving_app);
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

    served.http.extend(site_forward_pairs);
    // spec: SITE#attachments
    served.l4.extend(
        surviving_site_data
            .l4_routes
            .into_iter()
            .map(|entry| entry.route),
    );

    // spec: FLT#filing-and-clearing
    // spec: RCN#applying-absolute-state
    // An app held back from the tick, or whose ingresses could not be built,
    // is absent from the claims rather than claiming nothing: its own
    // conflicts would read as resolved, another app would win a port it does
    // not hold, and an attachment forwarding to it would read as having no
    // target. So nothing is concluded about those faults, as for a withheld
    // apply.
    let coverage = Coverage::of(dropped).and(app_coverage);
    let decided = coverage.is_complete();
    ProxyBuildResult {
        config: finish_proxy_config(
            build_proxy_config(&served.http, &site_redirect_pairs),
            served.l4,
            served.dtls,
            all_warm,
            cert_endpoint_url,
            renewal_ratios,
        ),
        coverage,
        observations,
        ready_observations,
        conflicts: decided.then_some(conflicts),
        unresolved_site_attachments: decided.then_some(surviving_site_data.unresolved),
        dtls_ingresses: decided.then_some(served.dtls_owners),
    }
}

/// The app ingresses the proxy serves, by what each is served as.
struct ServedIngresses {
    http: Vec<(
        crate::defs::ingress::IngressDef,
        crate::system::translate::proxy::ServiceUpstream,
    )>,
    l4: Vec<crate::system::types::L4Route>,
    dtls: Vec<crate::system::types::DtlsRoute>,
    /// The app each DTLS route belongs to, with the route's ingress name.
    dtls_owners: Vec<(AppName, String)>,
}

impl ServedIngresses {
    fn from_app(ingresses: Vec<site_proxy::AppIngress>) -> Self {
        let mut served = Self {
            http: Vec::new(),
            l4: Vec::new(),
            dtls: Vec::new(),
            dtls_owners: Vec::new(),
        };
        for ingress in ingresses {
            match ingress.serve {
                site_proxy::AppServe::Http(upstream) => served.http.push((ingress.def, *upstream)),
                site_proxy::AppServe::Tcp(route) => served.l4.push(route),
                site_proxy::AppServe::Dtls(route) => {
                    served
                        .dtls_owners
                        .push((ingress.app.clone(), route.ingress.clone()));
                    served.dtls.push(route);
                }
            }
        }
        served
    }
}

fn finish_proxy_config(
    mut config: crate::system::types::ProxyConfig,
    l4_routes: Vec<crate::system::types::L4Route>,
    dtls_routes: Vec<crate::system::types::DtlsRoute>,
    warm: std::collections::BTreeSet<String>,
    cert_endpoint_url: Option<&str>,
    renewal_ratios: &std::collections::BTreeMap<String, f64>,
) -> crate::system::types::ProxyConfig {
    config.l4_routes = l4_routes;
    config.dtls_routes = dtls_routes;
    crate::system::translate::proxy::augment_with_warm_certs(&mut config, warm);
    config.cert_endpoint_url = cert_endpoint_url.map(str::to_owned);
    config.renewal_ratios = renewal_ratios.clone();
    config
}
