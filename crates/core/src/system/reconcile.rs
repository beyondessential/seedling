use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    net::{Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use ipnet::Ipv6Net;
use parking_lot::{Mutex, RwLock};
use seedling_protocol::events::EventSender;
use seedling_protocol::names::AppName;
use tokio::sync::RwLock as AsyncRwLock;
use tracing::{error, warn};

use crate::{
    defs::{app::AppDef, enums::Priority, resource::Resource},
    oi::shells::ShellRegistry,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::DbHandle,
        desired::{DesiredState, EffectiveScales, compute, compute_uninstalling},
        gc::InstanceGcConfig,
        identity::InstanceId,
        lifecycle::LifecycleState,
        priority::AppPriority,
        scaling,
        site_services::{resolve::ResolveCtx, resolver::SiteServiceResolver},
        stopped,
    },
    system::{
        System,
        actuator::Actuator,
        caddy,
        observer::Observer,
        priority, resolver,
        types::{DataPlaneRules, SliceSpec, UnitSummary},
    },
};

pub use restarts::gc as gc_restart_records;

mod faults;
mod images;
mod phases;
mod restarts;
mod site_proxy;
mod state;

/// Per-tick budget for bringing each infra component (Caddy, resolver) up. The
/// whole ensure_* operation — image acquisition, container start, and health
/// check — must fit within this; overrunning it is reported as a bring-up
/// timeout for that component and the tick moves on.
const INFRA_ENSURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Back-off for reapplying the resource-control slices after the supervisor
/// refuses them. Capped rather than terminal: a host that recovers picks the
/// slices up again without an operator touching anything.
// spec: PRIO#resource-control-groups
const SLICE_BACKOFF_BASE: Duration = Duration::from_secs(10);
/// Budget for one slice sync. Bounded like every other blocking supervisor call
/// on this path, so a wedged bus cannot hold up the tick's actuation.
const SLICE_SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const SLICE_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// Consecutive failures before the operator is told. Below this the retry is
/// quiet: a slice apply that fails once and succeeds on the next tick is not
/// worth a fault.
const SLICE_FAULT_THRESHOLD: u32 = 5;

/// Enumerate hostnames from the site-ingress snapshot that need TLS this
/// tick. A hostname is only emitted when its parent ingress is non-stale,
/// declares a non-`None` TLS provider, *and* carries at least one
/// attachment — there's no point asking the issuance coordinator (and,
/// for tailnet hostnames, tailscaled) to mint a cert for a hostname
/// nothing is going to serve traffic on yet.
// spec: SITE#tailscale-discovery
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
    // These three are persisted by `ObservationFact::to_obs_kinds` but were
    // absent here, so their rows could not be interned and the restart seed
    // dropped them: a container unhealthy across a restart had a duplicate
    // `health_check_fail` appended with a fresh timestamp on every boot, and
    // the same for a start-limit-hit unit and a mismatched volume backend.
    "health_check_fail",
    "unit_start_limit_hit",
    "volume_backend_mismatch",
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
    /// spec: LCY#routing-pool
    pub observed_healthy: bool,
}

/// Whether a tick's view of the installed apps is complete enough to apply as
/// absolute state.
///
/// Routes, nftables rules and the proxy config are rebuilt from the tick's
/// snapshots and applied wholesale: whatever is not in the computed set is
/// removed from the host. So an app that fell out of the set because its
/// desired state or a registry lookup errored is not "an app that contributes
/// nothing" — it is an app whose DNAT, service routes and vhosts are about to
/// be withdrawn while its containers keep running.
///
/// The reconciler already applies this rule to one failure: a Caddy bring-up
/// failure skips the nftables and proxy applies for the tick rather than
/// applying a reduced state. `Partial` extends it to the rest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Coverage {
    Complete,
    Partial,
}

impl Coverage {
    fn of(dropped: usize) -> Self {
        if dropped == 0 {
            Self::Complete
        } else {
            Self::Partial
        }
    }

    fn is_complete(self) -> bool {
        self == Self::Complete
    }

    /// Weaken to `Partial` if `other` is.
    fn and(self, other: Self) -> Self {
        if self.is_complete() && other.is_complete() {
            Self::Complete
        } else {
            Self::Partial
        }
    }
}

/// Point-in-time snapshot of a single app's state, taken at tick start.
struct AppSnapshot {
    name: AppName,
    desired: DesiredState,
    /// `desired` was computed from the app definition, so it names every
    /// instance the app's active desired state holds. False while an
    /// operation, install or uninstall is steering it instead.
    // spec: GC#unscheduled-instances
    steady: bool,
    app_def: AppDef,
    phase: AppPhase,
    phase_handle: Arc<Mutex<AppPhase>>,
    /// Hostnames whose certs should be pre-warmed. Captured from the in-flight
    /// `OperationProgress`, if any.
    // spec: RT#certificate-warming
    warm_cert_hostnames: std::collections::BTreeSet<String>,
    /// Generation number of the app definition this snapshot was taken from.
    /// Used to detect operator-pushed updates that should reset transient
    /// per-deployment state such as the replace-loop guard.
    // spec: AUTO#replacement-guard
    current_generation: u64,
    /// The operator-set app priority, read fresh this tick so a change takes
    /// effect without a redeploy.
    // spec: PRIO#priority-setting
    app_priority: AppPriority,
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
    // spec: AUTO#job-completion
    /// Job instance IDs known to have completed during this process lifetime.
    /// If these appear running on a subsequent tick they are stopped immediately.
    completed_jobs: HashSet<InstanceId>,
    /// Instances with an active `crash_loop` fault. Re-derived from the fault
    /// table at the top of each tick so the suppression survives a daemon
    /// restart and lifts as soon as an operator clears the fault.
    // spec: FLT#workload-faults
    crash_looped: HashSet<InstanceId>,
    event_tx: EventSender,
    shells: Arc<ShellRegistry>,
    /// Previous tick's lifecycle states, keyed by (app, instance_id_hex).
    prev_states: BTreeMap<(AppName, String), LifecycleState>,
    /// Deployments with an active rolling update, keyed by (app, deployment_name).
    /// Set from pod actuation results; read by `compute_effective_scales` to bump
    /// the effective instance count by one during rollouts.
    // spec: UPD#rolling-updates
    rolling_updates: HashSet<(AppName, String)>,
    /// Deployments needing a healthcheck-driven replacement, keyed by
    /// (app, deployment_name). Populated each tick from observed pod health:
    /// any deployment with `on_failure: replace` whose healthy backend count
    /// is below its target gets bumped by one effective instance, so
    /// `ensure_scaled_group` provisions a fresh replacement alongside the
    /// existing (unhealthy) one.
    // spec: AUTO#health-check-replacement
    unhealthy_replace_deployments: HashSet<(AppName, String)>,
    /// Deployments whose latest replacement attempt itself failed to become
    /// healthy. While present, the runtime suppresses further bumps
    /// (`autonomous.healthcheck-replace.guard`). Cleared when the AppDef
    /// generation for the app changes (operator-pushed update).
    // spec: AUTO#replacement-guard
    replace_failed: HashSet<(AppName, String)>,
    /// Last `current_generation` we saw for each app; a higher value on a
    /// subsequent tick triggers a reset of `replace_failed` for that app.
    // spec: AUTO#replacement-guard
    last_seen_generation: HashMap<AppName, u64>,
    /// Paces retries when the supervisor refuses the slice set, so a host with
    /// a read-only unit directory is not asked to re-read its units every tick.
    /// One gate, not one per slice: the set is synced as a whole.
    // spec: PRIO#resource-control-groups
    slice_retry: crate::runtime::retry::RetryGate,
    /// Whether seedling is providing its own NAT64 translator.
    nat64_active: bool,
    /// Whether the jool translator instance is currently installed on
    /// this host. The daemon sets up NAT64 at startup when required,
    /// and the reconciler tears it down on transition to idle and
    /// re-installs it on the transition back to non-idle.
    // spec: NAT64#translator-lifecycle
    nat64_installed: bool,
    /// Whether the DNS64 plugin should translate names that already
    /// have a real AAAA record (emits `translate_all` into the
    /// Corefile). Set when NAT64 is seedling-managed and the host has
    /// no IPv6 egress — forcing every flow through the translator is
    /// the only way the pod can reach dual-stack remotes.
    // spec: NAT64#dns64
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
    // spec: OBS#ingresses
    caddy_data_path: tokio::sync::OnceCell<PathBuf>,
    /// First time each warm-cert hostname was seen during the current process
    /// lifetime, used to gate `cert_acquisition_failed` faults.
    // spec: FLT#certificate-faults
    warm_cert_first_seen: HashMap<String, std::time::Instant>,
    /// What has been read of the stored certificates, so each is parsed once
    /// and not on every tick.
    leaf_facts: crate::runtime::tls::env::LeafFactsCache,
    /// Scratch state for the image reconcile phase (last-GC timestamp).
    // spec: IMG#image-garbage-collection
    image_phase_state: images::ImagePhaseState,
    /// Cadence and retention of the unscheduled-instance sweep, and when it
    /// last ran.
    // spec: GC#unscheduled-instances
    instance_gc: InstanceGcConfig,
    last_instance_gc: Option<std::time::Instant>,
    /// URL Caddy should use for `tls.certificates.get_certificate`. Set
    /// once at daemon startup; threaded into every proxy config build so
    /// runtime-managed certs are served via the daemon.
    // spec: TLS#serving
    cert_endpoint_url: Option<String>,
    /// Drives ACME-DNS issuance for hostnames declared by ingresses. The
    /// reconciler calls `ensure()` for each TLS-terminating hostname after
    /// building the proxy config; the coordinator dedups and runs the flow
    /// in the background.
    // spec: TLS#issuance
    tls_coordinator: Option<Arc<crate::runtime::tls::issuance::Coordinator>>,
    /// The renewal points last handed to the proxy, kept for the ticks on which
    /// they cannot be worked out: the proxy configuration is rebuilt and applied
    /// whole, so leaving them out would tell the proxy to renew whenever it
    /// likes for as long as the facts were unreadable.
    // spec: CHAIN#proxy-strategies
    last_renewal_ratios: std::collections::BTreeMap<String, f64>,
    /// Counter of consecutive resolver inline-health-check failures, owned
    /// by the reconciler so [`resolver::ensure_resolver_running`] can
    /// debounce transient probe misses across ticks instead of bouncing
    /// the container on the first 2-second timeout.
    resolver_health_fail_count: std::sync::atomic::AtomicU32,
    /// DNS resolver feeding `site_service_endpoints.remote_host` lookups
    /// into the data plane. Optional only because the daemon's startup
    /// path may have failed to read system DNS config; in that case the
    /// reconciler still runs but DNS-named site service endpoints stay
    /// unresolved.
    // spec: SITE#endpoint-addressing
    site_resolver: Option<Arc<SiteServiceResolver>>,
    /// Consecutive failed observations per instance, so a single blip logs an
    /// error without minting an operator-visible fault. In memory beside the
    /// reconciler's other per-tick state; a restart re-observes anyway.
    // spec: OBS#failed-observation
    observe_failure_streaks: HashMap<InstanceId, u32>,
}

/// Units that could belong to `app`, judged only by the shape of their names.
///
/// This is a candidate enumeration and never a decision — `seedling-{app}-` is
/// not prefix-free, so `app-db`'s units land here too. It exists for the one
/// question the recorded identities cannot answer when there are none of them:
/// is there anything at all that might still be this app's?
// spec: TRD#uninstall-scope
/// Workload units that could belong to `app`, as candidates only — the caller
/// decides by matching exactly against recorded identity.
///
/// Restricted to services because the question is whether a *workload* is still
/// loaded. The runtime also owns slices under this same prefix
/// (`seedling-{app}-{tier}.slice`), and those outlive every workload in them by
/// design, so counting one as evidence of a live workload would leave an app
/// with no recorded instances unable to ever finish tearing down.
fn units_possibly_of<'a>(units: &'a [UnitSummary], app: &AppName) -> Vec<&'a UnitSummary> {
    let prefix = format!("seedling-{app}-");
    units
        .iter()
        .filter(|u| u.name.ends_with(".service") && u.name.starts_with(&prefix))
        .collect()
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
        site_resolver: Option<Arc<SiteServiceResolver>>,
        instance_gc: InstanceGcConfig,
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
            crash_looped: HashSet::new(),
            event_tx,
            prev_states: BTreeMap::new(),
            slice_retry: crate::runtime::retry::RetryGate::new(
                SLICE_BACKOFF_BASE,
                SLICE_BACKOFF_CAP,
            ),
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
            leaf_facts: Default::default(),
            image_phase_state: images::ImagePhaseState::new(),
            instance_gc,
            last_instance_gc: None,
            cert_endpoint_url,
            tls_coordinator,
            last_renewal_ratios: Default::default(),
            resolver_health_fail_count: std::sync::atomic::AtomicU32::new(0),
            site_resolver,
            observe_failure_streaks: HashMap::new(),
        }
    }

    // RCN#desired-state
    // RCN#desired-state
    // RCN#desired-state
    ///
    /// Returns the snapshots alongside how many installed apps were dropped
    /// because computing their desired state failed. The count is not
    /// diagnostic: routes, nftables rules and the proxy config are applied as
    /// absolute state, so a dropped app is indistinguishable from a deleted
    /// one unless the tick is told the set is incomplete.
    fn snapshot_all_apps(&self) -> (Vec<AppSnapshot>, usize) {
        let reg = self.app_registry.read();
        let mut snapshots = Vec::new();
        let mut skipped = 0usize;
        // spec: PRIO#priority-setting
        // One read of every app's standing for the whole tick, rather than a
        // round trip per app. A failure is not an answer: the standings decide
        // both the slice weights and the kill preference baked into any unit
        // started this tick, so rather than substituting a level nobody chose,
        // every app is reported as dropped and the tick holds what is running.
        let priorities = match self
            .db
            .call(crate::runtime::priority::load_all_app_priorities)
        {
            Ok(p) => {
                self.clear_system_fault("app_priorities_unreadable");
                p
            }
            Err(e) => {
                error!(
                    error = %e,
                    "could not read app priorities; holding this tick rather than actuating \
                     workloads at a standing nobody chose"
                );
                // Filed on the first failure rather than past a threshold: this
                // halts the whole tick, so nothing else — routes, health,
                // restarts, teardown — advances either, and an operator looking
                // at a system that has silently stopped moving needs to be told
                // why. Filing is idempotent, so repeated ticks do not pile up.
                self.file_system_fault(
                    "app_priorities_unreadable",
                    &format!(
                        "could not read the app priorities that decide how workloads compete \
                         for host resources, so reconciliation is held rather than actuating \
                         them at a standing nobody chose. Nothing else advances while this \
                         persists. Error: {e}"
                    ),
                );
                return (Vec::new(), reg.names().len().max(1));
            }
        };
        // spec: AUTO#scale
        // spec: AUTO#unreadable-scale
        // One read of every operator scaling decision for the whole tick,
        // rather than a round trip per app. A failure is not an answer: the
        // decisions say how many instances each deployment is owed, so rather
        // than standing the declared floor in and converging every scaled-up
        // deployment down to it, every app is reported as dropped and the tick
        // holds what is running.
        let scaling_decisions = match self
            .db
            .call(crate::runtime::scaling::load_all_scaling_decisions)
        {
            Ok(d) => {
                self.clear_system_fault("scaling_decisions_unreadable");
                d
            }
            Err(e) => {
                error!(
                    error = %e,
                    "could not read the operator scaling decisions; holding this tick rather \
                     than converging deployments to a scale nobody chose"
                );
                self.file_system_fault(
                    "scaling_decisions_unreadable",
                    &format!(
                        "could not read the record of operator-chosen scale, which says how \
                         many instances each deployment is owed, so reconciliation is held \
                         rather than converging them to a scale nobody chose. Nothing else \
                         advances while this persists. Error: {e}"
                    ),
                );
                return (Vec::new(), reg.names().len().max(1));
            }
        };

        for name in reg.names() {
            let entry = match reg.get(name.as_str()) {
                Some(e) => e,
                None => continue,
            };
            let phase = entry.phase.lock().clone();
            // spec: RCN#desired-state
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
            let progress = entry.active_progress.read();
            let app_def_arc = entry.app.def.load_full();
            let app_def = (*app_def_arc).clone();
            // spec: AUTO#scale
            let effective_scales =
                self.compute_effective_scales(&name, &app_def, &scaling_decisions);
            // spec: CTL#resource-stop
            let stopped_set = {
                let app_name_for_stop = name.clone();
                self.db.call(move |db| {
                    stopped::load_stopped(db, &app_name_for_stop).unwrap_or_default()
                })
            };
            let desired = match phase {
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def, &*self.registry),
                AppPhase::NotInstalled => unreachable!(),
                // spec: RCN#desired-state
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
                    skipped += 1;
                    continue;
                }
            };
            let warm_cert_hostnames = (*progress)
                .as_ref()
                .map(|p| p.warm_cert_hostnames.clone())
                .unwrap_or_default();
            let current_generation = entry.current_generation;
            // spec: PRIO#priority-setting
            // Read once for the whole tick above; an app with no stored
            // decision stands at the default.
            let app_priority = priorities.get(&name).copied().unwrap_or_default();
            let steady = phase == AppPhase::Installed && progress.is_none();
            snapshots.push(AppSnapshot {
                name,
                desired,
                steady,
                app_def,
                phase,
                phase_handle: Arc::clone(&entry.phase),
                warm_cert_hostnames,
                current_generation,
                app_priority,
            });
        }
        (snapshots, skipped)
    }
}

/// The slices the running workloads need, with the weights each should carry.
///
/// Pure so the derivation can be checked without a supervisor: applying it is
/// [`Reconciler::reconcile_slices`].
// spec: PRIO#resource-control-groups
// spec: PRIO#contended-capacity
fn desired_slices(apps: &[AppSnapshot]) -> BTreeMap<String, SliceSpec> {
    let mut desired: BTreeMap<String, SliceSpec> = BTreeMap::new();

    for app in apps {
        let app_slice = priority::app_slice(&app.name);
        desired.insert(
            app_slice.clone(),
            SliceSpec {
                name: app_slice,
                description: format!("seedling app {}", app.name),
                weight: priority::app_weight(app.app_priority),
            },
        );

        // The levels this app's definition actually declares, plus Normal,
        // where Jobs and every other workload that is not a Deployment stand.
        // A level that stops being declared has its slice forgotten, which is
        // safe: forgetting unlinks the unit file and leaves anything still
        // running in the slice alone.
        let mut tiers: BTreeSet<Priority> = BTreeSet::new();
        tiers.insert(Priority::Normal);
        for resource in app.app_def.resources.values() {
            if let Resource::Deployment(deployment) = resource {
                tiers.insert(deployment.def.lock().priority);
            }
        }
        for tier in tiers {
            let name = priority::tier_slice(&app.name, tier);
            desired.insert(
                name.clone(),
                SliceSpec {
                    name,
                    description: format!("seedling app {} at {}", app.name, tier.as_str()),
                    weight: priority::deployment_weight(tier),
                },
            );
        }
    }

    // spec: PRIO#kill-order
    // spec: PRIO#resource-control-groups
    // Inserted last so it always wins the key: an app named for the
    // infrastructure component realises the same slice name, and an app that
    // predates the reservation on that name would otherwise reweight the group
    // holding the proxy and the resolver down to its own standing.
    desired.insert(
        priority::INFRA_SLICE.to_owned(),
        SliceSpec {
            name: priority::INFRA_SLICE.to_owned(),
            description: "seedling infrastructure".to_owned(),
            weight: priority::INFRA_WEIGHT,
        },
    );

    desired
}

impl Reconciler {
    /// How long the proxy is to wait before renewing each certificate it holds
    /// that a strategy ahead of its own has a claim on.
    ///
    /// When the decisions cannot be made the last ones stand, rather than an
    /// empty set that would release every hostname at once.
    // spec: CHAIN#proxy-strategies
    fn resolve_renewal_ratios(&mut self) -> std::collections::BTreeMap<String, f64> {
        // Asked every tick, and answered from the one state the coordinator
        // holds for every chain decision: a second copy here would be a second
        // age for the same decisions, and one no operator action could clear.
        match self
            .tls_coordinator
            .as_ref()
            .and_then(|coordinator| coordinator.proxy_renewal_ratios())
        {
            Some(ratios) => {
                self.last_renewal_ratios = ratios.clone();
                ratios
            }
            // Not worked out this time: the last ones stand, since the proxy's
            // configuration is applied whole and leaving them out would tell it
            // to renew whenever it liked.
            None => self.last_renewal_ratios.clone(),
        }
    }
}

impl Reconciler {
    /// Bring the resource-control slices into step with the app priorities and
    /// the declared Deployment levels.
    ///
    /// Runs before actuation so a workload started this tick joins a slice that
    /// already carries the right weights, and reweights the slices of workloads
    /// that are already running, which is what makes an app priority change
    /// take effect without a redeploy.
    // spec: PRIO#resource-control-groups
    async fn reconcile_slices(&mut self, apps: &[AppSnapshot], dropped_apps: usize) {
        let now = Instant::now();
        if !self.slice_retry.should_attempt(now) {
            return;
        }

        let desired: Vec<SliceSpec> = desired_slices(apps).into_values().collect();
        // spec: RCN#applying-absolute-state
        // Forgetting a slice is only safe when every app was accounted for. An
        // app dropped by a transient error is absent from `apps` while its
        // workloads keep running, and its slice carries the weights they run
        // under.
        let prune = dropped_apps == 0;

        let synced = tokio::time::timeout(
            SLICE_SYNC_TIMEOUT,
            self.driver.process.sync_slices(desired, prune),
        )
        .await;

        match synced {
            Ok(Ok(())) => {
                self.slice_retry.record_success();
                self.clear_system_fault("slice_sync_failed");
            }
            Ok(Err(e)) => self.record_slice_failure(now, &e.to_string()),
            Err(_) => self.record_slice_failure(
                now,
                &format!("timed out after {}s", SLICE_SYNC_TIMEOUT.as_secs()),
            ),
        }
    }

    /// Pace the retry and, past a threshold, tell the operator. There is no
    /// give-up state: the next tick past the back-off tries again.
    // spec: PRIO#resource-control-groups
    fn record_slice_failure(&mut self, now: Instant, error: &str) {
        self.slice_retry.record_failure(now);
        let failures = self.slice_retry.failures();
        warn!(error, failures, "failed to sync resource-control slices");
        if failures >= SLICE_FAULT_THRESHOLD {
            self.file_system_fault(
                "slice_sync_failed",
                &format!(
                    "could not apply the resource-control groups that carry workload priority, \
                     after {failures} attempts; workloads run without their configured CPU and \
                     I/O shares until this clears. Last error: {error}"
                ),
            );
        }
    }
}

impl Reconciler {
    /// Build the effective-scale map for every Deployment in an app.
    ///
    /// Reads from the decisions the tick already loaded: the table is read
    /// once for the node, not once per app, because this runs for every app on
    /// every tick.
    // spec: UPD#rolling-updates
    fn compute_effective_scales(
        &self,
        app_name: &AppName,
        app_def: &AppDef,
        decisions: &crate::runtime::scaling::ScalingDecisions,
    ) -> EffectiveScales {
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
        let scale_data: Vec<(String, u16, u16, u16)> = deployments
            .into_iter()
            .map(|(dep_name, low, high)| {
                let stored = decisions
                    .get(&(app_name.clone(), dep_name.clone()))
                    .copied();
                let effective = scaling::effective_scale_from(stored, low, high);
                (dep_name, low, high, effective)
            })
            .collect();
        let mut scales = EffectiveScales::new();
        for (dep_name, low, high, mut effective) in scale_data {
            // spec: AUTO#replacement-guard
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
                // spec: AUTO#health-check-replacement
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

    // RCN#reconciliation-loop
    // RCN#reconciliation-loop
    // RCN#reconciliation-loop
    // FLT#filing-and-clearing
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn tick(&mut self) -> bool {
        // spec: INFRA#unavailable-backends — when the daemon came up without a
        // working container runtime / process manager, every actuation would
        // just error against the unavailable backends. File the overarching
        // system fault (idempotent) and skip the rest of the tick so the OI
        // stays responsive and the operator sees why nothing is running.
        if let Some(reason) = self.driver.degraded_reason() {
            self.file_system_fault(
                "container_runtime_unavailable",
                &format!(
                    "podman/systemd could not be reached at startup, so no workloads can be \
                     actuated. Restart seedling once the container runtime and service manager \
                     are healthy. Details: {reason}"
                ),
            );
            return true;
        }

        self.reconcile_stray_shells().await;

        let (apps, dropped_apps) = self.snapshot_all_apps();

        if apps.is_empty() {
            // spec: RCN#applying-absolute-state — an empty snapshot list
            // means "no app is installed" only when nothing was dropped on the
            // way here. Tearing down on a transient registry error would flush
            // every rule and remove Caddy, the resolver and NAT64 while the
            // workloads keep running.
            if dropped_apps == 0 {
                self.tear_down_idle().await;
            } else {
                warn!(
                    dropped = dropped_apps,
                    "every installed app failed to compute its desired state; \
                     holding the data plane rather than tearing it down"
                );
            }
            return false;
        }
        let app_coverage = Coverage::of(dropped_apps);

        self.reconcile_slices(&apps, dropped_apps).await;

        // spec: NAT64#translator-lifecycle
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

        // spec: FLT#workload-faults
        // The auto-restart suppression is in-memory but its truth lives in the
        // fault table, so re-derive it each tick: the suppression then
        // survives a daemon restart, and an operator clearing the fault takes
        // effect on the very next tick.
        self.crash_looped = self.db.call(|db| {
            let mut out = HashSet::new();
            let Ok(active) = crate::runtime::faults::list_active_faults(db, None) else {
                return out;
            };
            for f in active {
                if f.kind == "crash_loop"
                    && let Some(hex) = f.instance_id.as_deref()
                    && let Some(id) = InstanceId::from_hex(hex)
                {
                    out.insert(id);
                }
            }
            out
        });

        // spec: RCN#reconciliation-loop
        // --- Concurrent phase: pods ∥ volumes ∥ caddy ∥ resolver ---
        let (pod_updates, vol_observations, caddy_result, resolver_result) = tokio::join!(
            // spec: AUTO#job-completion
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
                &self.crash_looped,
            ),
            phases::run_volumes_phase(&self.observer, &self.actuator, &self.db, &apps),
            tokio::time::timeout(
                INFRA_ENSURE_TIMEOUT,
                caddy::ensure_caddy_running(
                    &*self.driver.container,
                    &*self.driver.process,
                    &self.node_prefix,
                    &self.data_dir,
                ),
            ),
            // spec: DNS#resolver-lifecycle
            tokio::time::timeout(
                INFRA_ENSURE_TIMEOUT,
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
        // spec: IMG#image-warming, IMG#pins, IMG#image-usage-tracking
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
        // AUTO#ingress-and-network-repair
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
                // `e` already names the specific failure (image pull, container
                // start, health check, …); don't presume it was the health
                // check.
                error!(error = %e, "caddy bring-up failed; skipping nftables and proxy this tick");
                self.file_system_fault("caddy_failed", &format!("caddy bring-up failed: {e}"));
                None
            }
            Err(_) => {
                // The whole ensure_caddy_running (image acquisition, container
                // start, and health check) exceeded the tick budget — not
                // specifically the health check.
                let secs = INFRA_ENSURE_TIMEOUT.as_secs();
                warn!(
                    "caddy bring-up did not complete within {secs}s; skipping nftables and proxy this tick"
                );
                self.file_system_fault(
                    "caddy_failed",
                    &format!("caddy bring-up did not complete within {secs}s (image pull, container start, or health check)"),
                );
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

        // --- Build the per-tick site-service resolution context. The
        //     reconciler reads the resolver cache, NAT64 activation, and
        //     IPv6-egress flag once and passes the borrow into every
        //     consumer (routes, nftables, fault classification). When the
        //     site resolver isn't available (system DNS config unreadable
        //     at startup) we substitute an empty lookup; DNS-named
        //     endpoints will surface as `Unresolved` and produce a fault
        //     after the consecutive-failure threshold via the resolver
        //     loop's separate path. ---
        // spec: SITE#endpoint-addressing
        let site_resolver_clone = self.site_resolver.clone();
        let nat64_active = self.nat64_active;
        let has_ipv6_egress = !self.force_dns64_translation;
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let make_resolve_ctx = || -> ResolveCtx<'_> {
            let resolver_ref: &dyn crate::runtime::site_services::resolve::HostnameLookup =
                match site_resolver_clone.as_deref() {
                    Some(r) => r,
                    None => &empty_lookup,
                };
            ResolveCtx {
                nat64_active,
                has_ipv6_egress,
                resolver: resolver_ref,
            }
        };

        // --- Compute routes (sync) ---
        let phases::RoutesBuild {
            routes: all_routes,
            observations: route_obs,
            coverage: routes_coverage,
        } = phases::compute_routes(
            &apps,
            &running_pods_by_app,
            &self.node_prefix,
            &*self.registry,
            &ext_snapshot,
            &make_resolve_ctx(),
        );
        let routes_coverage = routes_coverage.and(app_coverage);
        self.persist_obs(route_obs);

        // --- Classify site-service endpoint outcomes and reconcile the
        //     associated `site_service_endpoint_*` faults. ---
        // spec: SITE#endpoint-addressing
        {
            let unresolved_hosts: HashSet<String> = self
                .site_resolver
                .as_deref()
                .map(|r| r.unresolved_hosts())
                .unwrap_or_default();
            let fault_set = phases::classify_site_service_endpoints(
                &ext_snapshot,
                &make_resolve_ctx(),
                &unresolved_hosts,
            );
            self.reconcile_site_service_faults(fault_set);
        }

        // --- Load site-ingress snapshot (DB-backed) once per tick. ---
        // spec: SITE#site-ingresses, SITE#attachments
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

        // spec: CHAIN#proxy-strategies
        let renewal_ratios = self.resolve_renewal_ratios();

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
                &make_resolve_ctx(),
            );

            let proxy_build = phases::compute_proxy_config(
                &apps,
                &site_ingress_snapshot,
                &running_pods_by_app,
                &self.node_prefix,
                &*self.registry,
                self.cert_endpoint_url.as_deref(),
                &renewal_ratios,
            );

            (nft_build, proxy_build, caddy_ip)
        });

        // --- Apply phase: concurrent network-plane writes ---
        match nft_and_proxy {
            Some((nft_build, proxy_build, caddy_ip)) => {
                let phases::NftablesBuild {
                    rules: dp_rules,
                    degraded_services_by_app,
                    coverage: rules_coverage,
                } = nft_build;
                let rules_coverage = rules_coverage.and(app_coverage);
                // spec: FLT#ingress-and-service-faults
                self.file_service_degraded_faults(&apps, &degraded_services_by_app);
                let phases::ProxyBuildResult {
                    config: proxy_config,
                    coverage: proxy_coverage,
                    observations: proxy_obs,
                    ready_observations: proxy_ready_obs,
                    conflicts: ingress_conflicts,
                    unresolved_site_attachments,
                } = proxy_build;
                let proxy_coverage = proxy_coverage.and(app_coverage);
                // spec: SITE#ingress-conflicts
                self.reconcile_ingress_conflicts(&ingress_conflicts);
                // spec: SITE#attachments
                self.reconcile_unresolved_site_attachments(&unresolved_site_attachments);
                // The TLS-managed-ingress enumeration is shared with
                // the OI rollup and the expiry sweep so all three call
                // sites act on exactly the same set of hostnames; the
                // shared function skips `NotInstalled` apps per the
                // rules in [`tls::state::managed_ingresses`].
                let managed_ingresses = {
                    let reg = self.app_registry.read();
                    crate::runtime::tls::state::managed_ingresses(&reg)
                };

                // spec: TLS#issuance
                // Hand every TLS-terminating hostname to the issuance
                // coordinator. ensure() dedups in-flight requests, skips
                // hostnames with current certs / paused / non-acme_dns
                // policy, and runs the rest in the background.
                if let Some(coord) = self.tls_coordinator.as_ref() {
                    for mi in &managed_ingresses {
                        coord.ensure(&mi.hostname);
                    }
                    // spec: SITE#tailscale-discovery
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

                // spec: TLS#expiring-certificates
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

                // spec: RCN#applying-absolute-state — each of these three
                // replaces the host's whole state for its plane, so an
                // incomplete build must be withheld rather than applied: the
                // apps missing from it are still running.
                for (plane, coverage) in [
                    ("routes", routes_coverage),
                    ("nftables rules", rules_coverage),
                    ("proxy config", proxy_coverage),
                ] {
                    if !coverage.is_complete() {
                        warn!(
                            "an app is missing from this tick's {plane}; \
                             holding the previous state rather than applying a reduced one"
                        );
                    }
                }

                // `None` means the apply was withheld because the build did
                // not cover every app. It is deliberately not `Ok(())`: no
                // apply ran, so nothing may be concluded from it — in
                // particular the plane's fault must neither be filed nor
                // cleared, and no readiness may be recorded.
                let (routes_res, rules_res, proxy_res) = tokio::join!(
                    async {
                        match routes_coverage.is_complete() {
                            true => Some(self.driver.data_plane.apply_routes(&all_routes).await),
                            false => None,
                        }
                    },
                    async {
                        match rules_coverage.is_complete() {
                            true => Some(self.driver.data_plane.apply_rules(&dp_rules).await),
                            false => None,
                        }
                    },
                    apply_proxy_plane(&*self.driver.proxy, &proxy_config, proxy_coverage),
                );

                match routes_res {
                    Some(Err(e)) => {
                        error!(error = %e, "routes: apply_routes failed");
                        self.file_system_fault(
                            "routes_failed",
                            &format!("apply_routes failed: {e}"),
                        );
                    }
                    Some(Ok(())) => self.clear_system_fault("routes_failed"),
                    None => {}
                }
                match rules_res {
                    Some(Err(e)) => {
                        error!(error = %e, "rules: apply_rules failed");
                        self.file_system_fault(
                            "nftables_failed",
                            &format!("apply_rules failed: {e}"),
                        );
                    }
                    Some(Ok(())) => self.clear_system_fault("nftables_failed"),
                    None => {}
                }
                match proxy_res {
                    Some(Err(e)) => {
                        error!(error = ?e, addr = %caddy_ip, "proxy: apply_config failed");
                        self.file_system_fault(
                            "proxy_failed",
                            &format!("apply_config failed: {e}"),
                        );
                        // spec: FLT#ingress-and-service-faults
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
                    Some(Ok(())) => {
                        self.clear_system_fault("proxy_failed");
                        // spec: FLT#ingress-and-service-faults
                        self.clear_proxy_apply_failed_faults();
                        self.persist_obs(proxy_ready_obs);

                        // spec: PRX#proxy-upgrades
                        if let Err(e) =
                            caddy::write_cached_proxy_config(&self.data_dir, &proxy_config)
                        {
                            warn!(
                                error = %e,
                                "proxy: failed to cache proxy config; upgrade continuity may be impaired"
                            );
                        }
                    }
                    None => {}
                }

                // spec: OBS#ingresses
                self.observe_warm_certs(&apps).await;
            }
            None => {
                // Caddy unavailable — still apply routes (they don't need
                // caddy), unless this tick's route set is missing an app.
                if routes_coverage.is_complete() {
                    if let Err(e) = self.driver.data_plane.apply_routes(&all_routes).await {
                        error!(error = %e, "routes: apply_routes failed");
                        self.file_system_fault(
                            "routes_failed",
                            &format!("apply_routes failed: {e}"),
                        );
                    } else {
                        self.clear_system_fault("routes_failed");
                    }
                }
            }
        }

        self.emit_state_changes(&apps);
        self.retire_unscheduled_excess(&apps);
        self.sweep_unscheduled_instances(&apps);

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
                label: Some((crate::system::types::SESSION_LABEL, "shell")),
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
        // spec: DNS#resolver-lifecycle
        resolver::teardown_resolver(&*self.driver.container, &*self.driver.process).await;
        self.caddy_v4_addr = None;
        self.resolver_addr = None;

        // spec: NAT64#translator-lifecycle
        if self.nat64_installed {
            match crate::system::jool::teardown_nat64().await {
                Ok(()) => self.nat64_installed = false,
                Err(e) => {
                    // Left installed as far as the runtime knows, so the
                    // next idle tick tries the teardown again rather than
                    // concluding the translator is gone, and a non-idle tick
                    // does not redundantly re-setup.
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
        // spec: UPD#rolling-updates
        // Rebuild rolling_updates from scratch each tick so that completed
        // rollouts are automatically cleared.
        self.rolling_updates.clear();
        for (app_name, mut pod_update) in pod_updates {
            // FLT#workload-faults
            self.file_image_pull_faults(&app_name, &pod_update);
            // FLT#workload-faults
            self.file_unit_failure_faults(&app_name, &pod_update);
            // AUTO#restart-record
            // AUTO#crash-loop-detection
            // Restart bookkeeping runs first: it appends rate-derived crash
            // loops to the update so both triggers file through one path.
            self.record_restarts(&app_name, &mut pod_update);
            // FLT#workload-faults
            self.file_crash_loop_faults(&app_name, &pod_update);
            // FLT#workload-faults
            self.file_health_check_faults(&app_name, &pod_update);
            // FLT#workload-faults
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
                // spec: AUTO#job-completion
                self.completed_jobs.remove(&instance.id);
                // Track that this Job was started by the reconciler so that
                // a fast-completing job (which exits before the observer's
                // poll catches a `container_running` observation) is still
                // recognised as having "previously ran" on the next tick.
                if instance.kind == crate::defs::resource::ResourceKind::Job {
                    self.started_jobs.insert(instance.id);
                }
            }
            // spec: AUTO#job-completion
            for completed in &pod_update.completed_job_instances {
                self.completed_jobs.insert(*completed);
                self.started_jobs.remove(completed);
            }
            running_pods_by_app.insert(app_name, pod_update.running);
        }
        self.update_replace_state(apps, &running_pods_by_app);
        running_pods_by_app
    }

    // spec: AUTO#health-check-replacement
    // spec: AUTO#replacement-guard
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

        // spec: AUTO#replacement-guard
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
                // spec: AUTO#replacement-guard
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
        // (b) the count of healthy running instances is below the scale the
        // deployment should be running at, and (c) the replace-loop guard for
        // it has not tripped.
        self.unhealthy_replace_deployments.clear();
        // Per-deployment grace window in seconds, used downstream to detect
        // replacement-failure from observation history.
        let mut grace_secs_by_dep: HashMap<(AppName, String), i64> = HashMap::new();

        // Gather what each deployment looks like this tick first, so the
        // scales can be resolved in one pass over the database below rather
        // than a query per deployment inside the walk.
        let mut seen: Vec<DeploymentThisTick> = Vec::new();
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
                let scale = (dep_def.scale.start, dep_def.scale.end);
                let pod_def = dep_def.pod.lock();
                let container = pod_def.container.lock();
                let healthcheck = container.healthcheck.as_ref().map(|hc| {
                    (
                        hc.on_failure,
                        (hc.start_period_secs + hc.retries as u64 * hc.interval_secs) as i64,
                    )
                });
                drop(container);
                drop(pod_def);
                drop(dep_def);

                let dep_name = id.name.as_str();
                seen.push(DeploymentThisTick {
                    key: (app.name.clone(), dep_name.to_owned()),
                    healthcheck,
                    instance_health: pods
                        .iter()
                        .filter(|p| {
                            p.instance.kind == crate::defs::resource::ResourceKind::Deployment
                                && p.instance.name.as_deref() == Some(dep_name)
                        })
                        .map(|p| p.observed_healthy)
                        .collect(),
                    scale,
                });
            }
        }

        // spec: AUTO#scale
        // How many healthy instances a deployment is owed is the scale the
        // operator chose, not the declared lower bound. Read against the
        // bound, a deployment declaring `scale 1..5` and scaled to 5 that
        // loses an instance has four healthy against a target of one, and is
        // never found short — so the further it was scaled up, the less
        // likely it was to get a replacement at all.
        // One statement for the node, not one per deployment: `seen` spans
        // every app, so a query each would be a round trip per deployment per
        // tick on top of the one `compute_effective_scales` already makes.
        let read_targets: rusqlite::Result<Vec<u16>> = {
            let wanted: Vec<(AppName, String, u16, u16)> = seen
                .iter()
                .map(|d| (d.key.0.clone(), d.key.1.clone(), d.scale.0, d.scale.1))
                .collect();
            self.db.call(move |db| {
                let decisions = scaling::load_all_scaling_decisions(db)?;
                Ok(wanted
                    .into_iter()
                    .map(|(app, dep, low, high)| {
                        let stored = decisions.get(&(app, dep)).copied();
                        scaling::effective_scale_from(stored, low, high)
                    })
                    .collect())
            })
        };
        // spec: AUTO#unreadable-scale
        // Carried as `None` rather than as the lower bound. Read against the
        // bound a deployment scaled above it is never short, so substituting
        // one would withhold every replacement while reading as a decision to
        // make none. The guard below still runs: it bounds the churn of
        // replacements already in flight, which this says nothing about.
        let targets: Vec<Option<u16>> = match read_targets {
            Ok(targets) => targets.into_iter().map(Some).collect(),
            Err(e) => {
                error!(
                    error = %e,
                    "could not read the scaling decisions; leaving replacement state as it \
                     stands this tick rather than acting on a shortfall nobody measured"
                );
                vec![None; seen.len()]
            }
        };

        for (dep, target) in seen.into_iter().zip(targets) {
            let standing = deployment_standing(dep.healthcheck, &dep.instance_health, target);
            if let Some(grace_secs) = standing.grace_secs {
                grace_secs_by_dep.insert(dep.key.clone(), grace_secs);
            }
            if standing.wants_replacement && !self.replace_failed.contains(&dep.key) {
                self.unhealthy_replace_deployments.insert(dep.key);
            }
        }

        // spec: AUTO#replacement-guard
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
            // spec: TRD#uninstall-scope — match units against the
            // identities the registry recorded, not against a name prefix.
            // `seedling-{app}-` is not prefix-free: uninstalling `app` matched
            // every unit of a sibling called `app-db`, and the retry branch
            // then reset and stopped that healthy sibling's units every five
            // seconds while this uninstall never completed.
            let app_name_for_query = app.name.clone();
            let expected: HashSet<String> = match self.db.call(move |db| {
                crate::runtime::history::display_names_for_app(db, &app_name_for_query)
            }) {
                Ok(names) => names
                    .into_iter()
                    .map(|display_name| format!("seedling-{display_name}.service"))
                    .collect(),
                // Fail safe: an empty expected set makes every unit invisible,
                // which would read as "teardown finished" and delete the
                // registry rows while the units are still loaded. Wait for a
                // tick where the identities can actually be read.
                Err(e) => {
                    warn!(
                        app = %app.name,
                        "uninstall: could not load recorded instance names; \
                         not advancing teardown this tick: {e}"
                    );
                    continue;
                }
            };
            // An empty recorded set is ambiguous in exactly the way this
            // change is about. It is the truth for an app that was registered
            // but never scheduled, and it is also what a GC sweep leaves
            // behind if it reaps the rows while units are still loaded — and
            // an empty `expected` filters every unit away, which the branch
            // below reads as "teardown finished". Distinguish the two by
            // asking whether anything that could be ours is still loaded.
            // `seedling-{app}-` is not prefix-free, so it may over-match a
            // sibling; over-matching only makes this refuse to conclude,
            // which is the safe direction.
            if expected.is_empty() {
                match self.driver.process.list_units("seedling-").await {
                    Ok(units) if !units_possibly_of(&units, &app.name).is_empty() => {
                        tracing::error!(
                            app = %app.name,
                            "uninstall: units may still be loaded for this app but no recorded \
                             identities remain to match them against; not advancing teardown"
                        );
                        let app_name_owned = app.name.clone();
                        self.db.call(move |db| {
                            let _ = crate::runtime::faults::file_once(
                                db,
                                &crate::runtime::faults::FaultKey::new(
                                    &app_name_owned,
                                    "uninstall_unidentifiable_units",
                                    "",
                                ),
                                &crate::runtime::faults::FaultMeta::default(),
                                "units remain under this app's prefix but no recorded instance \
                                 identities remain to match them against, so teardown cannot be \
                                 safely completed",
                            );
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(
                            app = %app.name,
                            "uninstall: could not list units to corroborate an empty recorded \
                             set; not advancing teardown this tick: {e}"
                        );
                        continue;
                    }
                }
            }
            // The prefix scan only enumerates candidates; the decision is the
            // exact match against `expected`.
            match self
                .driver
                .process
                .list_units("seedling-")
                .await
                .map(|units| {
                    units
                        .into_iter()
                        .filter(|u| expected.contains(&u.name))
                        .collect::<Vec<_>>()
                }) {
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
                        if let Err(e) = crate::runtime::history::delete_instances_for_app(
                            db,
                            &app_name_owned,
                        ) {
                            warn!(app = %app_name_owned, "failed to clean up resource instances during uninstall: {e}");
                        }
                        // The other half of `uninstall_unidentifiable_units`:
                        // teardown finishing is the only thing that resolves
                        // it, and it must not outlive the app it describes.
                        let _ = crate::runtime::faults::clear_faults_by_kind(
                            db,
                            &app_name_owned,
                            "uninstall_unidentifiable_units",
                        );
                    });
                    // spec: EVT#event-types
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

/// One deployment as this tick found it, gathered before any decision so the
/// scales can be resolved together.
struct DeploymentThisTick {
    key: (AppName, String),
    /// The declared healthcheck's failure policy and grace window in seconds.
    healthcheck: Option<(crate::defs::container::HealthcheckOnFailure, i64)>,
    /// Whether each running instance of this deployment is observed healthy.
    instance_health: Vec<bool>,
    /// The declared bounds, which the stored scaling decision is clamped to.
    scale: (u16, u16),
}

/// What the replace-loop guard and the replacement bump each need to know
/// about one deployment.
struct DeploymentStanding {
    /// How long a fresh instance of this deployment has to become healthy.
    /// `None` where the deployment declares no healthcheck, which is also
    /// what puts it outside the guard.
    grace_secs: Option<i64>,
    /// The deployment replaces unhealthy instances, and is short of healthy
    /// ones against what it should be running.
    wants_replacement: bool,
}

/// Decide both of a deployment's standings together, so the two questions
/// stay apart.
///
/// The grace window follows the declared healthcheck: the guard watches every
/// fresh instance, whether a replacement or a rolling update brought it up,
/// so any deployment declaring a healthcheck has one whatever its policy.
/// Wanting a replacement follows the policy as well, and the observed health
/// of the instances running against the count that should be.
///
/// Reading both off the policy left a rolling update of a deployment with a
/// healthcheck but `on_failure: monitor` with no grace window at all, so the
/// guard never evaluated it and a stuck update inhibited its stale instances
/// for good.
// spec: AUTO#health-check-replacement
// spec: AUTO#replacement-guard
fn deployment_standing(
    healthcheck: Option<(crate::defs::container::HealthcheckOnFailure, i64)>,
    instance_health: &[bool],
    target: Option<u16>,
) -> DeploymentStanding {
    let Some((on_failure, grace_secs)) = healthcheck else {
        return DeploymentStanding {
            grace_secs: None,
            wants_replacement: false,
        };
    };
    // spec: AUTO#unreadable-scale
    // A deployment whose target could not be read is not a deployment known to
    // have enough: how many it is owed is simply unanswered, and spawning a
    // replacement against a guess is the churn the guard exists to bound. Its
    // grace window is declared in the app definition, so that much is known
    // either way.
    let Some(target) = target else {
        return DeploymentStanding {
            grace_secs: Some(grace_secs),
            wants_replacement: false,
        };
    };
    let healthy = instance_health.iter().filter(|h| **h).count();
    DeploymentStanding {
        grace_secs: Some(grace_secs),
        wants_replacement: matches!(
            on_failure,
            crate::defs::container::HealthcheckOnFailure::Replace
        ) && instance_health.contains(&false)
            && healthy < usize::from(target),
    }
}

/// Apply this tick's proxy config, or `None` when the build did not cover
/// every app and the apply is withheld.
///
/// A config with no virtual hosts and no layer-4 routes is applied like any
/// other. It is the computed answer — no ingress is declared — and the
/// coverage check is what tells it apart from a build that could not compute
/// one. Skipping it left whatever the proxy had last been given in force, so a
/// deleted ingress stayed reachable.
// spec: RCN#applying-absolute-state
async fn apply_proxy_plane(
    proxy: &dyn super::NetworkProxy,
    config: &crate::system::types::ProxyConfig,
    coverage: Coverage,
) -> Option<Result<(), super::BoxError>> {
    match coverage.is_complete() {
        true => Some(proxy.apply_config(config).await),
        false => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Coverage, DeploymentStanding, apply_proxy_plane, deployment_standing};
    use crate::defs::container::HealthcheckOnFailure;
    use crate::system::{BoxError, BoxFuture, NetworkProxy, types::ProxyConfig};

    #[derive(Default)]
    struct RecordingProxy(parking_lot::Mutex<Vec<bool>>);

    impl NetworkProxy for RecordingProxy {
        fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
            Box::pin(async { Ok(true) })
        }

        fn apply_config<'a>(
            &'a self,
            config: &'a ProxyConfig,
        ) -> BoxFuture<'a, Result<(), BoxError>> {
            self.0.lock().push(config.virtual_hosts.is_empty());
            Box::pin(async { Ok(()) })
        }
    }

    // spec: RCN#applying-absolute-state
    // Removing the last ingress produces an empty config. It used to be skipped
    // and reported as a success, so the proxy kept serving the ingress that was
    // deleted.
    #[tokio::test]
    async fn an_empty_proxy_config_is_applied() {
        let proxy = RecordingProxy::default();

        let outcome = apply_proxy_plane(&proxy, &ProxyConfig::default(), Coverage::Complete).await;

        assert!(matches!(outcome, Some(Ok(()))));
        assert_eq!(*proxy.0.lock(), vec![true], "applied once, and empty");
    }

    // spec: RCN#applying-absolute-state
    // An empty config from a build that could not cover every app is not the
    // answer "nothing to serve", and must leave the previous state standing.
    #[tokio::test]
    async fn a_partial_build_is_withheld_whatever_it_holds() {
        let proxy = RecordingProxy::default();

        let outcome = apply_proxy_plane(&proxy, &ProxyConfig::default(), Coverage::Partial).await;

        assert!(outcome.is_none(), "withheld, so no conclusion is drawn");
        assert!(proxy.0.lock().is_empty(), "nothing was pushed");
    }

    fn standing(
        on_failure: HealthcheckOnFailure,
        health: &[bool],
        target: u16,
    ) -> DeploymentStanding {
        deployment_standing(Some((on_failure, 90)), health, Some(target))
    }

    // spec: AUTO#replacement-guard
    // The grace window was recorded only after the on_failure=replace filter,
    // and the guard's candidate list is looked up through that map. A rolling
    // update of a deployment with a healthcheck but on_failure=monitor had no
    // entry, so the guard never evaluated it and a stuck update inhibited its
    // stale instances indefinitely.
    #[test]
    fn a_healthcheck_earns_a_grace_window_whatever_the_policy() {
        assert_eq!(
            standing(HealthcheckOnFailure::Monitor, &[true], 1).grace_secs,
            Some(90),
            "a monitor deployment is still watched by the guard"
        );
        assert_eq!(
            standing(HealthcheckOnFailure::Replace, &[true], 1).grace_secs,
            Some(90)
        );
        assert_eq!(
            deployment_standing(None, &[true], Some(1)).grace_secs,
            None,
            "no healthcheck, nothing to wait for"
        );
    }

    // spec: AUTO#health-check-replacement
    // Only `replace` spawns a replacement; `monitor` keeps its routing-pool
    // preference but is never bumped.
    #[test]
    fn only_a_replace_policy_wants_a_replacement() {
        assert!(standing(HealthcheckOnFailure::Replace, &[true, false], 2).wants_replacement);
        assert!(!standing(HealthcheckOnFailure::Monitor, &[true, false], 2).wants_replacement);
        assert!(
            !standing(HealthcheckOnFailure::Replace, &[true, true], 2).wants_replacement,
            "nothing unhealthy to replace"
        );
    }

    // spec: AUTO#unreadable-scale
    // The lower bound carried two wrong answers at once: a scaled-up
    // deployment never looked short, and one sitting at its floor looked
    // adequately staffed for a reason nobody had checked. Unknown is carried
    // as unknown, and nothing is replaced on a guess.
    #[test]
    fn an_unreadable_target_asks_for_no_replacement() {
        let unknown = deployment_standing(
            Some((HealthcheckOnFailure::Replace, 90)),
            &[true, false],
            None,
        );
        assert!(
            !unknown.wants_replacement,
            "the shortfall is unmeasured, not measured as nil"
        );
        assert_eq!(
            unknown.grace_secs,
            Some(90),
            "the window is declared in the app definition, so it is known either way"
        );
        assert!(
            standing(HealthcheckOnFailure::Replace, &[true, false], 2).wants_replacement,
            "the same deployment with a readable target is owed a replacement"
        );
    }

    // spec: AUTO#scale
    // The threshold was the declared lower bound while the running instances
    // follow the operator's effective scale. A deployment declaring
    // `scale 1..5` and scaled to 5 that loses one instance has four healthy
    // against a target of one, so `healthy < target` was false and no
    // replacement was spawned — the more it was scaled up, the less likely it
    // was to get one.
    #[test]
    fn a_scaled_up_deployment_is_short_against_the_scale_it_runs_at() {
        let health = [true, true, true, true, false];
        assert!(
            standing(HealthcheckOnFailure::Replace, &health, 5).wants_replacement,
            "scaled to 5, one unhealthy: a replacement is owed"
        );
        assert!(
            !standing(HealthcheckOnFailure::Replace, &health, 1).wants_replacement,
            "against the declared lower bound it looks amply covered, which was the bug"
        );
    }

    // spec: OBS#recording
    // `OBS_KINDS` exists to intern what `to_obs_kinds` persists, so a kind in
    // one and not the other silently defeats the restart seed: the row is
    // read from the DB, fails to intern, and the observation is re-written
    // with a fresh timestamp on every boot. Three kinds had drifted out.
    #[test]
    fn every_persisted_obs_kind_can_be_interned() {
        use crate::system::types::{ObservationFact, UnitExit};

        let facts = [
            ObservationFact::ContainerMissing,
            ObservationFact::ContainerCreated,
            ObservationFact::ContainerRunning { pid: 1 },
            ObservationFact::ContainerExited { exit_code: 0 },
            ObservationFact::ContainerHealthy,
            ObservationFact::ContainerUnhealthy,
            ObservationFact::ContainerPresentIndeterminate,
            ObservationFact::ContainerSpecHash("h".into()),
            ObservationFact::NetworkPresent,
            ObservationFact::NetworkMissing,
            ObservationFact::VolumePresent,
            ObservationFact::VolumeMissing,
            ObservationFact::VolumeBackendMismatch,
            ObservationFact::UnitActive,
            ObservationFact::UnitActivating,
            ObservationFact::UnitInactive,
            ObservationFact::UnitFailed,
            ObservationFact::UnitStartLimitHit,
            ObservationFact::UnitGone,
            ObservationFact::UnitRestartCounter {
                count: 1,
                exit: None::<UnitExit>,
            },
            ObservationFact::ProxyReachable,
            ObservationFact::ProxyUnreachable,
            ObservationFact::RoutePresent {
                hostname: "h".into(),
            },
            ObservationFact::RouteAbsent {
                hostname: "h".into(),
            },
        ];

        for fact in &facts {
            for (kind, _payload) in fact.to_obs_kinds() {
                assert!(
                    OBS_KINDS.contains(&kind),
                    "{kind:?} is persisted but missing from OBS_KINDS, so the \
                     restart seed cannot intern it",
                );
            }
        }
    }

    fn unit(name: &str) -> UnitSummary {
        UnitSummary {
            name: name.to_owned(),
            state: Default::default(),
        }
    }

    // spec: TRD#uninstall-scope
    #[test]
    fn no_recorded_identities_with_units_still_loaded_is_not_teardown_finished() {
        let units = [
            unit("seedling-app-web.service"),
            unit("seedling-other-web.service"),
        ];
        let app = AppName::new_unchecked("app");
        assert!(
            !units_possibly_of(&units, &app).is_empty(),
            "with no recorded identities left, a loaded unit under the app's own prefix is the \
             only remaining evidence that teardown has not finished; treating the empty record \
             set as completion deletes the rows while the unit is still loaded"
        );
    }

    // spec: TRD#uninstall-scope
    #[test]
    fn no_recorded_identities_and_no_units_is_genuinely_nothing_to_tear_down() {
        let units = [unit("seedling-other-web.service")];
        let app = AppName::new_unchecked("app");
        assert!(
            units_possibly_of(&units, &app).is_empty(),
            "an app registered but never scheduled has no rows and no units, and must still be \
             able to finish uninstalling"
        );
    }

    // spec: TRD#uninstall-scope
    // spec: PRIO#resource-control-groups
    // The runtime's own slices sit under the same prefix and outlive the
    // workloads in them, so counting one as a live workload would strand an app
    // that has no recorded instances in uninstalling forever.
    #[test]
    fn the_runtimes_own_slices_are_not_mistaken_for_live_workloads() {
        let units = [
            unit("seedling-app.slice"),
            unit("seedling-app-critical.slice"),
            unit("seedling-app-normal.slice"),
        ];
        let app = AppName::new_unchecked("app");
        assert!(
            units_possibly_of(&units, &app).is_empty(),
            "slices are the grouping, not the workload; an app whose units are all gone must \
             still be able to finish uninstalling while its slices are still loaded"
        );
    }

    // spec: TRD#uninstall-scope
    #[test]
    fn the_candidate_scan_may_over_match_a_sibling_but_only_towards_caution() {
        let units = [unit("seedling-app-db-web.service")];
        let app = AppName::new_unchecked("app");
        assert!(
            !units_possibly_of(&units, &app).is_empty(),
            "`seedling-app-` also matches `app-db`; over-matching here only withholds a \
             completion, never stops a sibling's unit — the decision itself is still the exact \
             match against recorded identity"
        );
    }
    use std::collections::HashMap;

    use crate::runtime::registry::{RegistryError, ScaledGroup};

    use super::*;

    /// A registry that fails for one named app and succeeds for every other.
    ///
    /// The stub `System` cannot make a registry lookup fail, which is why the
    /// existing tests all passed while the reconciler withdrew a live app's
    /// data plane on a transient error.
    struct RegistryFailingFor {
        broken: AppName,
        inner: crate::runtime::registry::EphemeralInstanceRegistry,
    }

    impl InstanceRegistry for RegistryFailingFor {
        fn get_or_create_singleton(
            &self,
            app: &AppName,
            kind: crate::defs::resource::ResourceKind,
            name: Option<&str>,
        ) -> Result<crate::runtime::identity::ResourceInstance, RegistryError> {
            if app == &self.broken {
                return Err(RegistryError::message("registry unavailable"));
            }
            self.inner.get_or_create_singleton(app, kind, name)
        }

        fn ensure_scaled_group(
            &self,
            app: &AppName,
            kind: crate::defs::resource::ResourceKind,
            name: Option<&str>,
            count: u16,
            reuse: crate::runtime::registry::InstanceReuse,
        ) -> Result<ScaledGroup, RegistryError> {
            if app == &self.broken {
                return Err(RegistryError::message("registry unavailable"));
            }
            self.inner
                .ensure_scaled_group(app, kind, name, count, reuse)
        }

        fn find_all_instances(
            &self,
            app: &AppName,
            kind: crate::defs::resource::ResourceKind,
            name: Option<&str>,
        ) -> Result<Vec<crate::runtime::identity::ResourceInstance>, RegistryError> {
            if app == &self.broken {
                return Err(RegistryError::message("registry unavailable"));
            }
            self.inner.find_all_instances(app, kind, name)
        }
    }

    fn snapshot_with_service(name: &str) -> AppSnapshot {
        let app_name = AppName::new(name).unwrap();
        let (app, err) = crate::runtime::apps::evaluate_script(
            &app_name,
            r#"app.service("api").http(8080);"#,
            &std::collections::BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "test script must evaluate: {err:?}");
        let app_def = (*app.def.load_full()).clone();
        AppSnapshot {
            name: app_name,
            desired: DesiredState::default(),
            steady: true,
            app_def,
            phase: AppPhase::Installed,
            phase_handle: Arc::new(Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            current_generation: 1,
            app_priority: AppPriority::default(),
        }
    }

    fn snapshot_with_priorities(
        name: &str,
        app_priority: AppPriority,
        script: &str,
    ) -> AppSnapshot {
        let app_name = AppName::new(name).unwrap();
        let (app, err) = crate::runtime::apps::evaluate_script(
            &app_name,
            script,
            &std::collections::BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "test script must evaluate: {err:?}");
        let app_def = (*app.def.load_full()).clone();
        AppSnapshot {
            name: app_name,
            desired: DesiredState::default(),
            steady: true,
            app_def,
            phase: AppPhase::Installed,
            phase_handle: Arc::new(Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            current_generation: 1,
            app_priority,
        }
    }

    // spec: PRIO#resource-control-groups
    #[test]
    fn each_declared_level_gets_a_tier_slice_under_its_app() {
        let apps = vec![snapshot_with_priorities(
            "demo",
            AppPriority::Normal,
            r#"
            app.deployment("api")
                .image("docker.io/library/nginx:1")
                .priority(Priority.Critical);
            app.deployment("batch")
                .image("docker.io/library/nginx:1")
                .priority(Priority.Low);
            "#,
        )];
        let slices = desired_slices(&apps);

        assert!(slices.contains_key("seedling-demo.slice"));
        assert!(slices.contains_key("seedling-demo-critical.slice"));
        assert!(slices.contains_key("seedling-demo-low.slice"));
        // Always present: Jobs and other non-Deployment workloads stand here.
        assert!(slices.contains_key("seedling-demo-normal.slice"));
        // Not declared by any Deployment, so not created. Forgetting a level
        // that stops being declared is safe because removal unlinks the unit
        // file and leaves anything still running in the slice alone.
        assert!(!slices.contains_key("seedling-demo-elevated.slice"));
    }

    // spec: PRIO#contended-capacity
    #[test]
    fn the_app_slice_carries_the_app_weight_and_the_tier_slice_the_tier_weight() {
        let apps = vec![snapshot_with_priorities(
            "demo",
            AppPriority::High,
            r#"
            app.deployment("api")
                .image("docker.io/library/nginx:1")
                .priority(Priority.Critical);
            "#,
        )];
        let slices = desired_slices(&apps);

        let app_slice = &slices["seedling-demo.slice"];
        assert_eq!(app_slice.weight, priority::app_weight(AppPriority::High));
        assert_eq!(app_slice.weight, priority::app_weight(AppPriority::High));

        let tier = &slices["seedling-demo-critical.slice"];
        assert_eq!(tier.weight, priority::deployment_weight(Priority::Critical));
    }

    // spec: PRIO#priority-setting
    // The same definition at a different app priority must produce a different
    // app-slice weight: that difference is what a reweight applies.
    #[test]
    fn changing_the_app_priority_changes_the_app_slice_weight() {
        let script = r#"app.deployment("api").image("docker.io/library/nginx:1");"#;
        let high = desired_slices(&[snapshot_with_priorities("demo", AppPriority::High, script)]);
        let low = desired_slices(&[snapshot_with_priorities("demo", AppPriority::Low, script)]);
        assert_ne!(
            high["seedling-demo.slice"].weight,
            low["seedling-demo.slice"].weight
        );
        // The tier slices are unchanged: the app term lives on the app slice.
        assert_eq!(
            high["seedling-demo-normal.slice"].weight,
            low["seedling-demo-normal.slice"].weight
        );
    }

    // spec: PRIO#kill-order
    #[test]
    fn the_infra_slice_is_always_present_and_outweighs_every_app() {
        let slices = desired_slices(&[]);
        let infra = &slices[priority::INFRA_SLICE];
        assert_eq!(infra.weight, priority::INFRA_WEIGHT);

        let apps = vec![snapshot_with_priorities(
            "demo",
            AppPriority::High,
            r#"app.deployment("api").image("docker.io/library/nginx:1");"#,
        )];
        let with_app = desired_slices(&apps);
        assert!(with_app[priority::INFRA_SLICE].weight > with_app["seedling-demo.slice"].weight);
    }

    // spec: PRIO#resource-control-groups
    // Registering an app under the infrastructure name is refused, but an app
    // that predates that reservation still reaches this derivation, and its
    // slice name is byte-identical to the infrastructure slice. The
    // infrastructure entry has to win, or the group holding the proxy and the
    // resolver takes the app's standing.
    #[test]
    fn an_app_named_for_the_infra_slice_cannot_displace_it() {
        let apps = vec![snapshot_with_priorities(
            priority::INFRA_COMPONENT,
            AppPriority::Low,
            r#"app.deployment("api").image("docker.io/library/nginx:1");"#,
        )];
        let slices = desired_slices(&apps);
        assert_eq!(
            slices[priority::INFRA_SLICE].weight,
            priority::INFRA_WEIGHT,
            "infrastructure keeps its own weight, not the app's"
        );
    }

    // spec: PRIO#resource-control-groups
    // Two apps whose names differ only by a hyphen must not share a slice, nor
    // nest one inside the other.
    #[test]
    fn hyphenated_app_names_get_their_own_slice_trees() {
        let script = r#"app.deployment("api").image("docker.io/library/nginx:1");"#;
        let apps = vec![
            snapshot_with_priorities("app", AppPriority::Normal, script),
            snapshot_with_priorities("app-two", AppPriority::High, script),
        ];
        let slices = desired_slices(&apps);
        assert!(slices.contains_key("seedling-app.slice"));
        assert!(slices.contains_key("seedling-app_two.slice"));
        assert_ne!(
            slices["seedling-app.slice"].weight,
            slices["seedling-app_two.slice"].weight
        );
    }

    // spec: RCN#applying-absolute-state
    // Routes are applied wholesale, so an app dropped from the computed set
    // has its live routes deleted. The tick has to know the set is short.
    #[test]
    fn routes_report_partial_coverage_when_an_app_is_dropped() {
        let apps = vec![
            snapshot_with_service("healthy"),
            snapshot_with_service("broken"),
        ];
        let registry = RegistryFailingFor {
            broken: AppName::new("broken").unwrap(),
            inner: crate::runtime::registry::EphemeralInstanceRegistry::new(),
        };
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &empty_lookup,
        };

        let build = phases::compute_routes(
            &apps,
            &HashMap::new(),
            &"fd5e:ed11:9000::/48".parse().unwrap(),
            &registry,
            &crate::runtime::external_service_mappings::ExternalServiceSnapshot::default(),
            &resolve_ctx,
        );

        assert_eq!(
            build.coverage,
            Coverage::Partial,
            "a dropped app must not look like a complete route set"
        );
        assert_eq!(
            build.routes.len(),
            1,
            "the healthy app still contributes its route"
        );
    }

    // spec: RCN#applying-absolute-state
    #[test]
    fn routes_report_complete_coverage_when_every_app_builds() {
        let apps = vec![
            snapshot_with_service("healthy"),
            snapshot_with_service("other"),
        ];
        let registry = RegistryFailingFor {
            broken: AppName::new("nobody").unwrap(),
            inner: crate::runtime::registry::EphemeralInstanceRegistry::new(),
        };
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &empty_lookup,
        };

        let build = phases::compute_routes(
            &apps,
            &HashMap::new(),
            &"fd5e:ed11:9000::/48".parse().unwrap(),
            &registry,
            &crate::runtime::external_service_mappings::ExternalServiceSnapshot::default(),
            &resolve_ctx,
        );

        assert_eq!(build.coverage, Coverage::Complete);
        assert_eq!(build.routes.len(), 2);
    }

    // spec: RCN#applying-absolute-state
    // The whole point of the distinction: an empty app set means "nothing is
    // installed" only when nothing was dropped getting there.
    #[test]
    fn coverage_distinguishes_idle_from_dropped() {
        assert!(Coverage::of(0).is_complete());
        assert!(!Coverage::of(1).is_complete());
        assert!(!Coverage::Complete.and(Coverage::Partial).is_complete());
        assert!(Coverage::Complete.and(Coverage::Complete).is_complete());
    }
}
