use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use ipnet::Ipv6Net;
use parking_lot::{Mutex, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::app::AppDef,
    oi::events::EventSender,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::Db,
        desired::{DesiredState, compute, compute_uninstalling},
        identity::InstanceId,
        lifecycle::LifecycleState,
    },
    system::{System, actuator::Actuator, caddy, observer::Observer, types::DataPlaneRules},
};

mod faults;
mod phases;
mod state;

pub mod pods;
pub mod proxy;
pub mod routes;
pub mod rules;
pub mod volumes;

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
}

/// Single global reconciler that processes all installed apps each tick.
pub struct Reconciler {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    observer: Observer,
    actuator: Actuator,
    caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
    caddy_v4_addr: Option<Ipv4Addr>,
    data_dir: PathBuf,
    db: Arc<Mutex<Db>>,
    registry: Arc<dyn InstanceRegistry>,
    app_registry: Arc<RwLock<AppRegistry>>,
    written_obs: HashSet<(InstanceId, &'static str)>,
    event_tx: EventSender,
    /// Previous tick's lifecycle states, keyed by (app, instance_id_hex).
    prev_states: BTreeMap<(String, String), LifecycleState>,
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
        caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
        data_dir: PathBuf,
        db: Db,
        app_registry: Arc<RwLock<AppRegistry>>,
        event_tx: EventSender,
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let actuator = Actuator::new(Arc::clone(&driver), node_prefix, Arc::clone(&registry));
        Self {
            driver,
            node_prefix,
            observer,
            actuator,
            caddy_admin_addr,
            caddy_v4_addr: None,
            data_dir,
            db: Arc::new(Mutex::new(db)),
            registry,
            app_registry,
            written_obs: HashSet::new(),
            event_tx,
            prev_states: BTreeMap::new(),
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
            let app_def = entry.app.def.lock().clone();
            let desired = match phase {
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def, &*self.registry),
                AppPhase::NotInstalled => unreachable!(),
                AppPhase::Installed => {
                    compute(&name, &app_def, (*progress).as_ref(), &*self.registry)
                }
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
            snapshots.push(AppSnapshot {
                name,
                desired,
                app_def,
                phase,
                phase_handle: Arc::clone(&entry.phase),
            });
        }
        snapshots
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
        let apps = self.snapshot_all_apps();

        if apps.is_empty() {
            self.tear_down_idle().await;
            return false;
        }

        // r[impl reconciliation.liveness]
        // --- Concurrent phase: pods ∥ volumes ∥ caddy ---
        let (pod_updates, vol_observations, caddy_result) = tokio::join!(
            phases::run_pods_phase(
                &self.observer,
                &self.actuator,
                &self.driver,
                &apps,
                &self.node_prefix
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
        );

        // --- Process pod results ---
        let running_pods_by_app = self.ingest_pod_results(&apps, pod_updates);

        // --- Process volume results ---
        self.ingest_volume_results(&apps, vol_observations);

        // --- Process caddy result ---
        // r[autonomous.ingress]
        let caddy_addrs = match caddy_result {
            Ok(Ok(addrs)) => {
                self.clear_system_fault("caddy_failed");
                *self.caddy_admin_addr.write().await = addrs.v6;
                self.caddy_v4_addr = addrs.v4.and_then(|sa| match sa.ip() {
                    IpAddr::V4(ip4) => Some(ip4),
                    _ => None,
                });
                Some(addrs)
            }
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
        let nft_and_proxy = caddy_addrs.and_then(|addrs| {
            let caddy_addr = addrs.v6;
            let caddy_ip = match caddy_addr.ip() {
                IpAddr::V6(ip) => ip,
                _ => {
                    warn!(
                        "caddy admin address is not yet IPv6; skipping nftables and proxy this tick"
                    );
                    return None;
                }
            };

            let dp_rules = phases::compute_nftables_rules(
                &apps,
                &running_pods_by_app,
                caddy_ip,
                self.caddy_v4_addr,
                &self.node_prefix,
                &*self.registry,
            );

            let proxy_build =
                phases::compute_proxy_config(&apps, &self.node_prefix, &*self.registry, caddy_addr);

            Some((dp_rules, proxy_build, caddy_addr))
        });

        // --- Apply phase: concurrent network-plane writes ---
        match nft_and_proxy {
            Some((dp_rules, proxy_build, caddy_addr)) => {
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
                        error!(error = ?e, addr = %caddy_addr, "proxy: apply_config failed");
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

        true
    }

    // -----------------------------------------------------------------------
    // Idle teardown
    // -----------------------------------------------------------------------

    async fn tear_down_idle(&mut self) {
        let empty_rules = DataPlaneRules::default();
        if let Err(e) = self.driver.data_plane.apply_rules(&empty_rules).await {
            error!(error = %e, "idle: flush rules failed");
        }
        if let Err(e) = self.driver.data_plane.apply_routes(&[]).await {
            error!(error = %e, "idle: clear routes failed");
        }
        caddy::teardown_caddy(&*self.driver.container, &*self.driver.process).await;
        self.caddy_v4_addr = None;
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
        for (app_name, pod_update) in pod_updates {
            // r[fault.image-pull]
            self.file_image_pull_faults(&app_name, &pod_update);
            // r[fault.container-start]
            self.file_unit_failure_faults(&app_name, &pod_update);
            self.file_instance_registry_faults(&app_name, &pod_update);
            self.file_pod_actuation_faults(&app_name, &pod_update);
            self.persist_obs(pod_update.observations);
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
