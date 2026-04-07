use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
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
    defs::app::{App, AppDef},
    runtime::{
        InstanceRegistry,
        desired::{DesiredState, OperationProgress, compute},
    },
    system::{System, actuator::Actuator, caddy, observer::Observer},
};

pub mod bridge;
pub mod pods;
pub mod proxy;
pub mod routes;
pub mod rules;
pub mod volumes;

// ---------------------------------------------------------------------------
// RunningPod
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Reconciler
// ---------------------------------------------------------------------------

pub struct Reconciler {
    app_name: String,
    app: Arc<Mutex<AppDef>>,
    /// Shared desired-state override. `None` = steady state (all resources
    /// desired at `Ready`). Set to `Some` while a lifecycle operation is
    /// in progress.
    active_progress: Arc<RwLock<Option<OperationProgress>>>,
    observer: Observer,
    actuator: Actuator,
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    registry: Arc<dyn InstanceRegistry>,
    /// Network-name → bridge-interface-name map. Populated at startup via
    /// `list_networks` and maintained during normal operation. Used by the
    /// bridge-address check on every tick.
    bridge_names: HashMap<String, String>,
    /// rtnetlink handle for the bridge-address check.  `None` until
    /// `populate_bridge_names` is called.
    netlink: Option<NetlinkHandle>,
    /// The Caddy admin API address, updated atomically during blue/green
    /// Caddy upgrades. Phases 5 and 6 read this to obtain Caddy's container
    /// IPv6 address; if the address is not yet IPv6, those phases are skipped.
    caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
    /// Data directory passed to `ensure_caddy_running` on every tick so the
    /// reconciler can recover Caddy if it crashes after startup.
    data_dir: PathBuf,
}

impl Reconciler {
    #[expect(
        clippy::too_many_arguments,
        reason = "all parameters are architecturally required for the reconciler"
    )]
    pub fn new(
        app_name: String,
        app: App,
        active_progress: Arc<RwLock<Option<OperationProgress>>>,
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
        bridge_names: HashMap<String, String>,
        caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
        data_dir: PathBuf,
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let actuator = Actuator::new(Arc::clone(&driver), node_prefix, Arc::clone(&registry));
        Self {
            app_name,
            app: app.def,
            active_progress,
            observer,
            actuator,
            driver,
            node_prefix,
            registry,
            bridge_names,
            netlink: None,
            caddy_admin_addr,
            data_dir,
        }
    }

    /// Populate `bridge_names` from existing `seedling-` networks and open
    /// the rtnetlink connection used by the bridge-address check.
    ///
    /// Call this once before entering the reconciliation loop so that pod
    /// bridges that survived a restart are immediately tracked.
    pub async fn populate_bridge_names(&mut self) {
        let (connection, handle, _) = match rtnetlink::new_connection() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    error = %e,
                    "failed to open rtnetlink connection for bridge address checks"
                );
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

    // r[reconciliation.loop]
    // r[reconciliation.convergence]
    // r[reconciliation.idempotency]
    // r[fault.non-blocking]
    /// Execute one reconciliation tick.
    ///
    /// Phases run sequentially. An error in one phase does not skip later
    /// phases. Within each phase, per-resource errors are logged and skipped;
    /// the reconciler continues with the next resource.
    pub async fn tick(&mut self) {
        let (desired, snapshot) = self.snapshot_desired();

        // Observe and actuate Deployments and Jobs.
        let pod_update = pods::observe_and_actuate(
            &self.observer,
            &self.actuator,
            &self.driver,
            &desired,
            &self.node_prefix,
        )
        .await;

        // Maintain bridge_names from this tick's pod actuations.
        for (net_name, bridge_name) in pod_update.new_bridges {
            self.bridge_names.insert(net_name, bridge_name);
        }
        for net_name in pod_update.removed_networks {
            self.bridge_names.remove(&net_name);
        }
        let running_pods = pod_update.running;

        // Bridge ::2 address check — re-add mount endpoint addresses that
        // were lost due to a crash between create_network and the initial
        // rtnetlink assignment.
        if let Some(ref handle) = self.netlink {
            bridge::ensure_mount_endpoints(handle, &self.bridge_names, &desired, &self.node_prefix)
                .await;
        }

        // Observe and actuate Volumes.
        volumes::observe_and_actuate(&self.observer, &self.actuator, &desired).await;

        // DataPlane: service routes.
        routes::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &running_pods,
            &self.app_name,
        )
        .await;

        // Caddy health check — ensure Caddy is running before phases 5 and 6.
        //
        // `ensure_caddy_running` is idempotent: it returns immediately when
        // Caddy is already healthy, and starts/restarts it otherwise. A
        // 10-second timeout prevents a slow startup from stalling the tick;
        // if the timeout fires, phases 5 and 6 are skipped this tick and the
        // next tick will try again.
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
            Ok(Ok(addr)) => {
                *self.caddy_admin_addr.write().await = addr;
            }
            Ok(Err(e)) => {
                error!(error = %e, "caddy health check failed; skipping phases 5 and 6 this tick");
                return;
            }
            Err(_) => {
                warn!("caddy health check timed out; skipping phases 5 and 6 this tick");
                return;
            }
        }

        let caddy_addr = *self.caddy_admin_addr.read().await;
        let caddy_ip = match caddy_addr.ip() {
            IpAddr::V6(ip) => ip,
            _ => {
                warn!("caddy admin address is not yet IPv6; skipping phases 5 and 6 this tick");
                return;
            }
        };

        // DataPlane: nftables rules.
        rules::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &running_pods,
            &self.app_name,
            caddy_ip,
        )
        .await;

        // Proxy config (Caddy).
        proxy::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &self.app_name,
            caddy_addr,
            &self.data_dir,
        )
        .await;
    }

    // r[desired-state.definition]
    // r[desired-state.steady]
    // r[desired-state.during-operation]
    /// Compute the desired state snapshot.
    ///
    /// Acquires the sync locks on `active_progress` and the AppDef
    /// simultaneously, computes the desired state, clones the AppDef
    /// snapshot, then drops both locks before any async work begins.
    fn snapshot_desired(&self) -> (DesiredState, crate::defs::app::AppDef) {
        let progress = self.active_progress.read();
        let app_def = self.app.lock();
        let desired = compute(&self.app_name, &app_def, (*progress).as_ref());
        let snapshot = app_def.clone();
        (desired, snapshot)
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
