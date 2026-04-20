use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use ipnet::Ipv6Net;
use parking_lot::{Mutex, RwLock};
use seedling_protocol::events::EventSender;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::{app::AppDef, resource::Resource},
    oi::shells::ShellRegistry,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::Db,
        desired::{DesiredState, EffectiveScales, compute, compute_uninstalling},
        identity::InstanceId,
        lifecycle::LifecycleState,
        scaling, stopped,
    },
    system::{
        System, actuator::Actuator, caddy, observer::Observer, resolver, types::DataPlaneRules,
    },
};

mod faults;
mod phases;
mod state;

pub mod pods;
pub mod proxy;
pub mod routes;
pub mod rules;
pub mod volumes;

/// Obs-kind values are a closed set of `&'static str`. Map DB strings back
/// to their static equivalents so they can live in the dedup `HashSet`.
const OBS_KINDS: &[&str] = &[
    "container_created",
    "container_running",
    "container_exited",
    "container_removed",
    "health_check_pass",
    "image_pull_started",
    "stop_sent",
    "volume_created",
    "volume_ready",
    "volume_removed",
    "volume_cleaned_up",
    "network_created",
    "backend_healthy",
    "network_removed",
    "network_cleaned_up",
    "ingress_configured",
    "ingress_ready",
    "ingress_removed",
    "ingress_cleaned_up",
];

/// Load all existing `(instance_id, obs_kind)` pairs from the DB so that
/// observations persisted in a previous session are not re-written with a
/// fresh timestamp on restart.
fn seed_written_obs_arc(db: &Arc<Mutex<Db>>) -> HashSet<(InstanceId, &'static str)> {
    fn intern(s: &str) -> Option<&'static str> {
        OBS_KINDS.iter().find(|&&k| k == s).copied()
    }

    let db = db.lock();
    let mut set = HashSet::new();
    let Ok(mut stmt) = db
        .conn
        .prepare("SELECT DISTINCT instance_id, obs_kind FROM world_observations")
    else {
        return set;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return set;
    };
    for row in rows {
        let Ok((id_hex, kind_str)) = row else {
            continue;
        };
        let Some(kind) = intern(&kind_str) else {
            continue;
        };
        let Ok(n) = u128::from_str_radix(&id_hex, 16) else {
            continue;
        };
        set.insert((InstanceId(uuid::Uuid::from_u128(n)), kind));
    }
    set
}

/// A pod instance observed to be running before this tick's actuations.
///
/// Running pod IPs are collected from the pre-actuation observation.
/// A container started during this tick will not yet have a SLAAC address
/// assigned and will appear in routes only on the next tick. This one-tick
/// lag is intentional and idempotent; the next tick will pick it up.
pub(crate) struct RunningPod {
    #[expect(
        dead_code,
        reason = "set by pods phase; available for future consumers"
    )]
    pub instance: crate::runtime::identity::ResourceInstance,
    pub pod_prefix: Ipv6Net,
    pub pod_ip: std::net::Ipv6Addr,
    /// The Deployment or Job resource definition, kept for binding lookups in
    /// phases 4 and 5.
    pub resource: crate::defs::resource::Resource,
}

/// Point-in-time snapshot of a single app's state, taken at tick start.
struct AppSnapshot {
    name: String,
    desired: DesiredState,
    app_def: AppDef,
    phase: AppPhase,
    phase_handle: Arc<Mutex<AppPhase>>,
    /// Hostnames whose certs should be pre-warmed. Captured from the in-flight
    /// `OperationProgress`, if any.
    // r[impl actuate.ingress.warm-certs]
    warm_cert_hostnames: std::collections::BTreeSet<String>,
}

/// Single global reconciler that processes all installed apps each tick.
pub struct Reconciler {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    observer: Observer,
    actuator: Actuator,
    caddy_admin_client: Arc<AsyncRwLock<reqwest::Client>>,
    caddy_v4_addr: Option<Ipv4Addr>,
    data_dir: PathBuf,
    db: Arc<Mutex<Db>>,
    registry: Arc<dyn InstanceRegistry>,
    app_registry: Arc<RwLock<AppRegistry>>,
    written_obs: HashSet<(InstanceId, &'static str)>,
    // r[impl autonomous.job-terminal.defense]
    /// Job instance IDs known to have completed during this process lifetime.
    /// If these appear running on a subsequent tick they are stopped immediately.
    completed_jobs: HashSet<InstanceId>,
    event_tx: EventSender,
    shells: Arc<ShellRegistry>,
    /// Previous tick's lifecycle states, keyed by (app, instance_id_hex).
    prev_states: BTreeMap<(String, String), LifecycleState>,
    /// Deployments with an active rolling update, keyed by (app, deployment_name).
    /// Set from pod actuation results; read by `compute_effective_scales` to bump
    /// the effective instance count by one during rollouts.
    // r[impl update.rolling.over-provision]
    rolling_updates: HashSet<(String, String)>,
    /// Whether seedling is providing its own NAT64 translator.
    nat64_active: bool,
    /// The resolver container's IPv6 address (set after resolver startup).
    resolver_addr: Option<Ipv6Addr>,
    /// Host-filesystem path of the Caddy data volume. Resolved lazily on the
    /// first tick that needs to inspect Caddy's certificate cache.
    // r[impl observe.ingress.certs]
    caddy_data_path: tokio::sync::OnceCell<PathBuf>,
    /// First time each warm-cert hostname was seen during the current process
    /// lifetime, used to gate `cert_acquisition_failed` faults.
    // r[impl fault.cert-acquisition]
    warm_cert_first_seen: HashMap<String, std::time::Instant>,
}

impl Reconciler {
    #[expect(
        clippy::too_many_arguments,
        reason = "construction requires all subsystem handles"
    )]
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
        caddy_admin_client: Arc<AsyncRwLock<reqwest::Client>>,
        data_dir: PathBuf,
        db: Db,
        app_registry: Arc<RwLock<AppRegistry>>,
        event_tx: EventSender,
        dns_servers: Vec<Ipv6Addr>,
        nat64_active: bool,
        shells: Arc<ShellRegistry>,
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let db = Arc::new(Mutex::new(db));
        let written_obs = seed_written_obs_arc(&db);
        let actuator = Actuator::new(
            Arc::clone(&driver),
            node_prefix,
            Arc::clone(&registry),
            dns_servers,
            Arc::clone(&db),
        );
        Self {
            driver,
            node_prefix,
            observer,
            actuator,
            caddy_admin_client,
            caddy_v4_addr: None,
            data_dir,
            db,
            registry,
            app_registry,
            written_obs,
            completed_jobs: HashSet::new(),
            event_tx,
            prev_states: BTreeMap::new(),
            rolling_updates: HashSet::new(),
            nat64_active,
            resolver_addr: None,
            shells,
            caddy_data_path: tokio::sync::OnceCell::new(),
            warm_cert_first_seen: HashMap::new(),
        }
    }

    // r[desired-state.definition]
    // r[desired-state.steady]
    // r[desired-state.during-operation]
    fn snapshot_all_apps(&self) -> Vec<AppSnapshot> {
        let reg = self.app_registry.read();
        let mut snapshots = Vec::new();
        for (name, status) in reg.list() {
            let entry = match reg.get(&name) {
                Some(e) => e,
                None => continue,
            };
            let phase = entry.phase.lock().clone();
            match phase {
                AppPhase::Installed | AppPhase::Uninstalling => {}
                AppPhase::NotInstalled => continue,
            }
            let _ = status;
            let progress = entry.active_progress.read();
            let app_def_arc = entry.app.def.load_full();
            let app_def = (*app_def_arc).clone();
            // r[impl autonomous.scale]
            let effective_scales = self.compute_effective_scales(&name, &app_def);
            // r[impl resource.stop]
            let stopped_set = {
                let db = self.db.lock();
                stopped::load_stopped(&db, &name).unwrap_or_default()
            };
            let desired = match phase {
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def, &*self.registry),
                AppPhase::NotInstalled => unreachable!(),
                AppPhase::Installed => compute(
                    &name,
                    &app_def,
                    (*progress).as_ref(),
                    &*self.registry,
                    &effective_scales,
                    &stopped_set,
                ),
            };
            let desired = match desired {
                Ok(d) => {
                    self.clear_registry_faults(&name);
                    d
                }
                Err(e) => {
                    error!(app = %name, error = %e, "failed to compute desired state; skipping app this tick");
                    self.file_registry_fault(
                        &name,
                        &format!("failed to compute desired state: {e}"),
                    );
                    continue;
                }
            };
            let warm_cert_hostnames = (*progress)
                .as_ref()
                .map(|p| p.warm_cert_hostnames.clone())
                .unwrap_or_default();
            snapshots.push(AppSnapshot {
                name,
                desired,
                app_def,
                phase,
                phase_handle: Arc::clone(&entry.phase),
                warm_cert_hostnames,
            });
        }
        snapshots
    }

    /// Build the effective-scale map for every Deployment in an app.
    // r[impl update.rolling.over-provision]
    fn compute_effective_scales(&self, app_name: &str, app_def: &AppDef) -> EffectiveScales {
        let db = self.db.lock();
        let mut scales = EffectiveScales::new();
        for (id, resource) in &app_def.resources {
            if let Resource::Deployment(deployment) = resource {
                let dep_def = deployment.def.lock();
                let low = dep_def.scale.start;
                let high = dep_def.scale.end;
                let mut effective =
                    scaling::effective_scale(&db, app_name, id.name.as_str(), low, high)
                        .unwrap_or(low);
                if self
                    .rolling_updates
                    .contains(&(app_name.to_owned(), id.name.as_str().to_owned()))
                {
                    effective = effective.saturating_add(1);
                }
                scales.insert(id.name.as_str().to_owned(), (low, high, effective));
            }
        }
        scales
    }

    // -----------------------------------------------------------------------
    // Tick — main reconciliation loop body
    // -----------------------------------------------------------------------

    // r[reconciliation.loop]
    // r[reconciliation.convergence]
    // r[reconciliation.idempotency]
    // r[fault.non-blocking]
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn tick(&mut self) -> bool {
        self.reconcile_stray_shells().await;

        let apps = self.snapshot_all_apps();

        if apps.is_empty() {
            self.tear_down_idle().await;
            return false;
        }

        // r[impl reconciliation.liveness]
        // --- Concurrent phase: pods ∥ volumes ∥ caddy ∥ resolver ---
        let (pod_updates, vol_observations, caddy_result, resolver_result) = tokio::join!(
            // r[impl autonomous.job-terminal]
            // Pass written_obs so the pod phase can detect completed Jobs: if
            // container_running was previously written for a Job instance but
            // the container is now gone, the job finished — don't restart.
            phases::run_pods_phase(
                &self.observer,
                &self.actuator,
                &self.driver,
                &apps,
                &self.node_prefix,
                &self.written_obs,
                &self.completed_jobs,
            ),
            phases::run_volumes_phase(&self.observer, &self.actuator, &apps),
            tokio::time::timeout(
                Duration::from_secs(10),
                caddy::ensure_caddy_running(
                    &*self.driver.container,
                    &*self.driver.process,
                    &self.node_prefix,
                    &self.data_dir,
                ),
            ),
            // r[impl infra.resolver.startup]
            tokio::time::timeout(
                Duration::from_secs(10),
                resolver::ensure_resolver_running(
                    &*self.driver.container,
                    &*self.driver.process,
                    &self.node_prefix,
                    &self.data_dir,
                    self.nat64_active,
                ),
            ),
        );

        // --- Process pod results ---
        let running_pods_by_app = self.ingest_pod_results(&apps, pod_updates);

        // --- Process volume results ---
        self.ingest_volume_results(&apps, vol_observations);

        // --- Process resolver result ---
        match resolver_result {
            Ok(Ok(addrs)) => {
                self.clear_system_fault("resolver_failed");
                self.resolver_addr = Some(addrs.v6);
            }
            Ok(Err(e)) => {
                error!(error = %e, "resolver startup failed this tick");
                self.file_system_fault("resolver_failed", &format!("resolver startup failed: {e}"));
            }
            Err(_) => {
                warn!("resolver startup timed out this tick");
                self.file_system_fault("resolver_failed", "resolver startup timed out");
            }
        }

        // --- Process caddy result ---
        // r[autonomous.ingress]
        let caddy_addrs = match caddy_result {
            Ok(Ok(addrs)) => match caddy::build_client(&addrs.admin_socket) {
                Ok(client) => {
                    self.clear_system_fault("caddy_failed");
                    *self.caddy_admin_client.write().await = client;
                    self.caddy_v4_addr = addrs.v4;
                    Some(addrs)
                }
                Err(e) => {
                    error!(error = %e, "failed to build caddy admin client");
                    self.file_system_fault(
                        "caddy_failed",
                        &format!("failed to build caddy admin client: {e}"),
                    );
                    None
                }
            },
            Ok(Err(e)) => {
                error!(error = %e, "caddy health check failed; skipping nftables and proxy this tick");
                self.file_system_fault("caddy_failed", &format!("caddy health check failed: {e}"));
                None
            }
            Err(_) => {
                warn!("caddy health check timed out; skipping nftables and proxy this tick");
                self.file_system_fault("caddy_failed", "caddy health check timed out");
                None
            }
        };

        // --- Uninstall phase (sequential, needs running_pods_by_app) ---
        self.run_uninstall_phase(&apps, &running_pods_by_app).await;

        // --- Compute routes (sync) ---
        let (all_routes, route_obs) = phases::compute_routes(
            &apps,
            &running_pods_by_app,
            &self.node_prefix,
            &*self.registry,
        );
        self.persist_obs(route_obs);

        // --- Compute nftables + proxy (sync, gated on caddy) ---
        let nft_and_proxy = caddy_addrs.map(|addrs| {
            let caddy_ip = addrs.v6;

            let dp_rules = phases::compute_nftables_rules(
                &apps,
                &running_pods_by_app,
                caddy_ip,
                self.caddy_v4_addr,
                &self.node_prefix,
                &*self.registry,
            );

            let proxy_build =
                phases::compute_proxy_config(&apps, &self.node_prefix, &*self.registry);

            (dp_rules, proxy_build, caddy_ip)
        });

        // --- Apply phase: concurrent network-plane writes ---
        match nft_and_proxy {
            Some((dp_rules, proxy_build, caddy_ip)) => {
                let phases::ProxyBuildResult {
                    config: proxy_config,
                    caddy_json,
                    observations: proxy_obs,
                    ready_observations: proxy_ready_obs,
                } = proxy_build;
                let has_proxy_config =
                    !proxy_config.virtual_hosts.is_empty() || !proxy_config.l4_routes.is_empty();

                self.persist_obs(proxy_obs);

                let (routes_res, rules_res, proxy_res) = tokio::join!(
                    self.driver.data_plane.apply_routes(&all_routes),
                    self.driver.data_plane.apply_rules(&dp_rules),
                    async {
                        if has_proxy_config {
                            self.driver.proxy.apply_config(&proxy_config).await
                        } else {
                            Ok(())
                        }
                    },
                );

                if let Err(e) = routes_res {
                    error!(error = %e, "routes: apply_routes failed");
                    self.file_system_fault("routes_failed", &format!("apply_routes failed: {e}"));
                } else {
                    self.clear_system_fault("routes_failed");
                }
                if let Err(e) = rules_res {
                    error!(error = %e, "rules: apply_rules failed");
                    self.file_system_fault("nftables_failed", &format!("apply_rules failed: {e}"));
                } else {
                    self.clear_system_fault("nftables_failed");
                }
                match proxy_res {
                    Err(e) => {
                        error!(error = ?e, addr = %caddy_ip, "proxy: apply_config failed");
                        self.file_system_fault(
                            "proxy_failed",
                            &format!("apply_config failed: {e}"),
                        );
                    }
                    Ok(()) if has_proxy_config => {
                        self.clear_system_fault("proxy_failed");
                        self.persist_obs(proxy_ready_obs);

                        // r[impl infra.proxy.upgrade.cache]
                        if let Err(e) = caddy::write_cached_proxy_json(&self.data_dir, &caddy_json)
                        {
                            warn!(
                                error = %e,
                                "proxy: failed to cache proxy config; upgrade continuity may be impaired"
                            );
                        }
                    }
                    Ok(()) => {}
                }

                // r[impl observe.ingress.certs]
                self.observe_warm_certs(&apps).await;
            }
            None => {
                // Caddy unavailable — still apply routes (they don't need caddy).
                if let Err(e) = self.driver.data_plane.apply_routes(&all_routes).await {
                    error!(error = %e, "routes: apply_routes failed");
                    self.file_system_fault("routes_failed", &format!("apply_routes failed: {e}"));
                } else {
                    self.clear_system_fault("routes_failed");
                }
            }
        }

        self.emit_state_changes(&apps);
        self.retire_unscheduled_excess(&apps);

        true
    }

    // -----------------------------------------------------------------------
    // Idle teardown
    // -----------------------------------------------------------------------

    /// Stop and remove any shell containers (labelled `seedling.session=shell`)
    /// that have no corresponding active entry in the shell registry.
    /// This cleans up containers and their pod networks that were left behind
    /// after a seedling restart or an unclean session exit.
    async fn reconcile_stray_shells(&self) {
        use crate::system::types::ContainerFilter;

        let active = self.shells.active_container_names();

        let containers = match self
            .driver
            .container
            .list(ContainerFilter {
                label: Some(("seedling.session", "shell")),
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!("stray shell reconciliation: list containers failed: {e}");
                return;
            }
        };

        for container in containers {
            let display_name = container
                .labels
                .get("seedling.display-name")
                .cloned()
                .unwrap_or_else(|| container.name.clone());

            if !active.contains(&display_name) {
                tracing::info!(container = %display_name, "removing stray shell container");
                let _ = self
                    .driver
                    .container
                    .remove_container(&display_name, true)
                    .await;
                let net_name = format!("seedling-{display_name}");
                let _ = self.driver.container.remove_network(&net_name).await;
            }
        }
    }

    async fn tear_down_idle(&mut self) {
        let empty_rules = DataPlaneRules::default();
        if let Err(e) = self.driver.data_plane.apply_rules(&empty_rules).await {
            error!(error = %e, "idle: flush rules failed");
        }
        if let Err(e) = self.driver.data_plane.apply_routes(&[]).await {
            error!(error = %e, "idle: clear routes failed");
        }
        caddy::teardown_caddy(&*self.driver.container, &*self.driver.process).await;
        // r[impl infra.resolver]
        resolver::teardown_resolver(&*self.driver.container, &*self.driver.process).await;
        self.caddy_v4_addr = None;
        self.resolver_addr = None;
    }

    // -----------------------------------------------------------------------
    // Pod result ingestion
    // -----------------------------------------------------------------------

    fn ingest_pod_results(
        &mut self,
        apps: &[AppSnapshot],
        pod_updates: Vec<(String, pods::PodActuationUpdate)>,
    ) -> HashMap<String, Vec<RunningPod>> {
        let mut running_pods_by_app = HashMap::new();
        // r[impl update.rolling.over-provision]
        // Rebuild rolling_updates from scratch each tick so that completed
        // rollouts are automatically cleared.
        self.rolling_updates.clear();
        for (app_name, pod_update) in pod_updates {
            // r[fault.image-pull]
            self.file_image_pull_faults(&app_name, &pod_update);
            // r[fault.container-start]
            self.file_unit_failure_faults(&app_name, &pod_update);
            // r[fault.external-volume-unmapped]
            self.file_external_volume_faults(&app_name, &pod_update);
            self.file_instance_registry_faults(&app_name, &pod_update);
            self.file_pod_actuation_faults(&app_name, &pod_update);
            for dep_name in &pod_update.rolling_deployments {
                self.rolling_updates
                    .insert((app_name.clone(), dep_name.clone()));
            }
            self.persist_obs(pod_update.observations);
            for instance in &pod_update.started_instances {
                self.written_obs.retain(|(id, _)| *id != instance.id);
                // r[impl autonomous.job-terminal.defense]
                self.completed_jobs.remove(&instance.id);
            }
            // r[impl autonomous.job-terminal.defense]
            self.completed_jobs
                .extend(pod_update.completed_job_instances.iter().copied());
            running_pods_by_app.insert(app_name, pod_update.running);
        }
        let _ = apps;
        running_pods_by_app
    }

    fn ingest_volume_results(
        &mut self,
        apps: &[AppSnapshot],
        vol_updates: Vec<(String, volumes::VolumeActuationUpdate)>,
    ) {
        for (app_name, vol_update) in vol_updates {
            self.file_volume_actuation_faults(&app_name, &vol_update);
            self.persist_obs(vol_update.observations);
        }
        let _ = apps;
    }

    // -----------------------------------------------------------------------
    // Uninstall phase
    // -----------------------------------------------------------------------

    async fn run_uninstall_phase(
        &mut self,
        apps: &[AppSnapshot],
        running_pods_by_app: &HashMap<String, Vec<RunningPod>>,
    ) {
        for app in apps {
            if app.phase != AppPhase::Uninstalling {
                continue;
            }
            let running = running_pods_by_app
                .get(&app.name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            if !running.is_empty() {
                continue;
            }
            let unit_prefix = format!("seedling-{}-", app.name);
            match self.driver.process.list_units(&unit_prefix).await {
                Ok(units) if units.is_empty() => {
                    let db = self.db.lock();
                    transition_phase(
                        &app.phase_handle,
                        AppPhase::NotInstalled,
                        &db,
                        &app.name,
                        "",
                    );
                    if let Err(e) = db.conn.execute(
                        "DELETE FROM resource_instances WHERE app = ?1",
                        rusqlite::params![app.name],
                    ) {
                        warn!(app = %app.name, "failed to clean up resource instances during uninstall: {e}");
                    }
                    let app_instance_ids: HashSet<InstanceId> = app
                        .desired
                        .resources
                        .iter()
                        .map(|dr| dr.instance.id)
                        .collect();
                    self.written_obs
                        .retain(|(id, _)| !app_instance_ids.contains(id));
                    self.completed_jobs
                        .retain(|id| !app_instance_ids.contains(id));
                    tracing::info!(app = %app.name, "uninstall complete");
                }
                Ok(units) => {
                    warn!(
                        app = %app.name,
                        count = units.len(),
                        "uninstall: units still loaded, retrying cleanup"
                    );
                    for unit in &units {
                        let _ = self.driver.process.reset_failed_unit(&unit.name).await;
                        let _ = self.driver.process.stop_unit(&unit.name).await;
                    }
                }
                Err(e) => {
                    warn!(app = %app.name, error = %e, "uninstall: list_units failed");
                }
            }
        }
    }
}
