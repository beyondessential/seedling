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
use seedling_protocol::names::AppName;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::{app::AppDef, resource::Resource},
    oi::shells::ShellRegistry,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::DbHandle,
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
mod images;
mod phases;
mod site_proxy;
mod state;

/// Enumerate hostnames from the site-ingress snapshot that need TLS this
/// tick. A hostname is only emitted when its parent ingress is non-stale,
/// declares a non-`None` TLS provider, *and* carries at least one
/// attachment — there's no point asking the issuance coordinator (and,
/// for tailnet hostnames, tailscaled) to mint a cert for a hostname
/// nothing is going to serve traffic on yet.
// r[impl ingress.site.tailscale]
fn site_ingress_tls_hostnames(snapshot: &site_proxy::SiteIngressSnapshot) -> Vec<String> {
    let mut out = Vec::new();
    for ing in &snapshot.ingresses {
        if ing.stale {
            continue;
        }
        if matches!(
            ing.tls_provider,
            crate::runtime::site_ingresses::TlsProvider::None
        ) {
            continue;
        }
        let has_attachment = snapshot
            .attachments
            .iter()
            .any(|a| a.site_ingress == ing.name);
        if !has_attachment {
            continue;
        }
        if !out.iter().any(|h: &String| h == &ing.hostname) {
            out.push(ing.hostname.clone());
        }
    }
    out
}

#[cfg(test)]
mod site_ingress_tls_hostname_tests {
    use super::*;
    use crate::runtime::site_ingress_attachments::{
        AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
    };
    use crate::runtime::site_ingresses::{
        DiscoveryProvider, SiteIngressDef, SiteIngressSource, TlsProvider,
    };
    use seedling_protocol::names::{AppName, AppServiceName, SiteIngressName};

    fn manual(name: &str, hostname: &str, tls: TlsProvider) -> SiteIngressDef {
        SiteIngressDef {
            name: SiteIngressName::new(name).unwrap(),
            hostname: hostname.into(),
            description: None,
            source: SiteIngressSource::Manual,
            tls_provider: tls,
            stale: false,
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    fn discovered(name: &str, hostname: &str, tls: TlsProvider) -> SiteIngressDef {
        SiteIngressDef {
            name: SiteIngressName::new(name).unwrap(),
            hostname: hostname.into(),
            description: None,
            source: SiteIngressSource::Discovered {
                provider: DiscoveryProvider::Tailscale,
                key: "n-1".into(),
            },
            tls_provider: tls,
            stale: false,
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    fn forward(name: &str, port: u16) -> SiteIngressAttachment {
        SiteIngressAttachment {
            site_ingress: SiteIngressName::new(name).unwrap(),
            port,
            protocol: AttachmentProtocol::Http,
            target: AttachmentTarget::Forward {
                app: AppName::new("web").unwrap(),
                service: AppServiceName::new("api").unwrap(),
            },
            created_at: "2026-04-28T00:00:00Z".into(),
        }
    }

    #[test]
    fn skips_unattached_ingress() {
        let snap = site_proxy::SiteIngressSnapshot {
            ingresses: vec![discovered(
                "tailscale",
                "host.ts.net",
                TlsProvider::Tailscale,
            )],
            attachments: vec![],
        };
        assert!(site_ingress_tls_hostnames(&snap).is_empty());
    }

    #[test]
    fn includes_attached_ingress() {
        let snap = site_proxy::SiteIngressSnapshot {
            ingresses: vec![discovered(
                "tailscale",
                "host.ts.net",
                TlsProvider::Tailscale,
            )],
            attachments: vec![forward("tailscale", 443)],
        };
        assert_eq!(
            site_ingress_tls_hostnames(&snap),
            vec!["host.ts.net".to_owned()]
        );
    }

    #[test]
    fn skips_stale() {
        let mut ing = discovered("tailscale", "host.ts.net", TlsProvider::Tailscale);
        ing.stale = true;
        let snap = site_proxy::SiteIngressSnapshot {
            ingresses: vec![ing],
            attachments: vec![forward("tailscale", 443)],
        };
        assert!(site_ingress_tls_hostnames(&snap).is_empty());
    }

    #[test]
    fn skips_no_tls() {
        let snap = site_proxy::SiteIngressSnapshot {
            ingresses: vec![manual("plain", "no-tls.example.com", TlsProvider::None)],
            attachments: vec![forward("plain", 80)],
        };
        assert!(site_ingress_tls_hostnames(&snap).is_empty());
    }
}

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
    "unit_failed",
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
fn seed_written_obs(db: &DbHandle) -> HashSet<(InstanceId, &'static str)> {
    fn intern(s: &str) -> Option<&'static str> {
        OBS_KINDS.iter().find(|&&k| k == s).copied()
    }

    db.call(|db| {
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
    })
}

/// A pod instance observed to be running before this tick's actuations.
///
/// Running pod IPs are collected from the pre-actuation observation.
/// A container started during this tick will not yet have a SLAAC address
/// assigned and will appear in routes only on the next tick. This one-tick
/// lag is intentional and idempotent; the next tick will pick it up.
pub(crate) struct RunningPod {
    pub instance: crate::runtime::identity::ResourceInstance,
    pub pod_prefix: Ipv6Net,
    pub pod_ip: std::net::Ipv6Addr,
    /// The Deployment or Job resource definition, kept for binding lookups in
    /// phases 4 and 5.
    pub resource: crate::defs::resource::Resource,
    /// True if podman last reported the container as healthy, or if no
    /// healthcheck is declared (running implies healthy in that case). Used by
    /// the routing layer to prefer healthy backends in the service pool.
    /// r[impl lifecycle.service.routing-pool]
    pub observed_healthy: bool,
}

/// Point-in-time snapshot of a single app's state, taken at tick start.
struct AppSnapshot {
    name: AppName,
    desired: DesiredState,
    app_def: AppDef,
    phase: AppPhase,
    phase_handle: Arc<Mutex<AppPhase>>,
    /// Hostnames whose certs should be pre-warmed. Captured from the in-flight
    /// `OperationProgress`, if any.
    // r[impl actuate.ingress.warm-certs]
    warm_cert_hostnames: std::collections::BTreeSet<String>,
    /// Generation number of the app definition this snapshot was taken from.
    /// Used to detect operator-pushed updates that should reset transient
    /// per-deployment state such as the replace-loop guard.
    // r[impl autonomous.healthcheck-replace.guard]
    current_generation: u64,
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
    db: DbHandle,
    registry: Arc<dyn InstanceRegistry>,
    app_registry: Arc<RwLock<AppRegistry>>,
    written_obs: HashSet<(InstanceId, &'static str)>,
    /// Job instance IDs the reconciler has asked systemd to start during this
    /// process lifetime. Acts as a "previously ran" indicator for the job
    /// terminal-detection logic when the job completed faster than the
    /// observer's poll interval (so no `container_running` observation was
    /// ever recorded). Cleared when the instance is replaced/retired.
    started_jobs: HashSet<InstanceId>,
    // r[impl autonomous.job-terminal.defense]
    /// Job instance IDs known to have completed during this process lifetime.
    /// If these appear running on a subsequent tick they are stopped immediately.
    completed_jobs: HashSet<InstanceId>,
    event_tx: EventSender,
    shells: Arc<ShellRegistry>,
    /// Previous tick's lifecycle states, keyed by (app, instance_id_hex).
    prev_states: BTreeMap<(AppName, String), LifecycleState>,
    /// Deployments with an active rolling update, keyed by (app, deployment_name).
    /// Set from pod actuation results; read by `compute_effective_scales` to bump
    /// the effective instance count by one during rollouts.
    // r[impl update.rolling.over-provision]
    rolling_updates: HashSet<(AppName, String)>,
    /// Deployments needing a healthcheck-driven replacement, keyed by
    /// (app, deployment_name). Populated each tick from observed pod health:
    /// any deployment with `on_failure: replace` whose healthy backend count
    /// is below its target gets bumped by one effective instance, so
    /// `ensure_scaled_group` provisions a fresh replacement alongside the
    /// existing (unhealthy) one.
    // r[impl autonomous.healthcheck-replace]
    unhealthy_replace_deployments: HashSet<(AppName, String)>,
    /// Deployments whose latest replacement attempt itself failed to become
    /// healthy. While present, the runtime suppresses further bumps
    /// (`autonomous.healthcheck-replace.guard`). Cleared when the AppDef
    /// generation for the app changes (operator-pushed update).
    // r[impl autonomous.healthcheck-replace.guard]
    replace_failed: HashSet<(AppName, String)>,
    /// Last `current_generation` we saw for each app; a higher value on a
    /// subsequent tick triggers a reset of `replace_failed` for that app.
    // r[impl autonomous.healthcheck-replace.guard]
    last_seen_generation: HashMap<AppName, u64>,
    /// Whether seedling is providing its own NAT64 translator.
    nat64_active: bool,
    /// Whether the jool translator instance is currently installed on
    /// this host. The daemon sets up NAT64 at startup when required,
    /// and the reconciler tears it down on transition to idle and
    /// re-installs it on the transition back to non-idle.
    // r[impl infra.nat64.translator.lifecycle]
    nat64_installed: bool,
    /// Whether the DNS64 plugin should translate names that already
    /// have a real AAAA record (emits `translate_all` into the
    /// Corefile). Set when NAT64 is seedling-managed and the host has
    /// no IPv6 egress — forcing every flow through the translator is
    /// the only way the pod can reach dual-stack remotes.
    // r[impl infra.nat64.dns64.force-translation]
    force_dns64_translation: bool,
    /// Upstream DNS servers CoreDNS forwards to. When `--dns-upstreams`
    /// is set, these are the operator-supplied servers; otherwise it's
    /// a single entry pointing at seedling's in-process forwarder on
    /// the resolver-bridge gateway.
    dns_upstreams: Vec<std::net::SocketAddr>,
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
    /// Scratch state for the image reconcile phase (last-GC timestamp).
    // r[impl image.gc]
    image_phase_state: images::ImagePhaseState,
    /// URL Caddy should use for `tls.certificates.get_certificate`. Set
    /// once at daemon startup; threaded into every proxy config build so
    /// runtime-managed certs are served via the daemon.
    // r[impl tls.cert.serve]
    cert_endpoint_url: Option<String>,
    /// Drives ACME-DNS issuance for hostnames declared by ingresses. The
    /// reconciler calls `ensure()` for each TLS-terminating hostname after
    /// building the proxy config; the coordinator dedups and runs the flow
    /// in the background.
    // r[impl tls.cert.eager-issuance]
    tls_coordinator: Option<Arc<crate::runtime::tls::issuance::Coordinator>>,
    /// Counter of consecutive resolver inline-health-check failures, owned
    /// by the reconciler so [`resolver::ensure_resolver_running`] can
    /// debounce transient probe misses across ticks instead of bouncing
    /// the container on the first 2-second timeout.
    resolver_health_fail_count: std::sync::atomic::AtomicU32,
    /// Last tick's set of (hostname, port) tuples that were in an
    /// app-vs-site-ingress conflict. Used to clear the corresponding
    /// `ingress_conflict` faults on the first tick where the conflict
    /// no longer appears.
    // r[impl ingress.site.conflict]
    prev_ingress_conflicts: std::collections::BTreeSet<(String, u16)>,
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
        db: DbHandle,
        app_registry: Arc<RwLock<AppRegistry>>,
        event_tx: EventSender,
        dns_servers: Vec<Ipv6Addr>,
        dns_upstreams: Vec<std::net::SocketAddr>,
        nat64_active: bool,
        force_dns64_translation: bool,
        shells: Arc<ShellRegistry>,
        cert_endpoint_url: Option<String>,
        tls_coordinator: Option<Arc<crate::runtime::tls::issuance::Coordinator>>,
    ) -> Self {
        let observer = Observer::new(Arc::clone(&driver));
        let written_obs = seed_written_obs(&db);
        let actuator = Actuator::new(
            Arc::clone(&driver),
            node_prefix,
            Arc::clone(&registry),
            dns_servers,
            db.clone(),
            event_tx.clone(),
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
            started_jobs: HashSet::new(),
            completed_jobs: HashSet::new(),
            event_tx,
            prev_states: BTreeMap::new(),
            rolling_updates: HashSet::new(),
            unhealthy_replace_deployments: HashSet::new(),
            replace_failed: HashSet::new(),
            last_seen_generation: HashMap::new(),
            nat64_active,
            // Daemon startup has already called `setup_nat64` iff
            // `nat64_active`; mirror that in our tracking flag.
            nat64_installed: nat64_active,
            force_dns64_translation,
            dns_upstreams,
            resolver_addr: None,
            shells,
            caddy_data_path: tokio::sync::OnceCell::new(),
            warm_cert_first_seen: HashMap::new(),
            image_phase_state: images::ImagePhaseState::new(),
            cert_endpoint_url,
            tls_coordinator,
            resolver_health_fail_count: std::sync::atomic::AtomicU32::new(0),
            prev_ingress_conflicts: std::collections::BTreeSet::new(),
        }
    }

    // r[desired-state.definition]
    // r[desired-state.steady]
    // r[desired-state.during-operation]
    fn snapshot_all_apps(&self) -> Vec<AppSnapshot> {
        let reg = self.app_registry.read();
        let mut snapshots = Vec::new();
        for (name, status) in reg.list() {
            let entry = match reg.get(name.as_str()) {
                Some(e) => e,
                None => continue,
            };
            let phase = entry.phase.lock().clone();
            // r[impl desired-state.during-install]
            // Installing apps participate in reconciliation exactly like
            // Installed apps: the install closure's rt.start() / rt.stop()
            // calls populate active_progress and the reconciler actuates the
            // resources placed into the desired state. Only NotInstalled
            // apps — those that have neither been installed nor have an
            // install currently in flight — are skipped.
            match phase {
                AppPhase::Installed | AppPhase::Installing | AppPhase::Uninstalling => {}
                AppPhase::NotInstalled => continue,
            }
            let _ = status;
            let progress = entry.active_progress.read();
            let app_def_arc = entry.app.def.load_full();
            let app_def = (*app_def_arc).clone();
            // r[impl autonomous.scale]
            let effective_scales = self.compute_effective_scales(&name, &app_def);
            // i[impl resource.stop]
            let stopped_set = {
                let app_name_for_stop = name.clone();
                self.db.call(move |db| {
                    stopped::load_stopped(db, &app_name_for_stop).unwrap_or_default()
                })
            };
            let desired = match phase {
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def, &*self.registry),
                AppPhase::NotInstalled => unreachable!(),
                // r[impl desired-state.during-install]
                // While Installing, the install closure drives every
                // resource into desired state explicitly via rt.start.
                // If the closure hasn't pushed its first entry yet
                // (active_progress is still None), the desired state
                // must stay empty — falling back to the steady-state
                // computation here would have the reconciler racing
                // ahead and starting every static resource before
                // on_install runs the prerequisite setup steps.
                AppPhase::Installing => match (*progress).as_ref() {
                    Some(_) => compute(
                        &name,
                        &app_def,
                        (*progress).as_ref(),
                        &*self.registry,
                        &effective_scales,
                        &stopped_set,
                    ),
                    None => Ok(DesiredState {
                        resources: Vec::new(),
                    }),
                },
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
            let current_generation = entry.current_generation;
            snapshots.push(AppSnapshot {
                name,
                desired,
                app_def,
                phase,
                phase_handle: Arc::clone(&entry.phase),
                warm_cert_hostnames,
                current_generation,
            });
        }
        snapshots
    }

    /// Build the effective-scale map for every Deployment in an app.
    // r[impl update.rolling.over-provision]
    fn compute_effective_scales(&self, app_name: &AppName, app_def: &AppDef) -> EffectiveScales {
        // Collect the data we need before entering db.call().
        let deployments: Vec<(String, u16, u16)> = app_def
            .resources
            .iter()
            .filter_map(|(id, resource)| {
                if let Resource::Deployment(deployment) = resource {
                    let dep_def = deployment.def.lock();
                    Some((
                        id.name.as_str().to_owned(),
                        dep_def.scale.start,
                        dep_def.scale.end,
                    ))
                } else {
                    None
                }
            })
            .collect();
        let app_name_owned = app_name.clone();
        let scale_data: Vec<(String, u16, u16, u16)> = self.db.call(move |db| {
            deployments
                .into_iter()
                .map(|(dep_name, low, high)| {
                    let effective =
                        scaling::effective_scale(db, &app_name_owned, &dep_name, low, high)
                            .unwrap_or(low);
                    (dep_name, low, high, effective)
                })
                .collect()
        });
        let mut scales = EffectiveScales::new();
        for (dep_name, low, high, mut effective) in scale_data {
            // r[impl autonomous.healthcheck-replace.guard]
            // The replace-loop guard suppresses BOTH rolling and
            // healthcheck-driven bumps once it trips. Otherwise a deployment
            // with a permanently-broken healthcheck would keep spawning
            // doomed replacement instances on every tick.
            let suppressed = self
                .replace_failed
                .contains(&(app_name.clone(), dep_name.clone()));
            if !suppressed {
                if self
                    .rolling_updates
                    .contains(&(app_name.clone(), dep_name.clone()))
                {
                    effective = effective.saturating_add(1);
                }
                // r[impl autonomous.healthcheck-replace]
                if self
                    .unhealthy_replace_deployments
                    .contains(&(app_name.clone(), dep_name.clone()))
                {
                    effective = effective.saturating_add(1);
                }
            }
            scales.insert(dep_name, (low, high, effective));
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

        // r[impl infra.nat64.translator.lifecycle]
        // Wake-from-idle: ensure NAT64 translator is installed before any
        // pod starts. Setup is idempotent (forwarding sysctls already at 1,
        // module already loaded, instance-add handles EEXIST) so it is safe
        // to run on every non-idle tick where `nat64_installed` is false.
        if self.nat64_active && !self.nat64_installed {
            match crate::system::jool::setup_nat64().await {
                Ok(()) => {
                    self.nat64_installed = true;
                    self.clear_system_fault("nat64_setup_failed");
                }
                Err(e) => {
                    error!(error = %e, "NAT64 re-setup failed; skipping tick to keep pods offline");
                    self.file_system_fault(
                        "nat64_setup_failed",
                        &format!("NAT64 re-setup failed: {e}"),
                    );
                    return true;
                }
            }
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
                &self.db,
                &apps,
                &self.node_prefix,
                &self.written_obs,
                &self.started_jobs,
                &self.completed_jobs,
            ),
            phases::run_volumes_phase(&self.observer, &self.actuator, &self.db, &apps),
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
                    &self.dns_upstreams,
                    self.nat64_active,
                    self.force_dns64_translation,
                    &self.resolver_health_fail_count,
                ),
            ),
        );

        // --- Process pod results ---
        let running_pods_by_app = self.ingest_pod_results(&apps, pod_updates);

        // --- Process volume results ---
        self.ingest_volume_results(&apps, vol_observations);

        // --- Image reconciliation: pins, tracking, and autonomous GC ---
        // r[impl actuate.image.warm] r[impl image.pin] r[impl image.track]
        self.reconcile_images(running_pods_by_app.values().flatten())
            .await;

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

        // --- Load external-service mapping + site-service endpoint
        // snapshot once per tick. ---
        let ext_snapshot = self
            .db
            .call(
                |db| match crate::runtime::external_service_mappings::ExternalServiceSnapshot::load(
                    db,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "external-service snapshot load failed; using empty");
                        crate::runtime::external_service_mappings::ExternalServiceSnapshot::default()
                    }
                },
            );

        // --- Compute routes (sync) ---
        let (all_routes, route_obs) = phases::compute_routes(
            &apps,
            &running_pods_by_app,
            &self.node_prefix,
            &*self.registry,
            &ext_snapshot,
        );
        self.persist_obs(route_obs);

        // --- Load site-ingress snapshot (DB-backed) once per tick. ---
        // r[impl ingress.site] r[impl ingress.site.attachment]
        let site_ingress_snapshot = self
            .db
            .call(|db| Ok::<_, rusqlite::Error>(site_proxy::load(db)))
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "site_proxy: load failed; using empty snapshot");
                site_proxy::SiteIngressSnapshot {
                    ingresses: Vec::new(),
                    attachments: Vec::new(),
                }
            });

        // --- Compute nftables + proxy (sync, gated on caddy) ---
        let nft_and_proxy = caddy_addrs.map(|addrs| {
            let caddy_ip = addrs.v6;

            let nft_build = phases::compute_nftables_rules(
                &apps,
                &running_pods_by_app,
                caddy_ip,
                self.caddy_v4_addr,
                &self.node_prefix,
                &*self.registry,
                &ext_snapshot,
            );

            let proxy_build = phases::compute_proxy_config(
                &apps,
                &site_ingress_snapshot,
                &running_pods_by_app,
                &self.node_prefix,
                &*self.registry,
                self.cert_endpoint_url.as_deref(),
            );

            (nft_build, proxy_build, caddy_ip)
        });

        // --- Apply phase: concurrent network-plane writes ---
        match nft_and_proxy {
            Some((nft_build, proxy_build, caddy_ip)) => {
                let phases::NftablesBuild {
                    rules: dp_rules,
                    degraded_services_by_app,
                } = nft_build;
                // r[impl fault.service-degraded]
                self.file_service_degraded_faults(&apps, &degraded_services_by_app);
                let phases::ProxyBuildResult {
                    config: proxy_config,
                    observations: proxy_obs,
                    ready_observations: proxy_ready_obs,
                    conflicts: ingress_conflicts,
                    unresolved_site_attachments,
                } = proxy_build;
                // r[impl ingress.site.conflict]
                self.reconcile_ingress_conflicts(&ingress_conflicts);
                // r[impl ingress.site.attachment]
                self.reconcile_unresolved_site_attachments(&unresolved_site_attachments);
                let has_proxy_config =
                    !proxy_config.virtual_hosts.is_empty() || !proxy_config.l4_routes.is_empty();

                // The TLS-managed-ingress enumeration is shared with
                // the OI rollup and the expiry sweep so all three call
                // sites act on exactly the same set of hostnames; the
                // shared function skips `NotInstalled` apps per the
                // rules in [`tls::state::managed_ingresses`].
                let managed_ingresses = {
                    let reg = self.app_registry.read();
                    crate::runtime::tls::state::managed_ingresses(&reg)
                };

                // r[impl tls.cert.eager-issuance]
                // Hand every TLS-terminating hostname to the issuance
                // coordinator. ensure() dedups in-flight requests, skips
                // hostnames with current certs / paused / non-acme_dns
                // policy, and runs the rest in the background.
                if let Some(coord) = self.tls_coordinator.as_ref() {
                    for mi in &managed_ingresses {
                        coord.ensure(&mi.hostname);
                    }
                    // r[impl ingress.site.tailscale]
                    // Site-ingress hostnames (manual + discovered) flow
                    // through the same coordinator. The dispatch inside
                    // ensure() picks the Tailscale issuer for discovered
                    // tailnet hostnames and leaves manual ones to the
                    // ACME pipeline (subject to the operator binding a
                    // policy, same as app ingresses).
                    for hostname in &site_ingress_tls_hostnames(&site_ingress_snapshot) {
                        coord.ensure(hostname);
                    }
                }

                // r[impl tls.fault.expiring]
                // Reconcile cert_expiring_soon faults against the
                // current TLS-terminating ingresses. Manual / CSR-derived
                // certs are surfaced when within fourteen days of expiry;
                // ACME-DNS certs are exempt because the renewal task
                // handles them. The sweep also clears stale faults whose
                // ingress is gone or whose cert has been replaced.
                let expiring_targets: Vec<crate::runtime::tls::expiring::IngressTarget> =
                    managed_ingresses
                        .iter()
                        .map(|mi| crate::runtime::tls::expiring::IngressTarget {
                            app: mi.app.clone(),
                            ingress_name: mi.ingress_name.clone(),
                            hostname: mi.hostname.clone(),
                        })
                        .collect();
                if let Err(e) = self
                    .db
                    .call(move |db| crate::runtime::tls::expiring::sweep(db, &expiring_targets))
                {
                    warn!(error = %e, "tls: expiring-cert fault sweep failed");
                }

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
                        // r[impl fault.proxy-apply-failed]
                        // Mirror the system-level proxy_failed fault at
                        // resource granularity so each affected app sees
                        // the degradation in `DescribeApp` / `GetStatus`,
                        // not just `ListFaults` for `_system`.
                        let app_ingresses: Vec<(AppName, String)> = apps
                            .iter()
                            .flat_map(|a| {
                                a.app_def.resources.values().filter_map(|r| match r {
                                    crate::defs::resource::Resource::Ingress(ing) => {
                                        Some((a.name.clone(), ing.name.as_str().to_owned()))
                                    }
                                    _ => None,
                                })
                            })
                            .collect();
                        let site_ingress_names: Vec<String> = site_ingress_snapshot
                            .ingresses
                            .iter()
                            .filter(|ing| !ing.stale)
                            .map(|ing| ing.name.as_str().to_owned())
                            .collect();
                        self.file_proxy_apply_failed_faults(
                            app_ingresses,
                            site_ingress_names,
                            &format!("apply_config failed: {e}"),
                        );
                    }
                    Ok(()) if has_proxy_config => {
                        self.clear_system_fault("proxy_failed");
                        // r[impl fault.proxy-apply-failed]
                        self.clear_proxy_apply_failed_faults();
                        self.persist_obs(proxy_ready_obs);

                        // r[impl infra.proxy.upgrade.cache]
                        if let Err(e) =
                            caddy::write_cached_proxy_config(&self.data_dir, &proxy_config)
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

        // r[impl infra.nat64.translator.lifecycle]
        if self.nat64_installed {
            match crate::system::jool::teardown_nat64().await {
                Ok(()) => self.nat64_installed = false,
                Err(e) => {
                    // Leave `nat64_installed = true` so the next non-idle
                    // tick won't redundantly try to re-setup. Instance
                    // removal failure is not fatal — the stale instance
                    // is harmless and the next startup will handle it.
                    error!(error = %e, "idle: NAT64 teardown failed");
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Pod result ingestion
    // -----------------------------------------------------------------------

    fn ingest_pod_results(
        &mut self,
        apps: &[AppSnapshot],
        pod_updates: Vec<(AppName, pods::PodActuationUpdate)>,
    ) -> HashMap<AppName, Vec<RunningPod>> {
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
            // r[fault.crash-loop]
            self.file_crash_loop_faults(&app_name, &pod_update);
            // r[fault.healthcheck]
            self.file_health_check_faults(&app_name, &pod_update);
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
                // Track that this Job was started by the reconciler so that
                // a fast-completing job (which exits before the observer's
                // poll catches a `container_running` observation) is still
                // recognised as having "previously ran" on the next tick.
                if instance.kind == crate::defs::resource::ResourceKind::Job {
                    self.started_jobs.insert(instance.id);
                }
            }
            // r[impl autonomous.job-terminal.defense]
            for completed in &pod_update.completed_job_instances {
                self.completed_jobs.insert(*completed);
                self.started_jobs.remove(completed);
            }
            running_pods_by_app.insert(app_name, pod_update.running);
        }
        self.update_replace_state(apps, &running_pods_by_app);
        running_pods_by_app
    }

    // r[impl autonomous.healthcheck-replace]
    // r[impl autonomous.healthcheck-replace.guard]
    fn update_replace_state(
        &mut self,
        apps: &[AppSnapshot],
        running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    ) {
        // Reset the replace-loop guard for any app whose generation increased
        // (operator pushed new code). Simultaneously prune entries for apps
        // that no longer exist.
        let active: std::collections::HashSet<AppName> =
            apps.iter().map(|a| a.name.clone()).collect();
        self.last_seen_generation
            .retain(|app, _| active.contains(app));
        self.replace_failed.retain(|(app, _)| active.contains(app));

        // r[impl autonomous.healthcheck-replace.guard]
        // The guard is in-memory but its truth lives in the persisted fault
        // table. Re-derive each tick so the suppression survives daemon
        // restarts (and so an operator clearing the fault has its effect
        // picked up immediately, ahead of the future fault-clear UI).
        let persisted_failed: std::collections::HashSet<(AppName, String)> = self.db.call(|db| {
            let mut out = std::collections::HashSet::new();
            let active = match crate::runtime::faults::list_active_faults(db, None) {
                Ok(v) => v,
                Err(_) => return out,
            };
            for f in active {
                if f.kind == "health_check_replace_failed"
                    && let Some(dep) = f.resource_name.clone()
                {
                    out.insert((f.app, dep));
                }
            }
            out
        });
        for entry in persisted_failed {
            self.replace_failed.insert(entry);
        }

        for app in apps {
            let prior = self.last_seen_generation.get(&app.name).copied();
            if prior.map(|p| app.current_generation > p).unwrap_or(false) {
                // r[impl autonomous.healthcheck-replace.guard]
                // Operator changed the AppDef — give the workload a fresh
                // chance to converge. Clear both the in-memory bump
                // suppression and the persisted hard fault.
                self.replace_failed.retain(|(a, _)| a != &app.name);
                self.clear_replace_failed_faults(&app.name);
            }
            self.last_seen_generation
                .insert(app.name.clone(), app.current_generation);
        }

        // Recompute the bump set from scratch each tick. A deployment is bumped
        // when (a) it has any unhealthy running instance with on_failure=replace,
        // (b) the count of healthy running instances is below the declared
        // target, and (c) the replace-loop guard for it has not tripped.
        self.unhealthy_replace_deployments.clear();
        // Per-deployment grace window in seconds, used downstream to detect
        // replacement-failure from observation history.
        let mut grace_secs_by_dep: HashMap<(AppName, String), i64> = HashMap::new();
        for app in apps {
            let pods = match running_pods_by_app.get(&app.name) {
                Some(p) => p.as_slice(),
                None => continue,
            };
            for (id, resource) in &app.app_def.resources {
                let crate::defs::resource::Resource::Deployment(dep) = resource else {
                    continue;
                };
                let dep_def = dep.def.lock();
                let target = dep_def.scale.start;
                let pod_def = dep_def.pod.lock();
                let container = pod_def.container.lock();
                let (policy, grace_secs) = match &container.healthcheck {
                    Some(hc) => (
                        Some(hc.on_failure),
                        (hc.start_period_secs + hc.retries as u64 * hc.interval_secs) as i64,
                    ),
                    None => (None, 0),
                };
                drop(container);
                drop(pod_def);
                drop(dep_def);
                if !matches!(
                    policy,
                    Some(crate::defs::container::HealthcheckOnFailure::Replace)
                ) {
                    continue;
                }
                let dep_name = id.name.as_str();
                grace_secs_by_dep.insert((app.name.clone(), dep_name.to_owned()), grace_secs);
                if self
                    .replace_failed
                    .contains(&(app.name.clone(), dep_name.to_owned()))
                {
                    continue;
                }
                let dep_pods: Vec<&RunningPod> = pods
                    .iter()
                    .filter(|p| {
                        p.instance.kind == crate::defs::resource::ResourceKind::Deployment
                            && p.instance.name.as_deref() == Some(dep_name)
                    })
                    .collect();
                let healthy = dep_pods.iter().filter(|p| p.observed_healthy).count();
                let any_unhealthy = dep_pods.iter().any(|p| !p.observed_healthy);
                if any_unhealthy && healthy < usize::from(target) {
                    self.unhealthy_replace_deployments
                        .insert((app.name.clone(), dep_name.to_owned()));
                }
            }
        }

        // r[impl autonomous.healthcheck-replace.guard]
        // Check each deployment that's currently bringing up a fresh instance —
        // whether the bump is from a rolling update (operator pushed new code)
        // or from healthcheck-driven replace (existing instance went sour).
        // Both paths spawn a "youngest" scaled instance that we expect to
        // become healthy within the grace window. If it doesn't, declare the
        // replacement failed, file the hard fault, and suppress further bumps
        // (of either kind) until the AppDef generation advances.
        let mut candidate_keys: std::collections::HashSet<(AppName, String)> =
            self.unhealthy_replace_deployments.iter().cloned().collect();
        candidate_keys.extend(self.rolling_updates.iter().cloned());
        let candidates: Vec<((AppName, String), i64)> = candidate_keys
            .into_iter()
            .filter_map(|key| grace_secs_by_dep.get(&key).copied().map(|g| (key, g)))
            .collect();
        let now_ms = jiff::Timestamp::now().as_millisecond();
        let failed: Vec<(AppName, String, String)> = self.db.call(move |db| {
            let mut out = Vec::new();
            for ((app, dep_name), grace_secs) in candidates {
                let group = match crate::runtime::history::find_instances_for_group(
                    db,
                    &app,
                    crate::defs::resource::ResourceKind::Deployment,
                    Some(dep_name.as_str()),
                ) {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                // find_instances_for_group returns oldest first; the youngest
                // is the most likely replacement attempt.
                let Some(youngest) = group.last() else {
                    continue;
                };
                let obs = match crate::runtime::history::query_observations(db, youngest) {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                let any_healthy = obs.iter().any(|o| o.obs_kind == "health_check_pass");
                if any_healthy {
                    continue;
                }
                let Some(first) = obs.first() else {
                    continue;
                };
                let age_secs = (now_ms - first.recorded_at) / 1000;
                // Allow some slack: 2× grace before declaring failed, so a
                // replacement that's marginally slow doesn't trip the guard.
                if age_secs > grace_secs.saturating_mul(2) {
                    out.push((app.clone(), dep_name.clone(), youngest.display_name.clone()));
                }
            }
            out
        });
        for (app, dep_name, replacement_display) in failed {
            self.replace_failed.insert((app.clone(), dep_name.clone()));
            self.unhealthy_replace_deployments
                .remove(&(app.clone(), dep_name.clone()));
            self.file_replace_failed_fault(&app, &dep_name, &replacement_display);
        }
    }

    fn ingest_volume_results(
        &mut self,
        apps: &[AppSnapshot],
        vol_updates: Vec<(AppName, volumes::VolumeActuationUpdate)>,
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
        running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
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
                    let app_name_owned = app.name.clone();
                    let phase_handle = Arc::clone(&app.phase_handle);
                    self.db.call(move |db| {
                        transition_phase(
                            &phase_handle,
                            AppPhase::NotInstalled,
                            db,
                            &app_name_owned,
                            "",
                        );
                        if let Err(e) = db.conn.execute(
                            "DELETE FROM resource_instances WHERE app = ?1",
                            rusqlite::params![app_name_owned],
                        ) {
                            warn!(app = %app_name_owned, "failed to clean up resource instances during uninstall: {e}");
                        }
                    });
                    // i[impl event.types]
                    // Uninstall is reconciler-driven and therefore emits no
                    // OperationCompleted event; this phase change is the only
                    // signal the UI gets that teardown has finished.
                    self.event_tx
                        .app_phase_changed(&app.name, "not_installed", None);
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
