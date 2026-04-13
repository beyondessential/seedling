use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use parking_lot::Mutex;

use sha2::{Digest, Sha256};

use ipnet::Ipv6Net;
use parking_lot::RwLock;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::app::AppDef,
    oi::events::EventSender,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        barrier::oracle::derive_lifecycle_state,
        db::Db,
        desired::{DesiredState, compute, compute_uninstalling},
        faults,
        history::{find_instances_for_group, insert_observation, query_observations},
        identity::InstanceId,
        lifecycle::LifecycleState,
    },
    system::{
        System, actuator::Actuator, caddy, observer::Observer,
        translate::proxy::build_proxy_config, types::DataPlaneRules,
    },
};

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
            run_pods_phase(
                &self.observer,
                &self.actuator,
                &self.driver,
                &apps,
                &self.node_prefix
            ),
            run_volumes_phase(&self.observer, &self.actuator, &apps),
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
        self.persist_obs(vol_observations);

        // --- Process caddy result ---
        // r[autonomous.ingress]
        let caddy_addrs = match caddy_result {
            Ok(Ok(addrs)) => {
                *self.caddy_admin_addr.write().await = addrs.v6;
                self.caddy_v4_addr = addrs.v4.and_then(|sa| match sa.ip() {
                    IpAddr::V4(ip4) => Some(ip4),
                    _ => None,
                });
                Some(addrs)
            }
            Ok(Err(e)) => {
                error!(error = %e, "caddy health check failed; skipping nftables and proxy this tick");
                None
            }
            Err(_) => {
                warn!("caddy health check timed out; skipping nftables and proxy this tick");
                None
            }
        };

        // --- Uninstall phase (sequential, needs running_pods_by_app) ---
        self.run_uninstall_phase(&apps, &running_pods_by_app).await;

        // --- Compute routes (sync) ---
        let (all_routes, route_obs) = compute_routes(
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

            let dp_rules = compute_nftables_rules(
                &apps,
                &running_pods_by_app,
                caddy_ip,
                self.caddy_v4_addr,
                &self.node_prefix,
                &*self.registry,
            );

            let proxy_build =
                compute_proxy_config(&apps, &self.node_prefix, &*self.registry, caddy_addr);

            Some((dp_rules, proxy_build, caddy_addr))
        });

        // --- Apply phase: concurrent network-plane writes ---
        match nft_and_proxy {
            Some((dp_rules, proxy_build, caddy_addr)) => {
                let ProxyBuildResult {
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
                }
                if let Err(e) = rules_res {
                    error!(error = %e, "rules: apply_rules failed");
                }
                match proxy_res {
                    Err(e) => {
                        error!(error = ?e, addr = %caddy_addr, "proxy: apply_config failed");
                    }
                    Ok(()) if has_proxy_config => {
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
            self.persist_obs(pod_update.observations);
            running_pods_by_app.insert(app_name, pod_update.running);
        }
        let _ = apps;
        running_pods_by_app
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
                    let _ = db.conn.execute(
                        "DELETE FROM resource_instances WHERE app = ?1",
                        rusqlite::params![app.name],
                    );
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

    // -----------------------------------------------------------------------
    // Fault filing
    // -----------------------------------------------------------------------

    fn file_image_pull_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let db = self.db.lock();
        for (instance, reference) in &update.image_pull_failures {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .iter()
                .any(|f| {
                    f.kind == "image_pull_failed" && f.instance_id.as_deref() == Some(&inst_hex)
                });
            if !already_filed {
                let desc = format!("failed to pull image: {reference}");
                let _ = faults::file_fault(
                    &db,
                    app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    "image_pull_failed",
                    &desc,
                );
            }
        }
        for (instance, _reference) in &update.image_pull_successes {
            let inst_hex = instance.id.to_hex();
            let cleared: Vec<_> = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| {
                    f.kind == "image_pull_failed" && f.instance_id.as_deref() == Some(&inst_hex)
                })
                .collect();
            for f in cleared {
                let _ = faults::clear_fault(&db, &f.id, app);
            }
        }
    }

    // r[fault.container-start]
    fn file_unit_failure_faults(&self, app: &str, update: &pods::PodActuationUpdate) {
        let db = self.db.lock();
        for instance in &update.unit_failures {
            let inst_hex = instance.id.to_hex();
            let kind_str = format!("{:?}", instance.kind).to_lowercase();
            let already_filed = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .iter()
                .any(|f| {
                    f.kind == "container_start_failed"
                        && f.instance_id.as_deref() == Some(&inst_hex)
                });
            if !already_filed {
                let desc = format!("unit for {} entered failed state", instance.display_name);
                let _ = faults::file_fault(
                    &db,
                    app,
                    Some(&kind_str),
                    instance.name.as_deref(),
                    Some(&inst_hex),
                    "container_start_failed",
                    &desc,
                );
            }
        }
        for instance in &update.unit_healthy {
            let inst_hex = instance.id.to_hex();
            let cleared: Vec<_> = faults::list_active_faults(&db, Some(app))
                .unwrap_or_default()
                .into_iter()
                .filter(|f| {
                    f.kind == "container_start_failed"
                        && f.instance_id.as_deref() == Some(&inst_hex)
                })
                .collect();
            for f in cleared {
                let _ = faults::clear_fault(&db, &f.id, app);
            }
        }
    }

    // -----------------------------------------------------------------------
    // State-change emission
    // -----------------------------------------------------------------------

    fn emit_state_changes(&mut self, apps: &[AppSnapshot]) {
        let db = self.db.lock();
        let mut new_states = BTreeMap::new();

        for app in apps {
            for dr in &app.desired.resources {
                let kind_str = format!("{:?}", dr.instance.kind).to_lowercase();
                let res_name = dr.instance.name.as_deref().unwrap_or("");
                let inst_hex = dr.instance.id.to_hex();

                let instances = find_instances_for_group(
                    &db,
                    &app.name,
                    dr.instance.kind,
                    dr.instance.name.as_deref(),
                )
                .unwrap_or_default();

                for inst in &instances {
                    let hex = inst.id.to_hex();
                    let obs = query_observations(&db, inst).unwrap_or_default();
                    let state = derive_lifecycle_state(inst, &obs);
                    let key = (app.name.clone(), hex.clone());

                    if let Some(&prev) = self.prev_states.get(&key)
                        && prev != state
                    {
                        crate::oi::events::resource_state_changed(
                            &self.event_tx,
                            &app.name,
                            &kind_str,
                            res_name,
                            &hex,
                            &format!("{state:?}"),
                        );
                    }

                    new_states.insert(key, state);
                }

                if instances.is_empty() {
                    let key = (app.name.clone(), inst_hex);
                    new_states.insert(key, LifecycleState::Pending);
                }
            }
        }

        self.prev_states = new_states;
    }

    // -----------------------------------------------------------------------
    // Observation persistence
    // -----------------------------------------------------------------------

    // r[impl observe.persist]
    fn persist_obs(
        &mut self,
        batch: Vec<(
            crate::runtime::identity::ResourceInstance,
            &'static str,
            serde_json::Value,
        )>,
    ) {
        for (instance, kind, payload) in batch {
            if !self.written_obs.insert((instance.id, kind)) {
                continue;
            }
            let db = self.db.lock();
            if let Err(e) = insert_observation(&db, &instance, kind, &payload) {
                error!(
                    error = %e,
                    instance = %instance.display_name,
                    obs = kind,
                    "reconciler: failed to persist observation"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Concurrent phase: pods (all apps in parallel)
// ---------------------------------------------------------------------------

async fn run_pods_phase(
    observer: &Observer,
    actuator: &Actuator,
    driver: &Arc<System>,
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

// ---------------------------------------------------------------------------
// Concurrent phase: volumes (all non-uninstalling apps in parallel)
// ---------------------------------------------------------------------------

async fn run_volumes_phase(
    observer: &Observer,
    actuator: &Actuator,
    apps: &[AppSnapshot],
) -> Vec<(
    crate::runtime::identity::ResourceInstance,
    &'static str,
    serde_json::Value,
)> {
    let futures: Vec<_> = apps
        .iter()
        .filter(|app| app.phase != AppPhase::Uninstalling)
        .map(|app| volumes::observe_and_actuate(observer, actuator, &app.desired))
        .collect();
    let results = futures_util::future::join_all(futures).await;
    results.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------
// Sync computation: service routes
// ---------------------------------------------------------------------------

fn compute_routes(
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
        let (routes, obs) = routes::build(
            &app.desired,
            &app.app_def,
            node_prefix,
            registry,
            running,
            &app.name,
        );
        all_routes.extend(routes);
        all_obs.extend(obs);
    }
    (all_routes, all_obs)
}

// ---------------------------------------------------------------------------
// Sync computation: nftables rules
// ---------------------------------------------------------------------------

fn compute_nftables_rules(
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
        all_service_dnat.extend(rules::build_service_dnat_rules(
            node_prefix,
            registry,
            running,
            &app.name,
        ));
    }
    DataPlaneRules {
        ingress: all_ingress,
        mounts: all_mounts,
        service_dnat: all_service_dnat,
    }
}

// ---------------------------------------------------------------------------
// Sync computation: proxy config
// ---------------------------------------------------------------------------

struct ProxyBuildResult {
    config: crate::system::types::ProxyConfig,
    caddy_json: serde_json::Value,
    observations: Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
    ready_observations: Vec<(
        crate::runtime::identity::ResourceInstance,
        &'static str,
        serde_json::Value,
    )>,
}

fn compute_proxy_config(
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    caddy_addr: SocketAddr,
) -> ProxyBuildResult {
    let mut all_pairs = Vec::new();
    let mut all_l4_routes = Vec::new();
    let mut observations = Vec::new();
    let mut ready_observations = Vec::new();
    for app in apps {
        if app.phase == AppPhase::Uninstalling {
            continue;
        }
        let build = proxy::collect(&app.app_def, &app.desired, node_prefix, registry, &app.name);
        all_pairs.extend(build.pairs);
        all_l4_routes.extend(build.l4_routes);
        observations.extend(build.observations);
        ready_observations.extend(build.ready_observations);
    }

    let mut config = build_proxy_config(&all_pairs, caddy_addr);
    config.l4_routes = all_l4_routes;
    let caddy_json = caddy::build_caddy_config(&config);

    ProxyBuildResult {
        config,
        caddy_json,
        observations,
        ready_observations,
    }
}

// ---------------------------------------------------------------------------
// Node prefix derivation
// ---------------------------------------------------------------------------

// r[infra.node.prefix]
/// Derive the node's /48 ULA prefix from `/etc/machine-id`.
///
/// The raw machine-id content (whitespace-trimmed) is hashed with SHA-256;
/// the first four bytes of the digest fill octets 2–5 of the prefix:
///
/// ```text
/// fd5e : <hash[0]><hash[1]> : <hash[2]><hash[3]> :: /48
/// ```
///
/// Hashing instead of direct interpretation means the derivation is
/// agnostic to the machine-id format (plain hex, UUID with dashes, etc.).
pub fn node_prefix_from_machine_id() -> std::io::Result<Ipv6Net> {
    let raw = std::fs::read_to_string("/etc/machine-id")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "machine-id is empty",
        ));
    }

    let digest = Sha256::digest(trimmed.as_bytes());

    let mut octets = [0u8; 16];
    octets[0] = 0xfd;
    octets[1] = 0x5e;
    octets[2] = digest[0];
    octets[3] = digest[1];
    octets[4] = digest[2];
    octets[5] = digest[3];

    Ok(Ipv6Net::new(std::net::Ipv6Addr::from(octets), 48)
        .expect("48 is a valid IPv6 prefix length"))
}
