use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use rtnetlink::Handle as NetlinkHandle;

use parking_lot::Mutex;

use sha2::{Digest, Sha256};

use ipnet::Ipv6Net;
use parking_lot::RwLock;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::app::AppDef,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::Db,
        desired::{DesiredState, compute, compute_uninstalling},
        history::insert_observation,
        identity::InstanceId,
    },
    system::{
        System, actuator::Actuator, caddy, observer::Observer,
        translate::proxy::build_proxy_config, types::DataPlaneRules,
    },
};

pub mod bridge;
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
    /// Network-name → bridge-interface-name map, maintained across ticks.
    bridge_names: HashMap<String, String>,
    netlink: Option<NetlinkHandle>,
    caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
    caddy_v4_addr: Option<Ipv4Addr>,
    data_dir: PathBuf,
    db: Arc<Mutex<Db>>,
    registry: Arc<dyn InstanceRegistry>,
    app_registry: Arc<RwLock<AppRegistry>>,
    written_obs: HashSet<(InstanceId, &'static str)>,
}

impl Reconciler {
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
        caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
        data_dir: PathBuf,
        db: Db,
        app_registry: Arc<RwLock<AppRegistry>>,
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let actuator = Actuator::new(Arc::clone(&driver), node_prefix, Arc::clone(&registry));
        Self {
            driver,
            node_prefix,
            observer,
            actuator,
            bridge_names: HashMap::new(),
            netlink: None,
            caddy_admin_addr,
            caddy_v4_addr: None,
            data_dir,
            db: Arc::new(Mutex::new(db)),
            registry,
            app_registry,
            written_obs: HashSet::new(),
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn populate_bridge_names(&mut self) {
        let (connection, handle, _) = match rtnetlink::new_connection() {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "failed to open rtnetlink connection for bridge address checks");
                return;
            }
        };
        tokio::spawn(connection);
        self.netlink = Some(handle);

        match self.driver.container.list_networks("seedling-").await {
            Ok(networks) => {
                for net in networks {
                    self.bridge_names.insert(net.name, net.bridge_name);
                }
            }
            Err(e) => {
                error!(error = %e, "failed to list pod networks for bridge-name map");
            }
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
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def),
                AppPhase::NotInstalled => unreachable!(),
                AppPhase::Installed => compute(&name, &app_def, (*progress).as_ref()),
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

    // r[reconciliation.loop]
    // r[reconciliation.convergence]
    // r[reconciliation.idempotency]
    // r[fault.non-blocking]
    #[tracing::instrument(skip_all, level = "debug")]
    /// Returns `true` if there are active apps to reconcile, `false` if the
    /// system is idle (no apps installed). The caller can use this to suspend
    /// the tick interval until the next `tick_notify`.
    pub async fn tick(&mut self) -> bool {
        let apps = self.snapshot_all_apps();

        // When no apps are installed (or all have finished uninstalling),
        // tear down infrastructure so the system is fully clean.
        if apps.is_empty() {
            // Flush nftables rules (empty set).
            let empty_rules = DataPlaneRules::default();
            if let Err(e) = self.driver.data_plane.apply_rules(&empty_rules).await {
                error!(error = %e, "idle: flush rules failed");
            }
            // Remove all service routes.
            if let Err(e) = self.driver.data_plane.apply_routes(&[]).await {
                error!(error = %e, "idle: clear routes failed");
            }
            // Stop Caddy and remove the proxy network.
            caddy::teardown_caddy(&*self.driver.container, &*self.driver.process).await;
            self.caddy_v4_addr = None;
            return false;
        }

        // Per-app phases: pods, uninstall, bridge, volumes
        let mut running_pods_by_app: HashMap<String, Vec<RunningPod>> = HashMap::new();

        for app in &apps {
            let pod_update = pods::observe_and_actuate(
                &self.observer,
                &self.actuator,
                &self.driver,
                &app.desired,
                &self.node_prefix,
            )
            .await;

            for (net_name, bridge_name) in pod_update.new_bridges {
                self.bridge_names.insert(net_name, bridge_name);
            }
            for net_name in pod_update.removed_networks {
                self.bridge_names.remove(&net_name);
            }
            self.persist_obs(pod_update.observations);
            running_pods_by_app.insert(app.name.clone(), pod_update.running);
        }

        for app in &apps {
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
                    // Clear the observation dedup set so that a future
                    // reinstall writes fresh observations instead of being
                    // blocked by stale entries from the previous install
                    // cycle. The cost of re-writing all observations on
                    // the next tick is bounded and harmless.
                    self.written_obs.clear();
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

        for app in &apps {
            if app.phase == AppPhase::Uninstalling {
                continue;
            }
            if let Some(ref handle) = self.netlink {
                bridge::ensure_mount_endpoints(
                    handle,
                    &self.bridge_names,
                    &app.desired,
                    &self.node_prefix,
                )
                .await;
            }
            let vol_obs =
                volumes::observe_and_actuate(&self.observer, &self.actuator, &app.desired).await;
            self.persist_obs(vol_obs);
        }

        // Global: service routes (aggregated across all apps)
        let mut all_routes = Vec::new();
        let mut route_obs = Vec::new();
        for app in &apps {
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
                &self.node_prefix,
                &*self.registry,
                running,
                &app.name,
            );
            all_routes.extend(routes);
            route_obs.extend(obs);
        }
        if let Err(e) = self.driver.data_plane.apply_routes(&all_routes).await {
            error!(error = %e, "routes: apply_routes failed");
        }
        self.persist_obs(route_obs);

        // r[autonomous.ingress]
        match tokio::time::timeout(
            Duration::from_secs(10),
            caddy::ensure_caddy_running(
                &*self.driver.container,
                &*self.driver.process,
                &self.node_prefix,
                &self.data_dir,
            ),
        )
        .await
        {
            Ok(Ok(addrs)) => {
                *self.caddy_admin_addr.write().await = addrs.v6;
                self.caddy_v4_addr = addrs.v4.and_then(|sa| match sa.ip() {
                    IpAddr::V4(ip4) => Some(ip4),
                    _ => None,
                });
            }
            Ok(Err(e)) => {
                error!(error = %e, "caddy health check failed; skipping nftables and proxy this tick");
                return true;
            }
            Err(_) => {
                warn!("caddy health check timed out; skipping nftables and proxy this tick");
                return true;
            }
        }

        let caddy_addr = *self.caddy_admin_addr.read().await;
        let caddy_ip = match caddy_addr.ip() {
            IpAddr::V6(ip) => ip,
            _ => {
                warn!("caddy admin address is not yet IPv6; skipping nftables and proxy this tick");
                return true;
            }
        };

        // Global: nftables rules (aggregated across all apps)
        let mut all_ingress = Vec::new();
        let mut all_mounts = Vec::new();
        let mut all_service_dnat = Vec::new();
        for app in &apps {
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
                self.caddy_v4_addr,
            ));
            all_mounts.extend(rules::build_mount_rules(running));
            all_service_dnat.extend(rules::build_service_dnat_rules(
                &self.node_prefix,
                &*self.registry,
                running,
                &app.name,
            ));
        }
        let dp_rules = DataPlaneRules {
            ingress: all_ingress,
            mounts: all_mounts,
            service_dnat: all_service_dnat,
        };
        if let Err(e) = self.driver.data_plane.apply_rules(&dp_rules).await {
            error!(error = %e, "rules: apply_rules failed");
        }

        // Global: proxy config (aggregated across all apps)
        let mut all_pairs = Vec::new();
        let mut all_l4_routes = Vec::new();
        let mut proxy_obs = Vec::new();
        let mut proxy_ready_obs = Vec::new();
        for app in &apps {
            if app.phase == AppPhase::Uninstalling {
                continue;
            }
            let build = proxy::collect(
                &app.app_def,
                &app.desired,
                &self.node_prefix,
                &*self.registry,
                &app.name,
            );
            all_pairs.extend(build.pairs);
            all_l4_routes.extend(build.l4_routes);
            proxy_obs.extend(build.observations);
            proxy_ready_obs.extend(build.ready_observations);
        }
        self.persist_obs(proxy_obs);

        if !all_pairs.is_empty() || !all_l4_routes.is_empty() {
            let mut config = build_proxy_config(&all_pairs, caddy_addr);
            config.l4_routes = all_l4_routes.clone();
            let caddy_json = caddy::build_caddy_config(&config);

            if let Err(e) = self.driver.proxy.apply_config(&config).await {
                error!(error = ?e, addr = %caddy_addr, "proxy: apply_config failed");
            } else {
                self.persist_obs(proxy_ready_obs);

                // r[impl infra.proxy.upgrade.cache]
                if let Err(e) = caddy::write_cached_proxy_json(&self.data_dir, &caddy_json) {
                    warn!(
                        error = %e,
                        "proxy: failed to cache proxy config; upgrade continuity may be impaired"
                    );
                }
            }
        }

        true
    }

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
