use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use ipnet::Ipv6Net;
use parking_lot::RwLock;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::warn;

use crate::{
    defs::app::App,
    runtime::{
        InstanceRegistry,
        desired::{DesiredState, OperationProgress, compute},
    },
    system::{System, actuator::Actuator, observer::Observer},
};

pub mod phase2_pods;
pub mod phase3_volumes;
pub mod phase4_routes;
pub mod phase5_rules;
pub mod phase6_proxy;

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
    #[expect(dead_code, reason = "used by future Phase 7 and external consumers")]
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
    app: App,
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
    /// `list_networks`; consulted by the future Phase 7 bridge-address check.
    #[expect(dead_code, reason = "populated at startup, consumed by future Phase 7")]
    bridge_names: HashMap<String, String>,
    /// The Caddy admin API address, updated atomically during blue/green
    /// Caddy upgrades. Phases 5 and 6 read this to obtain Caddy's container
    /// IPv6 address; if the address is not yet IPv6, those phases are skipped.
    caddy_admin_addr: Arc<AsyncRwLock<SocketAddr>>,
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
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let actuator = Actuator::new(Arc::clone(&driver), node_prefix, Arc::clone(&registry));
        Self {
            app_name,
            app,
            active_progress,
            observer,
            actuator,
            driver,
            node_prefix,
            registry,
            bridge_names,
            caddy_admin_addr,
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

        // Phase 2 — observe and actuate Deployments and Jobs.
        let running_pods = phase2_pods::observe_and_actuate(
            &self.observer,
            &self.actuator,
            &self.driver,
            &desired,
            &self.node_prefix,
        )
        .await;

        // Phase 3 — observe and actuate Volumes.
        phase3_volumes::observe_and_actuate(&self.observer, &self.actuator, &desired).await;

        // Phase 4 — DataPlane: service routes.
        phase4_routes::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &running_pods,
            &self.app_name,
        )
        .await;

        // Phases 5 and 6 require a valid IPv6 Caddy admin address. The
        // address is not IPv6 until Caddy has started and its address has
        // been written into the shared handle by `ensure_caddy_running`.
        let caddy_addr = *self.caddy_admin_addr.read().await;
        let caddy_ip = match caddy_addr.ip() {
            IpAddr::V6(ip) => ip,
            _ => {
                warn!("caddy admin address is not yet IPv6; skipping phases 5 and 6 this tick");
                return;
            }
        };

        // Phase 5 — DataPlane: nftables rules.
        phase5_rules::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &running_pods,
            &self.app_name,
            caddy_ip,
        )
        .await;

        // Phase 6 — Proxy config (Caddy).
        phase6_proxy::apply(
            &self.driver,
            &snapshot,
            &self.node_prefix,
            &*self.registry,
            &self.app_name,
            caddy_addr,
        )
        .await;
    }

    // r[desired-state.definition]
    // r[desired-state.steady]
    // r[desired-state.during-operation]
    /// Phase 1: compute the desired state snapshot.
    ///
    /// Acquires the sync locks on `active_progress` and the AppDef
    /// simultaneously, computes the desired state, clones the AppDef
    /// snapshot, then drops both locks before any async work begins.
    fn snapshot_desired(&self) -> (DesiredState, crate::defs::app::AppDef) {
        let progress = self.active_progress.read();
        let app_def = self.app.0.lock();
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
/// Layout: `fd5e:XXYY:ZZWW::/48` where `XX YY ZZ WW` are the first four
/// bytes of the machine ID (read as the first 8 hex characters of the file).
pub fn node_prefix_from_machine_id() -> std::io::Result<Ipv6Net> {
    let raw = std::fs::read_to_string("/etc/machine-id")?;
    let hex = raw.trim();
    if hex.len() < 8 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "machine-id too short to derive node prefix",
        ));
    }

    let parse = |s: &str| -> std::io::Result<u8> {
        u8::from_str_radix(s, 16).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid hex in machine-id: {e}"),
            )
        })
    };

    let b0 = parse(&hex[0..2])?;
    let b1 = parse(&hex[2..4])?;
    let b2 = parse(&hex[4..6])?;
    let b3 = parse(&hex[6..8])?;

    let mut octets = [0u8; 16];
    octets[0] = 0xfd;
    octets[1] = 0x5e;
    octets[2] = b0;
    octets[3] = b1;
    octets[4] = b2;
    octets[5] = b3;

    Ok(Ipv6Net::new(std::net::Ipv6Addr::from(octets), 48)
        .expect("48 is a valid IPv6 prefix length"))
}
