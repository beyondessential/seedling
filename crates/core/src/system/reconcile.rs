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
    defs::{
        app::AppDef,
        enums::Priority,
        resource::{Resource, ResourceKind},
    },
    oi::shells::ShellRegistry,
    runtime::{
        AppPhase, InstanceRegistry,
        apps::{AppRegistry, transition_phase},
        db::DbHandle,
        desired::{DesiredState, EffectiveScales, compute, compute_uninstalling, merge_sessions},
        gates::{GateRuntime, GateView},
        gc::InstanceGcConfig,
        health::InstanceHealth,
        identity::InstanceId,
        lifecycle::LifecycleState,
        priority::AppPriority,
        scaling,
        secrets::Cipher,
        site_services::{resolve::ResolveCtx, resolver::SiteServiceResolver},
        stopped,
    },
    system::{
        System,
        actuator::Actuator,
        caddy,
        observer::Observer,
        priority::AppStandings,
        resolver,
        types::{DataPlaneRules, UnitSummary},
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

/// The subject of the `packet_filter_failed` fault for the isolation rules,
/// beside the one for the rules held as absolute state, which has none.
const ISOLATION_FAULT_SUBJECT: &str = "isolation";

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
    "health_check_starting",
    "unit_start_limit_hit",
    "volume_backend_mismatch",
    "image_pull_started",
    "stop_sent",
    "unit_failed",
    "unit_inactive",
    "volume_created",
    "volume_ready",
    "volume_removed",
    "volume_cleaned_up",
    "network_created",
    "backend_healthy",
    "backend_lost",
    "network_removed",
    "network_cleaned_up",
    "ingress_configured",
    "ingress_ready",
    "ingress_removed",
    "ingress_cleaned_up",
    "cert_valid",
    "cert_not_valid",
    "cert_status_blocked",
    "cert_status_expired",
    "cert_status_error",
    "cert_status_pending",
    "cert_status_active",
    "cert_status_none",
];

/// Load the `(instance_id, obs_kind)` pairs still on record from the DB so that
/// observations persisted in a previous session are not re-written with a
/// fresh timestamp on restart.
///
/// A kind that a later recorded kind contradicts is not on record as the fact
/// last recorded for the instance, so a start-up that read a health result that
/// had failed and then passed would otherwise take the failure for recorded and
/// miss its recurrence.
// spec: OBS#recording
fn seed_written_obs(db: &DbHandle) -> HashSet<(InstanceId, &'static str)> {
    fn intern(s: &str) -> Option<&'static str> {
        OBS_KINDS.iter().find(|&&k| k == s).copied()
    }

    db.call(|db| {
        let mut set = HashSet::new();
        let Ok(mut stmt) = db.conn.prepare(
            "SELECT instance_id, obs_kind FROM world_observations \
             GROUP BY instance_id, obs_kind ORDER BY MAX(id)",
        ) else {
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
            let id = InstanceId(uuid::Uuid::from_u128(n));
            for other in Reconciler::contradicting_kinds(kind) {
                set.remove(&(id, *other));
            }
            set.insert((id, kind));
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

    /// `Partial` when a snapshot every contribution draws on could not be
    /// read, which `None` stands for.
    fn of_read<T>(snapshot: Option<&T>) -> Self {
        match snapshot {
            Some(_) => Self::Complete,
            None => Self::Partial,
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
    app_def: Arc<AppDef>,
    /// `app_def` with each resource an operation placed as the operation's
    /// definition declares it: what the proxy routes, the tier slices and the
    /// healthcheck pass read.
    // spec: OPS#changes-to-an-app
    declarations: Arc<AppDef>,
    phase: AppPhase,
    phase_handle: Arc<Mutex<AppPhase>>,
    /// Hostnames whose certs should be pre-warmed. Captured from the in-flight
    /// `OperationProgress`, if any.
    // spec: RT#certificate-warming
    warm_cert_hostnames: std::collections::BTreeSet<String>,
    /// The operator-set app priority, read fresh this tick so a change takes
    /// effect without a redeploy.
    // spec: PRIO#priority-setting
    app_priority: AppPriority,
    /// The digests of the keys of the gates of each definition `desired`
    /// resolves its workloads against, which the spec hash of a workload
    /// ordered after one of them carries.
    // spec: UPD#spec-hash
    gate_keys: pods::GateKeys,
    /// What the app's gates hold, and which of its workloads wait on a target.
    // spec: RCN#gate-holds
    // spec: GATE#start-order
    gates: GateView,
    /// Where the app's install stands in the order installs completed.
    // spec: ING#claims-across-apps
    install_seq: Option<i64>,
}

/// Single global reconciler that processes all installed apps each tick.
pub struct Reconciler {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    observer: Observer,
    actuator: Actuator,
    caddy_admin_client: Arc<AsyncRwLock<reqwest::Client>>,
    caddy_v4_addr: Option<Ipv4Addr>,
    /// Whether the `caddy_failed` fault on file is for a rejected proxy
    /// upgrade, which clears only when an upgrade succeeds.
    // spec: FLT#host-faults
    caddy_upgrade_rejected: bool,
    data_dir: PathBuf,
    db: DbHandle,
    /// The host's secret key, under which gate keys are digested.
    cipher: Arc<Cipher>,
    /// What the runtime remembers of its gates' checks.
    // spec: GRUN#checks
    gate_runtime: Arc<GateRuntime>,
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
    prev_health: BTreeMap<(AppName, String), Option<InstanceHealth>>,
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
    /// Paces retries when the supervisor refuses the slice set, so a host with
    /// a read-only unit directory is not asked to re-read its units every tick.
    /// One gate, not one per slice: the set is synced as a whole.
    // spec: PRIO#resource-control-groups
    slice_retry: crate::runtime::retry::RetryGate,
    /// Whether seedling is providing its own NAT64 translator.
    nat64_active: bool,
    /// What the reconciler knows of the jool translator instance on this host.
    /// The reconciler sets it up when an app is installed, and
    /// tears it down when none is.
    // spec: NAT64#translator-lifecycle
    nat64_translator: Nat64Translator,
    /// Whether the DNS64 plugin should translate names that already
    /// have a real AAAA record (emits `translate_all` into the
    /// Corefile). Set when NAT64 is seedling-managed and the host has
    /// no IPv6 egress — forcing every flow through the translator is
    /// the only way the pod can reach dual-stack remotes.
    // spec: NAT64#dns64
    force_dns64_translation: bool,
    /// The host facts that may be undetermined, and are looked for again on
    /// each iteration until they are.
    // spec: NAT64#detection
    // spec: NAT64#ipv6-egress
    host_probes: crate::system::host_probes::HostProbes,
    /// Whether the faults for `host_probes` have been brought up to date since
    /// they last moved.
    host_faults_synced: bool,
    /// A host fact has been determined since the daemon last asked, so the
    /// installed apps' definitions are due to be evaluated again.
    // spec: CONST#determining-host-constants
    host_facts_changed: bool,
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
    /// Apps whose uninstall this reconciler has filed
    /// `uninstall_unidentifiable_units` for. Such a fault that is no longer on
    /// file was cleared by an operator, who has thereby said to finish the
    /// teardown; it is not filed again for that uninstall.
    // spec: TRD#uninstall-scope
    unidentifiable_units_filed: HashSet<AppName>,
    /// Uninstalling apps whose recorded identities this iteration has read,
    /// with no read of them failing so far. Their
    /// `teardown_identities_unreadable` is cleared when the iteration ends, so
    /// that a read failing later in the same iteration leaves it filed rather
    /// than clearing it and filing it again.
    // spec: TRD#uninstall-scope
    teardown_identities_read: HashSet<AppName>,
}

/// Workload units that could belong to `app`, as candidates only: judged by
/// the shape of their names, less each one that exactly matches an identity
/// recorded for another app.
///
/// `seedling-{app}-` is not prefix-free, so `app-db`'s units land among the
/// shapes, and `ruled_out` (the unit names of every other app's recorded
/// instances) is what takes them out again. It exists for the one question the
/// app's own recorded identities cannot answer when there are none of them:
/// is there anything at all that might still be this app's?
///
/// Restricted to services because the question is whether a *workload* is still
/// loaded. The runtime also owns slices under this same prefix
/// (`seedling-{app}-{tier}.slice`), and those outlive every workload in them by
/// design, so counting one as evidence of a live workload would leave an app
/// with no recorded instances unable to ever finish tearing down.
// spec: TRD#uninstall-scope
fn units_possibly_of<'a>(
    units: &'a [UnitSummary],
    app: &AppName,
    ruled_out: &HashSet<String>,
) -> Vec<&'a UnitSummary> {
    let prefix = format!("seedling-{app}-");
    units
        .iter()
        .filter(|u| u.name.ends_with(".service") && u.name.starts_with(&prefix))
        .filter(|u| !ruled_out.contains(&u.name))
        .collect()
}

impl Reconciler {
    /// This reconciler, for a host with facts still to determine: it looks for
    /// them again on each iteration, and files a fault for each until it finds
    /// them.
    // spec: NAT64#detection
    // spec: NAT64#ipv6-egress
    pub fn with_host_probes(mut self, probes: crate::system::host_probes::HostProbes) -> Self {
        self.force_dns64_translation = self.force_dns64_for(&probes);
        self.host_probes = probes;
        self
    }

    /// Whether the resolver translates every name: NAT64 is this runtime's own
    /// and the host has no IPv6 egress, which it is taken to lack while that is
    /// undetermined.
    // spec: NAT64#dns64
    /// Whether site service endpoints are routed as for a host with IPv6
    /// egress: only once it is determined that the host has it, as the
    /// resolver synthesises for undetermined egress.
    // spec: SITE#endpoint-addressing
    fn routes_with_ipv6_egress(&self) -> bool {
        self.host_probes.ipv6_egress.determined().unwrap_or(false)
    }

    fn force_dns64_for(&self, probes: &crate::system::host_probes::HostProbes) -> bool {
        self.nat64_active && !probes.ipv6_egress.determined().unwrap_or(false)
    }

    /// Whether a host fact was determined since this was last asked, and so
    /// the installed apps' definitions are due to be evaluated again.
    pub fn take_host_facts_changed(&mut self) -> bool {
        std::mem::take(&mut self.host_facts_changed)
    }

    /// Look again for the host facts that are undetermined, and hold a fault
    /// for each that still is.
    // spec: NAT64#detection
    // spec: NAT64#ipv6-egress
    // spec: CONST#determining-host-constants
    async fn retry_host_probes(&mut self) {
        if self.host_probes.pending() {
            let moved = self.host_probes.retry().await;
            let ipv4 = self
                .host_probes
                .ipv4_egress
                .determined()
                .filter(|_| moved.ipv4_egress);
            let ipv6 = self
                .host_probes
                .ipv6_egress
                .determined()
                .filter(|_| moved.ipv6_egress);
            let timezone = self
                .host_probes
                .timezone
                .determined()
                .filter(|_| moved.timezone);
            let constants = &self.host_probes.constants;
            let found = crate::sysconst::Determined {
                ipv4_egress: ipv4,
                ipv6_egress: ipv6,
                timezone,
                available_threads: constants
                    .available_threads
                    .determined()
                    .filter(|_| moved.available_threads),
                available_memory: constants
                    .available_memory
                    .determined()
                    .filter(|_| moved.available_memory),
                node_name: constants.node_name.determined().filter(|_| moved.node_name),
            };
            let any_found = found.ipv4_egress.is_some()
                || found.ipv6_egress.is_some()
                || found.timezone.is_some()
                || found.available_threads.is_some()
                || found.available_memory.is_some()
                || found.node_name.is_some();
            if any_found {
                crate::sysconst::determined(found);
                self.host_facts_changed = true;
            }
            self.force_dns64_translation = self.force_dns64_for(&self.host_probes);
            self.host_faults_synced = false;
        }
        if self.host_faults_synced {
            return;
        }
        let probes = &self.host_probes;
        let undetermined: [(&str, &str, Option<String>); 4] = [
            (
                "nat64_detection_failed",
                "whether the network provides NAT64",
                probes
                    .nat64_detection
                    .as_ref()
                    .and_then(|f| f.undetermined().map(str::to_owned)),
            ),
            (
                "ipv4_egress_undetermined",
                "whether the host has IPv4 egress",
                probes.ipv4_egress.undetermined().map(str::to_owned),
            ),
            (
                "ipv6_egress_undetermined",
                "whether the host has IPv6 egress",
                probes.ipv6_egress.undetermined().map(str::to_owned),
            ),
            (
                "timezone_undetermined",
                "the host's time zone",
                probes.timezone.undetermined().map(str::to_owned),
            ),
        ];
        let constants = probes
            .constants
            .undetermined()
            .into_iter()
            .map(|(name, why)| format!("{name} ({why})"))
            .collect::<Vec<_>>();
        for (kind, what, why) in undetermined {
            match why {
                Some(why) => {
                    self.file_system_fault(kind, &format!("cannot determine {what}: {why}"))
                }
                None => self.clear_system_fault(kind),
            }
        }
        // spec: CONST#determining-host-constants
        // One fault naming every constant still undetermined, its description
        // kept to the ones that still are as the others are found.
        if constants.is_empty() {
            self.clear_system_fault("host_constant_undetermined");
        } else {
            let description = format!("cannot determine {}", constants.join(", "));
            tracing::error!("{description}");
            self.db.call(move |db| {
                let key = crate::runtime::faults::FaultKey::app_wide(
                    &AppName::new_unchecked("_system"),
                    "host_constant_undetermined",
                );
                let filed = crate::runtime::faults::file_once(
                    db,
                    &key,
                    &crate::runtime::faults::FaultMeta::default(),
                    &description,
                )
                .and_then(|filed| {
                    if !filed {
                        crate::runtime::faults::update_description(db, &key, &description)?;
                    }
                    Ok(())
                });
                if let Err(e) = filed {
                    tracing::error!("cannot file host_constant_undetermined: {e}");
                }
            });
        }
        self.host_faults_synced = true;
    }

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
        cipher: Arc<Cipher>,
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
        gate_runtime: Arc<GateRuntime>,
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
            caddy_upgrade_rejected: false,
            data_dir,
            db,
            cipher,
            gate_runtime,
            registry,
            app_registry,
            written_obs,
            started_jobs: HashSet::new(),
            completed_jobs: HashSet::new(),
            crash_looped: HashSet::new(),
            event_tx,
            prev_states: BTreeMap::new(),
            prev_health: BTreeMap::new(),
            slice_retry: crate::runtime::retry::RetryGate::new(
                SLICE_BACKOFF_BASE,
                SLICE_BACKOFF_CAP,
            ),
            rolling_updates: HashSet::new(),
            unhealthy_replace_deployments: HashSet::new(),
            replace_failed: HashSet::new(),
            nat64_active,
            nat64_translator: Nat64Translator::Unknown,
            force_dns64_translation,
            host_probes: crate::system::host_probes::HostProbes::settled(false, false),
            host_faults_synced: false,
            host_facts_changed: false,
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
            unidentifiable_units_filed: HashSet::new(),
            teardown_identities_read: HashSet::new(),
        }
    }

    // spec: RCN#desired-state
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
                // spec: FLT#host-faults
                // Held while any one stored priority cannot be read, and the
                // app it belongs to is held back below.
                let mut unreadable: Vec<String> = p
                    .iter()
                    .filter_map(|(app, level)| level.as_ref().err().map(|e| format!("{app} ({e})")))
                    .collect();
                unreadable.sort();
                if unreadable.is_empty() {
                    self.clear_system_fault("app_priorities_unreadable");
                } else {
                    self.file_system_fault(
                        "app_priorities_unreadable",
                        &format!(
                            "the stored priority of these apps cannot be read, so they are not \
                             actuated until it can be: {}",
                            unreadable.join(", ")
                        ),
                    );
                }
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
            // spec: PRIO#unreadable-priorities
            // An app whose stored priority cannot be read is not actuated, so
            // its running workloads keep running as they are.
            if matches!(priorities.get(&name), Some(Err(_))) && phase != AppPhase::NotInstalled {
                skipped += 1;
                continue;
            }
            // spec: RCN#desired-state
            // Only NotInstalled apps are skipped, unless a shell session or a
            // gate check that has ended is still tearing down what it started:
            // nothing else stops those resources.
            match phase {
                AppPhase::Installed | AppPhase::Uninstalling => {}
                AppPhase::NotInstalled if !entry.has_dynamic_slots() => continue,
                AppPhase::NotInstalled => {}
            }
            // Read before the progress is held, as it reads the progress
            // itself. A snapshot that went unsteady since is rechecked before
            // the GC sweeps by it.
            let steady = entry.is_steady();
            let progress = entry.active_progress.read();
            let app_def = entry.app.def.load_full();
            // spec: AUTO#scale
            let effective_scales =
                self.compute_effective_scales(&name, &app_def, &scaling_decisions);
            // spec: CTL#resource-stop
            // spec: FLT#app-faults
            // An unreadable set of stops is not "nothing stopped": that would
            // start every resource an operator stopped.
            let stopped_set = {
                let app_name_for_stop = name.clone();
                match self
                    .db
                    .call(move |db| stopped::load_stopped(db, &app_name_for_stop))
                {
                    Ok(set) => set,
                    Err(e) => {
                        error!(app = %name, error = %e, "cannot read the app's stopped resources; skipping app this tick");
                        self.file_desired_state_fault(
                            &name,
                            &format!("cannot read the app's stopped resources: {e}"),
                        );
                        skipped += 1;
                        continue;
                    }
                }
            };
            // spec: GRUN#gate-state
            // spec: RCN#gate-holds
            let gates = match phase {
                AppPhase::Installed => {
                    let (app_name, def, cipher) =
                        (name.clone(), Arc::clone(&app_def), Arc::clone(&self.cipher));
                    let stopped = stopped_set.clone();
                    let remembered = self.gate_runtime.remembered(&name);
                    let definition_error = entry.script_error.as_ref().map(|(why, _)| why.clone());
                    // spec: GATE#body
                    let body = progress
                        .as_ref()
                        .and_then(crate::runtime::OperationProgress::body_starts);
                    let driven = self.gate_runtime.take_snapshot(
                        &name,
                        &app_def,
                        definition_error.as_deref(),
                        &stopped,
                    );
                    match driven {
                        Some(driven) => crate::runtime::gates::view_from(
                            &app_name,
                            &def,
                            &driven.gates,
                            &driven.readiness,
                            &remembered,
                            body.as_ref(),
                        ),
                        None => self.db.call(move |db| {
                            let state = crate::runtime::gates::evaluate(
                                db,
                                &app_name,
                                &def,
                                &cipher,
                                definition_error.as_deref(),
                                &remembered,
                            );
                            crate::runtime::gates::view(
                                db,
                                &app_name,
                                &def,
                                &state,
                                &stopped,
                                &remembered,
                                body.as_ref(),
                            )
                        }),
                    }
                }
                _ => GateView::default(),
            };
            // A resource a gate holds is kept at `Unscheduled` as one an
            // operator stopped is.
            let down = gates.keep_down(&stopped_set);
            let desired = match phase {
                AppPhase::Uninstalling => compute_uninstalling(&name, &app_def, &*self.registry),
                // spec: OPS#shell-sessions
                // An app that is not installed desires nothing of its own.
                AppPhase::NotInstalled => Ok(DesiredState::default()),
                AppPhase::Installed => compute(
                    &name,
                    &app_def,
                    (*progress).as_ref(),
                    &*self.registry,
                    &effective_scales,
                    &down,
                ),
            };
            // Read in place, as the operation's progress is, rather than
            // copied each tick.
            let session_slots: Vec<_> = entry
                .sessions
                .slots()
                .into_iter()
                .chain(entry.checks.slots())
                .collect();
            let session_guards: Vec<_> = session_slots.iter().map(|slot| slot.read()).collect();
            let sessions: Vec<&crate::runtime::OperationProgress> =
                session_guards.iter().filter_map(|g| g.as_ref()).collect();
            let (desired, gate_keys) = match desired {
                Ok(mut d) => {
                    // spec: RCN#desired-state
                    // spec: RCN#gate-holds
                    // Each open shell session's dynamic resources, and the Job
                    // of each gate check being run, join the desired state
                    // whatever phase the app is in and whatever else it holds.
                    // Where the app is going or gone, they are held stopped.
                    merge_sessions(
                        &mut d,
                        &sessions,
                        matches!(phase, AppPhase::Uninstalling | AppPhase::NotInstalled),
                    );
                    // spec: UPD#spec-hash
                    let gate_keys = match pods::GateKeys::of(&app_def, &d, &self.cipher) {
                        Ok(keys) => keys,
                        Err(e) => {
                            error!(app = %name, error = %e, "cannot digest the app's gate keys; skipping app this tick");
                            self.file_desired_state_fault(
                                &name,
                                &format!("cannot digest the app's gate keys: {e}"),
                            );
                            skipped += 1;
                            continue;
                        }
                    };
                    self.clear_desired_state_fault(&name);
                    (d, gate_keys)
                }
                Err(e) => {
                    error!(app = %name, error = %e, "failed to compute desired state; skipping app this tick");
                    self.file_desired_state_fault(
                        &name,
                        &format!("failed to compute desired state: {e}"),
                    );
                    // spec: TRD#uninstall-scope
                    // An uninstalling app's desired state is its recorded
                    // instances, so failing to compute it is failing to read
                    // them, and the teardown is held until they can be read.
                    if phase == AppPhase::Uninstalling {
                        self.hold_teardown_identities_unreadable(&name, &e.to_string());
                    }
                    skipped += 1;
                    continue;
                }
            };
            let warm_cert_hostnames = (*progress)
                .iter()
                .chain(sessions.iter().copied())
                .flat_map(|p| p.warm_cert_hostnames.iter().cloned())
                .collect();
            // spec: PRIO#priority-setting
            // Read once for the whole tick above; an app with no stored
            // decision stands at the default.
            let app_priority = priorities
                .get(&name)
                .and_then(|p| p.as_ref().ok())
                .copied()
                .unwrap_or_default();
            let declarations = desired.declarations(&app_def);
            snapshots.push(AppSnapshot {
                name,
                desired,
                steady,
                app_def,
                declarations,
                phase,
                phase_handle: Arc::clone(&entry.phase),
                warm_cert_hostnames,
                app_priority,
                gate_keys,
                gates,
                install_seq: entry.install_seq,
            });
        }
        (snapshots, skipped)
    }
}

/// What the reconciler knows of the NAT64 translator on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nat64Translator {
    /// Not yet established: the host may still hold the translator from an
    /// earlier run of the daemon.
    Unknown,
    Installed,
    Removed,
}

#[derive(Debug, PartialEq, Eq)]
enum Nat64Step {
    SetUp,
    TearDown,
}

/// What an iteration does about the translator. While NAT64 is active it is
/// kept set up whenever an app is installed, and torn down when
/// none is; a translator known to be removed is not torn down again.
// spec: NAT64#translator-lifecycle
fn nat64_step(active: bool, translator: Nat64Translator, idle: bool) -> Option<Nat64Step> {
    match (active, idle, translator) {
        (false, _, _) => None,
        (true, false, Nat64Translator::Installed) => None,
        (true, false, _) => Some(Nat64Step::SetUp),
        (true, true, Nat64Translator::Removed) => None,
        (true, true, _) => Some(Nat64Step::TearDown),
    }
}

/// What to do with `caddy_failed` once an iteration has a proxy serving.
#[derive(Debug, PartialEq, Eq)]
enum CaddyFaultAction {
    File(String),
    Clear,
    Hold,
}

/// The `caddy_failed` fault's handling for a proxy that is up, and whether the
/// fault now on file is for a rejected upgrade.
///
/// A failed bring-up clears once an iteration finds the proxy healthy; a
/// rejected upgrade leaves the previous proxy healthy and serving, so it
/// clears only when an upgrade succeeds.
// spec: FLT#host-faults
fn caddy_fault_action(
    image: &caddy::ProxyImageOutcome,
    upgrade_rejected: bool,
) -> (CaddyFaultAction, bool) {
    match image {
        caddy::ProxyImageOutcome::UpgradeRejected(why) => {
            (CaddyFaultAction::File(why.clone()), true)
        }
        caddy::ProxyImageOutcome::Replaced => (CaddyFaultAction::Clear, false),
        caddy::ProxyImageOutcome::Current if upgrade_rejected => (CaddyFaultAction::Hold, true),
        caddy::ProxyImageOutcome::Current => (CaddyFaultAction::Clear, false),
    }
}

/// The standings the running workloads take: each app's priority and the
/// Deployment levels its workloads stand at.
///
/// Pure so the derivation can be checked without a supervisor: applying it is
/// [`Reconciler::reconcile_groups`].
// spec: PRIO#resource-control-groups
fn desired_standings(apps: &[AppSnapshot]) -> Vec<AppStandings> {
    apps.iter()
        .map(|app| {
            // The levels this app's definition actually declares, plus Normal,
            // where Jobs and every other workload that is not a Deployment
            // stand. A level that stops being declared has its group
            // forgotten, which is safe: forgetting leaves anything still
            // running in it alone.
            let mut levels: BTreeSet<Priority> = BTreeSet::new();
            levels.insert(Priority::Normal);
            for resource in app.declarations.resources.values() {
                if let Resource::Deployment(deployment) = resource {
                    levels.insert(deployment.def.lock().priority);
                }
            }
            AppStandings {
                app: app.name.clone(),
                priority: app.app_priority,
                levels,
            }
        })
        .collect()
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
    /// Bring the resource-control groups into step with the app priorities and
    /// the declared Deployment levels.
    ///
    /// Runs before actuation so a workload started this tick joins a group that
    /// already carries the right weights, and reweights the groups of workloads
    /// that are already running, which is what makes an app priority change
    /// take effect without a redeploy.
    // spec: PRIO#resource-control-groups
    async fn reconcile_groups(&mut self, apps: &[AppSnapshot], dropped_apps: usize) {
        let now = Instant::now();
        if !self.slice_retry.should_attempt(now) {
            return;
        }

        let desired = desired_standings(apps);
        // spec: RCN#applying-absolute-state
        // Forgetting a group is only safe when every app was accounted for. An
        // app dropped by a transient error is absent from `apps` while its
        // workloads keep running, and its group carries the weights they run
        // under.
        let prune = dropped_apps == 0;

        let synced = tokio::time::timeout(
            SLICE_SYNC_TIMEOUT,
            self.driver.process.sync_groups(desired, prune),
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
                        *dep_def.scale.start(),
                        *dep_def.scale.end(),
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

    // spec: RCN#reconciliation-loop
    // spec: FLT#filing-and-clearing
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn tick(&mut self) -> bool {
        let more = self.iterate().await;
        self.settle_teardown_identities_read();
        more
    }

    async fn iterate(&mut self) -> bool {
        // spec: INFRA#pod-network-sources
        // spec: INFRA#forwarding-policy
        // spec: INFRA#pod-traffic-to-the-host
        // No workload starts until the isolation rules are installed. The
        // install is retried here after a growing delay, and the fault is
        // settled whether or not the backends are reachable.
        let isolation = self.driver.ensure_isolation().await;
        // spec: FLT#host-faults
        match &isolation {
            Err(e) => self.file_system_fault_for(
                "packet_filter_failed",
                ISOLATION_FAULT_SUBJECT,
                &format!("{e}; no workloads or shell sessions will start until they are"),
            ),
            Ok(()) => self.clear_system_fault_for("packet_filter_failed", ISOLATION_FAULT_SUBJECT),
        }

        // spec: INFRA#unavailable-backends — when the daemon came up without a
        // working container runtime / process manager, every actuation would
        // just error against the unavailable backends. File the overarching
        // system fault (idempotent) and skip the rest of the tick so the OI
        // stays responsive and the operator sees why nothing is running.
        if let Some(reason) = self.driver.probe_backends().await {
            self.file_system_fault(
                "container_runtime_unavailable",
                &format!(
                    "the container runtime or service manager cannot be reached, so no \
                     workloads can be actuated; both are probed again on later iterations. \
                     Details: {reason}"
                ),
            );
            return true;
        }
        // spec: FLT#host-faults
        // Both backends are reachable, so the fault no longer holds.
        self.clear_system_fault("container_runtime_unavailable");

        if isolation.is_err() {
            return true;
        }

        self.retry_host_probes().await;

        self.reconcile_stray_shells().await;

        let (apps, dropped_apps) = self.snapshot_all_apps();
        self.note_teardown_identities_read(&apps);

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
                // Not idle: nothing outside will announce that the state has
                // become readable, so the loop must keep coming back to look.
                return true;
            }
            return false;
        }
        let app_coverage = Coverage::of(dropped_apps);

        self.reconcile_groups(&apps, dropped_apps).await;

        // spec: NAT64#translator-lifecycle
        // Wake-from-idle: ensure NAT64 translator is installed before any
        // pod starts. Setup is idempotent (forwarding sysctls already at 1,
        // module already loaded, instance-add handles EEXIST) so it is safe
        // to run on every non-idle tick where `nat64_installed` is false.
        if nat64_step(self.nat64_active, self.nat64_translator, false) == Some(Nat64Step::SetUp) {
            match self.driver.translator.set_up().await {
                Ok(()) => {
                    self.nat64_translator = Nat64Translator::Installed;
                    self.clear_system_fault("nat64_setup_failed");
                    // spec: FLT#host-faults
                    // A translator that is set up again is no longer one a
                    // teardown failed to remove.
                    self.clear_system_fault("nat64_teardown_failed");
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
        // A fault table that cannot be read is not one without crash loops:
        // the set last read stands, so no suppression lifts on a failed read.
        let crash_looped = self.db.call(|db| {
            crate::runtime::faults::list_active_faults(db, None).map(|active| {
                active
                    .into_iter()
                    .filter(|f| f.kind == "crash_loop")
                    .filter_map(|f| f.instance_id.as_deref().and_then(InstanceId::from_hex))
                    .collect::<HashSet<_>>()
            })
        });
        match crash_looped {
            Ok(set) => self.crash_looped = set,
            Err(e) => error!(
                "cannot read the active crash_loop faults; the suppression last read stands: {e}"
            ),
        }

        // spec: DNS#resolver-lifecycle
        // The resolver is running and healthy before any workload container
        // starts, so it is brought up ahead of the phase that starts them, not
        // alongside it. While it is not, the iteration goes no further.
        let resolver_result = tokio::time::timeout(
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
        )
        .await;
        match resolver_result {
            Ok(Ok(addrs)) => {
                self.clear_system_fault("resolver_failed");
                self.resolver_addr = Some(addrs.v6);
            }
            Ok(Err(e)) => {
                error!(error = %e, "resolver startup failed this tick; no workload is started");
                self.file_system_fault("resolver_failed", &format!("resolver startup failed: {e}"));
                return true;
            }
            Err(_) => {
                warn!("resolver startup timed out this tick; no workload is started");
                self.file_system_fault("resolver_failed", "resolver startup timed out");
                return true;
            }
        }

        // spec: RCN#reconciliation-loop
        // --- Concurrent phase: pods ∥ volumes ∥ caddy ---
        let (pod_updates, vol_observations, caddy_result) = tokio::join!(
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
        );

        // --- Process pod results ---
        let (running_pods_by_app, failed_units) = self.ingest_pod_results(&apps, pod_updates);

        // spec: LINUX#supervised-units
        // A failed unit stays loaded until its result is on record, so that a
        // unit gone without one is a unit the supervisor reported succeeded.
        for instance in &failed_units {
            let unit = crate::system::actuator::unit_name(instance);
            if let Err(e) = self.driver.process.reset_failed_unit(&unit).await {
                warn!(unit = %unit, error = %e, "could not discard failed unit; retrying next tick");
            }
        }

        // --- Process volume results ---
        self.ingest_volume_results(&apps, vol_observations);

        // --- Image reconciliation: pins, tracking, and autonomous GC ---
        // spec: IMG#image-warming, IMG#pins, IMG#image-usage-tracking
        self.reconcile_images(running_pods_by_app.values().flatten())
            .await;

        // --- Process caddy result ---
        // spec: AUTO#ingress-and-network-repair
        let caddy_addrs = match caddy_result {
            Ok(Ok(ensured)) => match caddy::build_client(&ensured.addrs.admin_socket) {
                Ok(client) => {
                    let (action, rejected) =
                        caddy_fault_action(&ensured.image, self.caddy_upgrade_rejected);
                    self.caddy_upgrade_rejected = rejected;
                    match action {
                        CaddyFaultAction::File(why) => {
                            self.file_system_fault("caddy_failed", &why);
                        }
                        CaddyFaultAction::Clear => self.clear_system_fault("caddy_failed"),
                        CaddyFaultAction::Hold => {}
                    }
                    *self.caddy_admin_client.write().await = client;
                    self.caddy_v4_addr = ensured.addrs.v4;
                    Some(ensured.addrs)
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
        // spec: SITE#unreadable-site-services
        // `None` is not "no site services or mappings": every consumer below
        // withholds what it would otherwise conclude from them.
        let ext_snapshot = self.load_external_services();

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
        let has_ipv6_egress = self.routes_with_ipv6_egress();
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
            ext_snapshot.as_ref(),
            &make_resolve_ctx(),
        );
        let routes_coverage = routes_coverage.and(app_coverage);
        self.persist_obs(route_obs);

        // --- Classify site-service endpoint outcomes and reconcile the
        //     associated `site_service_endpoint_*` faults. ---
        // spec: SITE#endpoint-addressing
        self.converge_site_service_faults(ext_snapshot.as_ref(), &make_resolve_ctx());

        // --- Load site-ingress snapshot (DB-backed) once per tick. ---
        // spec: SITE#site-ingresses, SITE#attachments
        // spec: SITE#unreadable-site-ingresses
        // `None` is not "no site ingresses": every consumer below withholds
        // what it would otherwise conclude from them.
        let site_ingress_snapshot = self.load_site_ingresses();

        // spec: CHAIN#proxy-strategies
        let renewal_ratios = self.resolve_renewal_ratios();

        // --- Compute nftables + proxy (sync, gated on caddy) ---
        let nft_and_proxy = caddy_addrs.map(|addrs| {
            let caddy_ip = addrs.v6;

            let proxy_build = phases::compute_proxy_config(
                &apps,
                app_coverage,
                site_ingress_snapshot.as_ref(),
                &running_pods_by_app,
                &self.node_prefix,
                &*self.registry,
                self.cert_endpoint_url.as_deref(),
                &renewal_ratios,
            );

            // spec: PRX#shared-ports
            // After the proxy, whose served claims the redirected ports are.
            let nft_build = phases::compute_nftables_rules(
                &apps,
                &running_pods_by_app,
                caddy_ip,
                self.caddy_v4_addr,
                &self.node_prefix,
                &*self.registry,
                ext_snapshot.as_ref(),
                &make_resolve_ctx(),
                &proxy_build,
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
                    dtls_ingresses,
                } = proxy_build;
                // spec: SITE#unreadable-site-ingresses
                // spec: FLT#filing-and-clearing
                // `None` when the build is partial, an app or the site
                // ingresses missing from it: neither filed nor cleared.
                if let Some(ingress_conflicts) = &ingress_conflicts {
                    // spec: SITE#ingress-conflicts
                    self.reconcile_ingress_conflicts(ingress_conflicts);
                }
                if let Some(unresolved) = &unresolved_site_attachments {
                    // spec: SITE#attachments
                    self.reconcile_unresolved_site_attachments(unresolved);
                }
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
                    if let Some(snapshot) = &site_ingress_snapshot {
                        for hostname in &site_ingress_tls_hostnames(snapshot) {
                            coord.ensure(hostname);
                        }
                    }
                }

                // spec: FLT#certificate-faults
                // A hostname no ingress needs a certificate for has nothing
                // left to fail at.
                // spec: SITE#unreadable-site-ingresses — unless the site
                // ingresses, which may need it, could not be read.
                if let Some(snapshot) = &site_ingress_snapshot {
                    let needing_certificates: std::collections::BTreeSet<String> =
                        managed_ingresses
                            .iter()
                            .map(|mi| mi.hostname.clone())
                            .chain(site_ingress_tls_hostnames(snapshot))
                            .collect();
                    self.clear_cert_issue_faults_not_needed(needing_certificates);
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
                            "a contributor is missing from this tick's {plane}; \
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
                    apply_routes_plane(&*self.driver.data_plane, &all_routes, routes_coverage),
                    apply_rules_plane(&*self.driver.data_plane, &dp_rules, rules_coverage),
                    apply_proxy_plane(&*self.driver.proxy, &proxy_config, proxy_coverage),
                );

                // spec: FLT#ingress-and-service-faults
                // `None` when the build is partial: an ingress missing from it
                // is absent rather than gone, so its fault is neither filed nor
                // cleared.
                if let Some(served) = &dtls_ingresses {
                    self.reconcile_dtls_refusals(served).await;
                }

                self.settle_plane_fault("routes_failed", "apply_routes", routes_res);
                self.settle_plane_fault("packet_filter_failed", "apply_rules", rules_res);
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
                        // The proxy config is applied only with the site
                        // ingresses read, so the snapshot is here.
                        let site_ingress_names: Vec<String> = site_ingress_snapshot
                            .iter()
                            .flat_map(|snapshot| &snapshot.ingresses)
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
                // caddy), unless this tick's route set is missing a
                // contributor.
                let routes_res =
                    apply_routes_plane(&*self.driver.data_plane, &all_routes, routes_coverage)
                        .await;
                self.settle_plane_fault("routes_failed", "apply_routes", routes_res);
            }
        }

        self.emit_state_changes(&apps);
        self.retire_unscheduled_excess(&apps);
        self.sweep_unscheduled_instances(&apps);

        true
    }

    /// The site services and external service mappings, or `None` when they
    /// cannot be read, holding `site_services_unreadable` for as long as they
    /// cannot.
    // spec: SITE#unreadable-site-services
    // spec: FLT#host-faults
    fn load_external_services(
        &self,
    ) -> Option<crate::runtime::external_service_mappings::ExternalServiceSnapshot> {
        match self
            .db
            .call(crate::runtime::external_service_mappings::ExternalServiceSnapshot::load)
        {
            Ok(snapshot) => {
                self.clear_system_fault("site_services_unreadable");
                Some(snapshot)
            }
            Err(e) => {
                error!(
                    error = %e,
                    "could not read the site services or the external service mappings; \
                     holding the routes and packet-filtering rules as last applied"
                );
                self.file_system_fault(
                    "site_services_unreadable",
                    &format!(
                        "could not read the site services or the external service mappings, so \
                         the routes and the packet-filtering rules are held as last applied \
                         rather than withdrawing every external service. Error: {e}"
                    ),
                );
                None
            }
        }
    }

    /// Converge the site service endpoint faults to this tick's
    /// classification of the endpoints in `ext_snapshot`, or leave them as
    /// they are when it is `None` because the site services could not be read.
    // spec: SITE#endpoint-addressing
    // spec: SITE#unreadable-site-services
    fn converge_site_service_faults(
        &mut self,
        ext_snapshot: Option<&crate::runtime::external_service_mappings::ExternalServiceSnapshot>,
        resolve_ctx: &ResolveCtx<'_>,
    ) {
        let Some(ext_snapshot) = ext_snapshot else {
            return;
        };
        let unresolved_hosts: HashSet<String> = self
            .site_resolver
            .as_deref()
            .map(|r| r.unresolved_hosts())
            .unwrap_or_default();
        let fault_set =
            phases::classify_site_service_endpoints(ext_snapshot, resolve_ctx, &unresolved_hosts);
        self.reconcile_site_service_faults(fault_set);
    }

    /// The site ingresses and their attachments, or `None` when they cannot
    /// be read, holding `site_ingresses_unreadable` for as long as they cannot.
    // spec: SITE#unreadable-site-ingresses
    // spec: FLT#host-faults
    fn load_site_ingresses(&self) -> Option<site_proxy::SiteIngressSnapshot> {
        match self.db.call(site_proxy::load) {
            Ok(snapshot) => {
                self.clear_system_fault("site_ingresses_unreadable");
                Some(snapshot)
            }
            Err(e) => {
                error!(
                    error = %e,
                    "could not read the site ingresses or their attachments; holding the \
                     proxy configuration and packet-filtering rules as last applied"
                );
                self.file_system_fault(
                    "site_ingresses_unreadable",
                    &format!(
                        "could not read the site ingresses or their attachments, so the proxy \
                         configuration and the packet-filtering rules are held as last applied \
                         rather than withdrawing every site ingress. Error: {e}"
                    ),
                );
                None
            }
        }
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
                let released = net_name.clone();
                if let Err(e) = self
                    .db
                    .call(move |db| crate::runtime::pod_networks::release(db, &released))
                {
                    tracing::warn!(%net_name, "cannot free a pod network prefix: {e}");
                }
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
        // spec: FLT#host-faults
        // A stopped proxy and resolver are neither failing to come up nor
        // holding a rejected upgrade.
        self.caddy_upgrade_rejected = false;
        self.clear_system_fault("caddy_failed");
        self.clear_system_fault("resolver_failed");

        // spec: NAT64#translator-lifecycle
        if nat64_step(self.nat64_active, self.nat64_translator, true) == Some(Nat64Step::TearDown) {
            match self.driver.translator.tear_down().await {
                Ok(()) => {
                    self.nat64_translator = Nat64Translator::Removed;
                    // spec: FLT#host-faults
                    self.clear_system_fault("nat64_teardown_failed");
                }
                Err(e) => {
                    // spec: NAT64#translator-lifecycle
                    self.file_system_fault(
                        "nat64_teardown_failed",
                        &format!("tearing down the NAT64 translator failed: {e}"),
                    );
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
    ) -> (
        HashMap<AppName, Vec<RunningPod>>,
        Vec<crate::runtime::ResourceInstance>,
    ) {
        let mut running_pods_by_app = HashMap::new();
        let mut recorded_failures = Vec::new();
        // spec: UPD#rolling-updates
        // Rebuild rolling_updates from scratch each tick so that completed
        // rollouts are automatically cleared.
        self.rolling_updates.clear();
        for (app_name, mut pod_update) in pod_updates {
            // spec: FLT#workload-faults
            self.file_image_pull_faults(&app_name, &pod_update);
            // spec: FLT#workload-faults
            self.file_unit_failure_faults(&app_name, &pod_update);
            // spec: AUTO#restart-record
            // spec: AUTO#crash-loop-detection
            // Restart bookkeeping runs first: it appends rate-derived crash
            // loops to the update so both triggers file through one path.
            self.record_restarts(&app_name, &mut pod_update);
            // spec: FLT#workload-faults
            self.file_crash_loop_faults(&app_name, &pod_update);
            // spec: FLT#workload-faults
            self.file_health_check_faults(&app_name, &pod_update);
            // spec: FLT#workload-faults
            self.file_external_volume_faults(&app_name, &pod_update);
            self.file_instance_registry_faults(&app_name, &pod_update);
            self.file_pod_actuation_faults(&app_name, &pod_update);
            for dep_name in &pod_update.rolling_deployments {
                self.rolling_updates
                    .insert((app_name.clone(), dep_name.clone()));
            }
            let recorded = self.persist_obs(pod_update.observations);
            // spec: LINUX#supervised-units
            recorded_failures.extend(
                pod_update
                    .failed_units
                    .iter()
                    .filter(|i| {
                        recorded.contains(&(i.id, "unit_failed"))
                            || recorded.contains(&(i.id, "unit_start_limit_hit"))
                    })
                    .cloned(),
            );
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
        (running_pods_by_app, recorded_failures)
    }

    // spec: AUTO#health-check-replacement
    // spec: AUTO#replacement-guard
    fn update_replace_state(
        &mut self,
        apps: &[AppSnapshot],
        running_pods_by_app: &HashMap<AppName, Vec<RunningPod>>,
    ) {
        // spec: AUTO#replacement-guard
        // The guard is in-memory but its truth lives in the persisted fault
        // table. Re-derive each tick so the suppression survives daemon
        // restarts (and so an operator clearing the fault has its effect
        // picked up immediately).
        //
        // The fault clears when the app's generation changes. That is read
        // from the generation history, which records when each generation was
        // made, not from a generation this process remembered: a change made
        // while the daemon was down would otherwise never clear it.
        let persisted_failed: std::collections::HashSet<(AppName, String)> = self.db.call(|db| {
            let mut out = std::collections::HashSet::new();
            let active = match crate::runtime::faults::list_active_faults(db, None) {
                Ok(v) => v,
                Err(_) => return out,
            };
            for f in active {
                if f.kind != "health_check_replace_failed" {
                    continue;
                }
                let Some(dep) = f.resource_name.clone() else {
                    continue;
                };
                match crate::runtime::generations::changed_since(db, &f.app, f.timestamp) {
                    Ok(false) => {
                        out.insert((f.app, dep));
                    }
                    Ok(true) => {
                        let key = crate::runtime::faults::FaultKey::new(
                            &f.app,
                            "health_check_replace_failed",
                            &f.subject,
                        );
                        if let Err(e) = crate::runtime::faults::clear_fault_by_key(db, &key) {
                            warn!(app = %f.app, "failed to clear health_check_replace_failed: {e}");
                            out.insert((f.app, dep));
                        }
                    }
                    // Whether the generation changed is unknown: the guard holds.
                    Err(e) => {
                        warn!(app = %f.app, "could not read the generation history: {e}");
                        out.insert((f.app, dep));
                    }
                }
            }
            out
        });
        self.replace_failed = persisted_failed;

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
            for (id, resource) in &app.declarations.resources {
                let crate::defs::resource::Resource::Deployment(dep) = resource else {
                    continue;
                };
                let dep_def = dep.def.lock();
                let scale = (*dep_def.scale.start(), *dep_def.scale.end());
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

    // spec: LCY#external-volumes
    fn hold_external_volume_states(
        &self,
        states: Vec<(crate::runtime::ResourceInstance, LifecycleState)>,
    ) {
        if states.is_empty() {
            return;
        }
        self.db.call(move |db| {
            for (instance, state) in &states {
                if let Err(e) =
                    crate::runtime::history::set_external_volume_state(db, instance, *state)
                {
                    error!(
                        instance = %instance.display_name,
                        error = %e,
                        "could not hold an external volume's lifecycle state"
                    );
                }
            }
        });
    }

    fn ingest_volume_results(
        &mut self,
        apps: &[AppSnapshot],
        vol_updates: Vec<(AppName, volumes::VolumeActuationUpdate)>,
    ) {
        for (app_name, vol_update) in vol_updates {
            self.file_volume_actuation_faults(&app_name, &vol_update);
            self.persist_obs(vol_update.observations);
            self.hold_external_volume_states(vol_update.external_volume_states);
        }
        let _ = apps;
    }

    // -----------------------------------------------------------------------
    // Uninstall phase
    // -----------------------------------------------------------------------

    /// Bring the volumes, services and ingresses of an uninstalling app whose
    /// workloads are gone to `Unscheduled`: volumes are removed, and held where
    /// that is their removal, and the routes and virtual hosts of the services
    /// and ingresses are left out of the state applied after this.
    // spec: TRD#teardown
    async fn bring_remaining_instances_to_unscheduled(
        &mut self,
        app: &AppSnapshot,
        apps: &[AppSnapshot],
    ) {
        let mut observations = Vec::new();
        // The undeclared ones as much as the declared: neither's routes or
        // virtual hosts are in the state applied after this.
        let instances = app
            .desired
            .resources
            .iter()
            .map(|dr| &dr.instance)
            .chain(&app.desired.undeclared);
        for instance in instances {
            let (removed, cleaned_up) = match instance.kind {
                ResourceKind::Service => ("network_removed", "network_cleaned_up"),
                ResourceKind::Ingress => ("ingress_removed", "ingress_cleaned_up"),
                _ => continue,
            };
            for kind in ["stop_sent", removed, cleaned_up] {
                observations.push((instance.clone(), kind, serde_json::json!({})));
            }
        }
        self.persist_obs(observations);

        let volumes =
            volumes::observe_and_actuate(&self.observer, &self.actuator, &self.db, &app.desired)
                .await;
        self.ingest_volume_results(apps, vec![(app.name.clone(), volumes)]);
    }

    /// Remove the storage of every anonymous volume still recorded against an
    /// instance of `app`, whose pods are all `Unscheduled`, and return
    /// whether none is left recorded.
    ///
    /// A removal that fails is part of its instance's stop failing, so it
    /// holds `stop_failed` for the instance until a later removal succeeds.
    // spec: ACTU#deployment-and-job-instances
    // spec: TRD#teardown
    async fn remove_recorded_anonymous_volumes(&mut self, app: &AppSnapshot) -> bool {
        let app_name = app.name.clone();
        let recorded = match self
            .db
            .call(move |db| crate::runtime::anonymous_volumes::instances_of_app(db, &app_name))
        {
            Ok(recorded) => recorded,
            Err(e) => {
                warn!(
                    app = %app.name,
                    "uninstall: could not read the anonymous volumes recorded for the app; \
                     not advancing teardown this tick: {e}"
                );
                self.teardown_identities_unreadable(&app.name, &e.to_string());
                return false;
            }
        };
        let mut all_removed = true;
        for id in recorded {
            let outcome =
                crate::system::actuator::remove_anonymous_volumes(&self.driver, &self.db, id, &[])
                    .await;
            if outcome.is_err() {
                all_removed = false;
            }
            // spec: FLT#workload-faults
            let Some(instance) = app
                .desired
                .resources
                .iter()
                .map(|dr| &dr.instance)
                .chain(&app.desired.undeclared)
                .find(|instance| instance.id == id)
            else {
                if let Err(e) = outcome {
                    error!(
                        app = %app.name,
                        instance = %id.to_hex(),
                        "uninstall: removing the anonymous volumes of an instance the app does \
                         not hold failed: {e}"
                    );
                }
                continue;
            };
            let key = crate::runtime::faults::FaultKey::new(&app.name, "stop_failed", id.to_hex());
            let held = match outcome {
                Ok(()) => self
                    .db
                    .call(move |db| crate::runtime::faults::clear_fault_by_key(db, &key))
                    .map(|_| ()),
                Err(e) => {
                    error!(
                        app = %app.name,
                        instance = %instance.display_name,
                        "uninstall: removing the instance's anonymous volumes failed; \
                         holding teardown: {e}"
                    );
                    let meta = crate::runtime::faults::FaultMeta {
                        resource_type: Some(instance.kind.type_name().to_owned()),
                        resource_name: instance.name.clone(),
                        instance_id: Some(id.to_hex()),
                    };
                    let description = e.to_string();
                    self.db
                        .call(move |db| {
                            crate::runtime::faults::file_once(db, &key, &meta, &description)
                        })
                        .map(|_| ())
                }
            };
            if let Err(e) = held {
                warn!(
                    app = %app.name,
                    instance = %instance.display_name,
                    "uninstall: could not bring stop_failed in line with the anonymous volume \
                     removal: {e}"
                );
            }
        }
        all_removed
    }

    /// Hold `app`'s teardown for a unit that may be its own but matches no
    /// recorded identity, filing `uninstall_unidentifiable_units`.
    ///
    /// Returns whether an operator has since cleared that fault, which lets the
    /// teardown complete. It is filed once for an uninstall; a fault that is
    /// no longer there was cleared by an operator.
    // spec: TRD#uninstall-scope
    fn hold_for_unidentifiable_units(&mut self, app: &AppName) -> bool {
        const KIND: &str = "uninstall_unidentifiable_units";
        let key = crate::runtime::faults::FaultKey::new(app, KIND, "");
        if self.unidentifiable_units_filed.contains(app) {
            let app_for_read = app.clone();
            return match self
                .db
                .call(move |db| crate::runtime::faults::list_active_faults(db, Some(&app_for_read)))
            {
                Ok(active) => !active.iter().any(|f| f.kind == KIND),
                Err(e) => {
                    warn!(%app, "uninstall: could not read the app's faults; holding teardown: {e}");
                    false
                }
            };
        }
        tracing::error!(
            %app,
            "uninstall: units may still be loaded for this app but no recorded \
             identities remain to match them against; not advancing teardown"
        );
        let filed = self.db.call(move |db| {
            crate::runtime::faults::file_once(
                db,
                &key,
                &crate::runtime::faults::FaultMeta::default(),
                "units remain under this app's prefix that match no recorded identity of \
                 this or any other app, so teardown cannot be safely completed; clear this \
                 fault to complete it and leave those units untouched",
            )
        });
        match filed {
            Ok(_) => {
                self.unidentifiable_units_filed.insert(app.clone());
            }
            Err(e) => warn!(%app, "uninstall: could not file {KIND}: {e}"),
        }
        false
    }

    /// Hold `teardown_identities_unreadable` for `app` while its recorded
    /// identities cannot be read.
    // spec: TRD#uninstall-scope
    fn hold_teardown_identities_unreadable(&self, app: &AppName, why: &str) {
        let key = crate::runtime::faults::FaultKey::new(app, "teardown_identities_unreadable", "");
        let description = format!(
            "the recorded identities of this app's resources cannot be read, so its \
             teardown is held until they can be: {why}"
        );
        if let Err(e) = self.db.call(move |db| {
            crate::runtime::faults::file_once(
                db,
                &key,
                &crate::runtime::faults::FaultMeta::default(),
                &description,
            )
        }) {
            warn!(%app, "uninstall: could not file teardown_identities_unreadable: {e}");
        }
    }

    /// Note that the recorded identities of each uninstalling app in `apps`
    /// were read: an uninstalling app's desired state is its recorded
    /// instances, so computing it read them.
    // spec: TRD#uninstall-scope
    fn note_teardown_identities_read(&mut self, apps: &[AppSnapshot]) {
        self.teardown_identities_read.extend(
            apps.iter()
                .filter(|app| app.phase == AppPhase::Uninstalling)
                .map(|app| app.name.clone()),
        );
    }

    /// A read of `app`'s recorded identities during its teardown failed: hold
    /// the fault, and keep this iteration's earlier reads from clearing it.
    // spec: TRD#uninstall-scope
    fn teardown_identities_unreadable(&mut self, app: &AppName, why: &str) {
        self.teardown_identities_read.remove(app);
        self.hold_teardown_identities_unreadable(app, why);
    }

    /// Clear `teardown_identities_unreadable` for each app whose recorded
    /// identities this iteration read with no read of them failing. An app
    /// whose identities it did not read keeps the fault as it was.
    // spec: FLT#app-faults
    fn settle_teardown_identities_read(&mut self) {
        for app in std::mem::take(&mut self.teardown_identities_read) {
            self.clear_app_fault(&app, "teardown_identities_unreadable");
        }
    }

    fn clear_app_fault(&self, app: &AppName, kind: &str) {
        let key = crate::runtime::faults::FaultKey::new(app, kind, "");
        if let Err(e) = self
            .db
            .call(move |db| crate::runtime::faults::clear_fault_by_key(db, &key))
        {
            warn!(%app, kind, "could not clear the fault: {e}");
        }
    }

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
            self.bring_remaining_instances_to_unscheduled(app, apps)
                .await;
            // spec: GRUN#uninstall
            // The gate records, their holds and the record that the app is set
            // up go with the uninstall, so that installing again runs every
            // gate as for the first time. While they cannot be deleted the app
            // does not reach `NotInstalled`.
            let gate_state_deleted = {
                let app_name = app.name.clone();
                match self
                    .db
                    .call(move |db| crate::runtime::gates::store::delete_app_state(db, &app_name))
                {
                    Ok(()) => true,
                    Err(e) => {
                        error!(
                            app = %app.name,
                            "uninstall: could not delete the app's gate state; trying again next tick: {e}"
                        );
                        false
                    }
                }
            };
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
                    self.teardown_identities_unreadable(&app.name, &e.to_string());
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
            // sibling; each unit that exactly matches another app's recorded
            // identity is ruled out, and over-matching past that only makes
            // this refuse to conclude, which is the safe direction.
            if expected.is_empty() {
                let app_for_others = app.name.clone();
                let ruled_out: HashSet<String> = match self.db.call(move |db| {
                    crate::runtime::history::display_names_of_other_apps(db, &app_for_others)
                }) {
                    Ok(names) => names
                        .into_iter()
                        .map(|display_name| format!("seedling-{display_name}.service"))
                        .collect(),
                    Err(e) => {
                        warn!(
                            app = %app.name,
                            "uninstall: could not load the other apps' recorded identities; \
                             not advancing teardown this tick: {e}"
                        );
                        self.teardown_identities_unreadable(&app.name, &e.to_string());
                        continue;
                    }
                };
                match self.driver.process.list_units("seedling-").await {
                    Ok(units) if !units_possibly_of(&units, &app.name, &ruled_out).is_empty() => {
                        if !self.hold_for_unidentifiable_units(&app.name) {
                            continue;
                        }
                        // An operator cleared the fault: teardown completes
                        // and the units that were not ruled out stay as they are.
                        warn!(
                            app = %app.name,
                            "uninstall: completing teardown at an operator's instruction, \
                             leaving the units that could not be ruled out"
                        );
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
                    // spec: TRD#teardown
                    // Teardown completes when every recorded instance is
                    // `Unscheduled`; no unit being loaded is not the same.
                    let app_for_check = app.name.clone();
                    match self.db.call(move |db| {
                        crate::runtime::history::teardown_complete(db, &app_for_check)
                    }) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(e) => {
                            warn!(
                                app = %app.name,
                                "uninstall: could not read the lifecycle of the recorded \
                                 instances; not advancing teardown this tick: {e}"
                            );
                            self.teardown_identities_unreadable(&app.name, &e.to_string());
                            continue;
                        }
                    }
                    // spec: ACTU#deployment-and-job-instances
                    // spec: TRD#teardown
                    // A pod's anonymous volume whose removal failed is known
                    // of only by its record, which goes with the recorded
                    // instances: the teardown waits for its removal.
                    if !self.remove_recorded_anonymous_volumes(app).await {
                        continue;
                    }
                    // spec: GRUN#uninstall
                    if !gate_state_deleted {
                        continue;
                    }
                    let app_name_owned = app.name.clone();
                    let phase_handle = Arc::clone(&app.phase_handle);
                    let moved = self.db.call(move |db| {
                        // spec: TRD#teardown
                        // The recorded instances go first, and a failure to
                        // delete them holds the teardown: an app left with
                        // instances recorded would pass them on to the next
                        // app registered under its name.
                        if let Err(e) = crate::runtime::history::delete_instances_for_app(
                            db,
                            &app_name_owned,
                        ) {
                            error!(
                                app = %app_name_owned,
                                "uninstall: could not forget the recorded instances; trying again next tick: {e}"
                            );
                            return false;
                        }
                        if let Err(e) = transition_phase(
                            &phase_handle,
                            AppPhase::NotInstalled,
                            db,
                            &app_name_owned,
                        ) {
                            error!(
                                app = %app_name_owned,
                                "uninstall: could not record that teardown finished; trying again next tick: {e}"
                            );
                            return false;
                        }
                        // The other half of `uninstall_unidentifiable_units`:
                        // teardown finishing is the only thing that resolves
                        // it, and it must not outlive the app it describes.
                        for kind in [
                            "uninstall_unidentifiable_units",
                            "teardown_identities_unreadable",
                        ] {
                            let _ = crate::runtime::faults::clear_faults_by_kind(
                                db,
                                &app_name_owned,
                                kind,
                            );
                        }
                        true
                    });
                    if !moved {
                        continue;
                    }
                    self.unidentifiable_units_filed.remove(&app.name);
                    self.gate_runtime.forget_app(&app.name);
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
                        .chain(app.desired.undeclared.iter().map(|instance| instance.id))
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

/// Apply this tick's routes, or `None` when the build did not cover every
/// contributor and the apply is withheld.
// spec: RCN#applying-absolute-state
async fn apply_routes_plane(
    data_plane: &dyn super::DataPlane,
    routes: &[crate::system::types::ServiceRoute],
    coverage: Coverage,
) -> Option<Result<(), super::BoxError>> {
    match coverage.is_complete() {
        true => Some(data_plane.apply_routes(routes).await),
        false => None,
    }
}

/// Apply this tick's packet-filtering rules, or `None` when the build did not
/// cover every contributor and the apply is withheld.
// spec: RCN#applying-absolute-state
async fn apply_rules_plane(
    data_plane: &dyn super::DataPlane,
    rules: &crate::system::types::DataPlaneRules,
    coverage: Coverage,
) -> Option<Result<(), super::BoxError>> {
    match coverage.is_complete() {
        true => Some(data_plane.apply_rules(rules).await),
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

        fn dtls_refusals<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<std::collections::BTreeMap<String, jiff::Timestamp>, BoxError>>
        {
            Box::pin(async { Ok(Default::default()) })
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
            !units_possibly_of(&units, &app, &HashSet::new()).is_empty(),
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
            units_possibly_of(&units, &app, &HashSet::new()).is_empty(),
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
            units_possibly_of(&units, &app, &HashSet::new()).is_empty(),
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
            !units_possibly_of(&units, &app, &HashSet::new()).is_empty(),
            "`seedling-app-` also matches `app-db`; over-matching here only withholds a \
             completion, never stops a sibling's unit — the decision itself is still the exact \
             match against recorded identity"
        );
    }
    use std::collections::HashMap;

    use crate::{
        defs::gate::GateKeyDigests,
        runtime::registry::{RegistryError, ScaledGroup},
    };

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

        fn instances_of_app(
            &self,
            app: &AppName,
        ) -> Result<Vec<crate::runtime::identity::ResourceInstance>, RegistryError> {
            if app == &self.broken {
                return Err(RegistryError::message("registry unavailable"));
            }
            self.inner.instances_of_app(app)
        }

        fn service_address(
            &self,
            node_prefix: &Ipv6Net,
            instance: &crate::runtime::identity::ResourceInstance,
        ) -> Result<Ipv6Addr, RegistryError> {
            if instance.app == self.broken {
                return Err(RegistryError::message("registry unavailable"));
            }
            self.inner.service_address(node_prefix, instance)
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
        let app_def = app.def.load_full();
        AppSnapshot {
            name: app_name,
            desired: DesiredState::default(),
            steady: true,
            declarations: Arc::clone(&app_def),
            app_def,
            phase: AppPhase::Installed,
            phase_handle: Arc::new(Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            app_priority: AppPriority::default(),
            gate_keys: Default::default(),
            gates: Default::default(),
            install_seq: Some(1),
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
        let app_def = app.def.load_full();
        AppSnapshot {
            name: app_name,
            desired: DesiredState::default(),
            steady: true,
            declarations: Arc::clone(&app_def),
            app_def,
            phase: AppPhase::Installed,
            phase_handle: Arc::new(Mutex::new(AppPhase::Installed)),
            warm_cert_hostnames: Default::default(),
            app_priority,
            gate_keys: Default::default(),
            gates: Default::default(),
            install_seq: Some(1),
        }
    }

    fn volume_instance(app: &str, name: &str) -> crate::runtime::identity::ResourceInstance {
        crate::runtime::identity::ResourceInstance::new_singleton(
            AppName::new(app).unwrap(),
            crate::defs::resource::ResourceKind::Volume,
            name,
        )
    }

    fn volume_update() -> volumes::VolumeActuationUpdate {
        volumes::VolumeActuationUpdate {
            observations: Vec::new(),
            external_volume_states: Vec::new(),
            observe_failures: Vec::new(),
            observed: Vec::new(),
            create_failures: Vec::new(),
            remove_failures: Vec::new(),
            present: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn active_kinds(oi: &crate::oi::test_support::TestOi, app: &str) -> Vec<(String, String)> {
        let app = AppName::new_unchecked(app);
        oi.state.db.call(move |db| {
            crate::runtime::faults::list_active_faults(db, Some(&app))
                .unwrap()
                .into_iter()
                .map(|f| (f.kind, f.subject))
                .collect()
        })
    }

    // spec: FLT#host-faults
    // spec: RCN#applying-absolute-state
    #[test]
    fn a_packet_filter_fault_follows_the_applies_and_a_withheld_one_moves_nothing() {
        let oi = crate::oi::test_support::TestOi::new();
        let reconciler = test_reconciler(&oi);
        let filed = || {
            active_kinds(&oi, "_system")
                .iter()
                .any(|(kind, _)| kind == "packet_filter_failed")
        };

        reconciler.settle_plane_fault("packet_filter_failed", "apply_rules", None);
        assert!(!filed(), "a withheld apply files nothing");

        reconciler.settle_plane_fault(
            "packet_filter_failed",
            "apply_rules",
            Some(Err("the ruleset was refused".into())),
        );
        assert!(filed(), "a failed apply files the fault");

        reconciler.settle_plane_fault("packet_filter_failed", "apply_rules", None);
        assert!(filed(), "a withheld apply clears nothing");

        reconciler.settle_plane_fault("packet_filter_failed", "apply_rules", Some(Ok(())));
        assert!(!filed(), "the next successful apply clears it");
    }

    // spec: FLT#volume-faults
    #[test]
    fn volume_create_and_remove_failures_clear_when_the_volume_is_created_or_removed() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        let data = volume_instance("demo", "data");
        let other = volume_instance("demo", "other");

        let mut failing = volume_update();
        failing.create_failures = vec![
            (data.clone(), "no space".into()),
            (other.clone(), "no space".into()),
        ];
        failing.remove_failures = vec![(data.clone(), "busy".into())];
        reconciler.file_volume_actuation_faults(&app, &failing);
        assert_eq!(active_kinds(&oi, "demo").len(), 3);

        // `data` is created and `other` is not: only its own fault clears.
        let mut recovered = volume_update();
        recovered.present = vec![data.clone()];
        reconciler.file_volume_actuation_faults(&app, &recovered);
        let held = active_kinds(&oi, "demo");
        assert!(!held.contains(&("volume_create_failed".into(), data.id.to_hex())));
        assert!(held.contains(&("volume_create_failed".into(), other.id.to_hex())));
        assert!(held.contains(&("volume_remove_failed".into(), data.id.to_hex())));

        let mut removed = volume_update();
        removed.removed = vec![data.clone()];
        reconciler.file_volume_actuation_faults(&app, &removed);
        assert!(
            !active_kinds(&oi, "demo").contains(&("volume_remove_failed".into(), data.id.to_hex()))
        );
    }

    // spec: FLT#volume-faults
    // spec: OBS#failed-observation
    #[test]
    fn volume_observation_failures_fault_on_the_third_and_survive_other_apps_ticks() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        let data = volume_instance("demo", "data");
        let bystander = volume_instance("elsewhere", "data");
        let other_app = AppName::new("elsewhere").unwrap();

        let mut failing = volume_update();
        failing.observe_failures = vec![(data.clone(), "podman".into())];
        let mut bystander_ok = volume_update();
        bystander_ok.observed = vec![bystander.clone()];
        for tick in 1..=3 {
            reconciler.file_volume_actuation_faults(&app, &failing);
            // Another app's pass between the two must not reset the streak.
            reconciler.file_volume_actuation_faults(&other_app, &bystander_ok);
            let held = active_kinds(&oi, "demo");
            assert_eq!(
                held.contains(&("observe_failed".into(), data.id.to_hex())),
                tick == 3,
                "tick {tick}: {held:?}"
            );
        }

        let mut ok = volume_update();
        ok.observed = vec![data.clone()];
        reconciler.file_volume_actuation_faults(&app, &ok);
        assert!(active_kinds(&oi, "demo").is_empty());
    }

    // spec: FLT#certificate-faults
    #[test]
    fn cert_issue_failed_clears_for_a_hostname_no_ingress_needs() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let reconciler = test_reconciler(&oi);
        let system = AppName::new_unchecked("_system");
        oi.state.db.call(|db| {
            let system = AppName::new_unchecked("_system");
            for host in ["kept.example.com", "gone.example.com"] {
                crate::runtime::faults::file_fault(
                    db,
                    &system,
                    Some("hostname"),
                    Some(host),
                    None,
                    "cert_issue_failed",
                    "boom",
                )
                .unwrap();
            }
        });

        reconciler.clear_cert_issue_faults_not_needed(["kept.example.com".to_owned()].into());
        let held: Vec<String> = oi.state.db.call(move |db| {
            crate::runtime::faults::list_active_faults(db, Some(&system))
                .unwrap()
                .into_iter()
                .map(|f| f.subject)
                .collect()
        });
        assert_eq!(held, vec!["kept.example.com".to_owned()]);
    }

    // spec: FLT#workload-faults
    #[test]
    fn image_pull_failed_is_keyed_by_reference_and_clears_when_the_image_is_present() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        let first = volume_instance("demo", "one");
        let second = volume_instance("demo", "two");
        let image = "docker.io/library/nginx:1.29";

        // Two instances needing one broken image make one fault, named by the image.
        let update = pods::PodActuationUpdate {
            image_pull_failures: vec![
                (first.clone(), image.to_owned()),
                (second.clone(), image.to_owned()),
            ],
            ..Default::default()
        };
        reconciler.file_image_pull_faults(&app, &update);
        reconciler.file_image_pull_faults(&app, &update);
        assert_eq!(
            active_kinds(&oi, "demo"),
            vec![("image_pull_failed".to_owned(), image.to_owned())]
        );

        // The image turning up on the host clears it, whoever put it there.
        oi.block_on(async {
            oi.state.container_runtime.pull_image(image).await.unwrap();
            reconciler.reconcile_images(std::iter::empty()).await;
        });
        assert!(active_kinds(&oi, "demo").is_empty());
    }

    // spec: FLT#ingress-and-service-faults
    #[test]
    fn site_ingress_target_missing_is_keyed_by_attachment() {
        use crate::runtime::site_ingress_attachments::AttachmentProtocol;
        let oi = crate::oi::test_support::TestOi::new();
        let reconciler = test_reconciler(&oi);
        let attachment =
            |ingress: &str, port: u16, reason: &str| site_proxy::UnresolvedAttachment {
                site_ingress: ingress.to_owned(),
                port,
                protocol: AttachmentProtocol::Tcp,
                reason: reason.to_owned(),
            };
        let held = || -> Vec<String> {
            let mut subjects: Vec<String> = active_kinds(&oi, "_system")
                .into_iter()
                .filter(|(k, _)| k == "site_ingress_target_missing")
                .map(|(_, s)| s)
                .collect();
            subjects.sort();
            subjects
        };

        // `edge` and `edge-2` share a name prefix, and one ingress has two ports.
        reconciler.reconcile_unresolved_site_attachments(&[
            attachment("edge", 80, "no service"),
            attachment("edge", 81, "no service"),
            attachment("edge-2", 80, "no service"),
        ]);
        assert_eq!(held().len(), 3);

        // The reason changing leaves one fault, and resolving one clears only it.
        reconciler.reconcile_unresolved_site_attachments(&[
            attachment("edge", 80, "unbound"),
            attachment("edge-2", 80, "no service"),
        ]);
        assert_eq!(
            held(),
            vec!["edge-2:80:tcp".to_owned(), "edge:80:tcp".to_owned()]
        );
    }

    // spec: FLT#filing-and-clearing
    #[test]
    fn clearing_desired_state_unreadable_leaves_instance_identity_unreadable() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        let instance = volume_instance("demo", "web");
        oi.state.db.call({
            let (app, instance) = (app.clone(), instance.clone());
            move |db| {
                crate::runtime::faults::file_fault(
                    db,
                    &app,
                    None,
                    None,
                    None,
                    "desired_state_unreadable",
                    "lookup",
                )
                .unwrap();
                crate::runtime::faults::file_fault(
                    db,
                    &app,
                    Some("deployment"),
                    Some("web"),
                    Some(&instance.id.to_hex()),
                    "instance_identity_unreadable",
                    "lookup",
                )
                .unwrap();
            }
        });

        reconciler.clear_desired_state_fault(&app);
        assert_eq!(
            active_kinds(&oi, "demo"),
            vec![(
                "instance_identity_unreadable".to_owned(),
                instance.id.to_hex()
            )]
        );
    }

    // spec: FLT#workload-faults
    #[test]
    fn the_replacement_guard_clears_on_a_generation_made_while_the_daemon_was_down() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        reconciler.file_replace_failed_fault(&app, "web", "web-2");
        reconciler.update_replace_state(&[], &HashMap::new());
        assert_eq!(
            active_kinds(&oi, "demo").len(),
            1,
            "no new generation: it holds"
        );

        // A new generation is made after the fault, then the process restarts:
        // a fresh reconciler has no memory of the generation it last saw.
        std::thread::sleep(std::time::Duration::from_millis(5));
        oi.state.db.call({
            let app = app.clone();
            move |db| {
                crate::runtime::generations::update_script(
                    db,
                    &app,
                    r#"app.deployment("web").image("docker.io/library/nginx:1.30");"#,
                )
                .unwrap()
            }
        });
        let mut restarted = test_reconciler(&oi);
        restarted.update_replace_state(&[], &HashMap::new());
        assert!(active_kinds(&oi, "demo").is_empty());
        assert!(restarted.replace_failed.is_empty());
        let _ = &mut reconciler;
    }

    // spec: INFRA#pod-network-sources
    // spec: INFRA#forwarding-policy
    // spec: INFRA#pod-traffic-to-the-host
    // spec: FLT#host-faults
    #[test]
    fn no_workload_is_started_until_the_isolation_rules_are_installed() {
        let dir = tempfile::tempdir().unwrap();
        let container = Arc::new(crate::system::stub::StubContainerRuntime::new(
            dir.path().join("vols"),
        ));
        let process = Arc::new(crate::system::stub::StubProcessManager::new(Arc::clone(
            &container,
        )));
        let backends = crate::system::backends::Backends::working(container, process);
        let plane = Arc::new(crate::system::stub::StubDataPlane::default());
        plane.refuse_isolation(true);
        let mut system = crate::system::System::for_tests(
            backends,
            Arc::new(crate::system::stub::StubNetworkProxy),
            dir.path(),
        );
        system.data_plane = Arc::clone(&plane) as _;
        let system = Arc::new(system);
        let oi = crate::oi::test_support::TestOi::with_driver(Arc::clone(&system));
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"app.deployment("web").image("docker.io/library/nginx:1.29");"#,
            }),
        )
        .unwrap();
        *oi.state.registry.read().get("demo").unwrap().phase.lock() = AppPhase::Installed;
        let mut reconciler = test_reconciler(&oi);
        let isolation_fault = |oi: &crate::oi::test_support::TestOi| {
            active_kinds(oi, "_system")
                .contains(&("packet_filter_failed".to_owned(), "isolation".to_owned()))
        };
        let units = |oi: &crate::oi::test_support::TestOi| {
            oi.block_on(oi.state.driver.process.list_units("seedling-"))
                .unwrap()
        };

        assert!(oi.block_on(reconciler.tick()));
        assert!(isolation_fault(&oi), "{:?}", active_kinds(&oi, "_system"));
        assert!(units(&oi).is_empty(), "started {:?}", units(&oi));
        assert!(
            oi.block_on(oi.state.driver.ensure_isolation()).is_err(),
            "a shell session would not start"
        );

        plane.refuse_isolation(false);
        oi.block_on(system.isolation.retry_now());
        oi.block_on(reconciler.tick());
        assert!(!isolation_fault(&oi), "cleared once installed");
        // The resolver comes up ahead of any workload, so its start shows the
        // iteration has gone past the gate.
        assert!(
            units(&oi)
                .iter()
                .any(|u| u.name.starts_with("seedling-resolver-")),
            "actuation goes on once the rules are installed: {:?}",
            units(&oi)
        );
    }

    // spec: FLT#filing-and-clearing
    #[test]
    fn the_isolation_and_rule_apply_faults_are_filed_and_cleared_apart() {
        let oi = crate::oi::test_support::TestOi::new();
        let reconciler = test_reconciler(&oi);
        let filed = |subject: &str| {
            active_kinds(&oi, "_system")
                .contains(&("packet_filter_failed".to_owned(), subject.to_owned()))
        };

        reconciler.file_system_fault_for(
            "packet_filter_failed",
            ISOLATION_FAULT_SUBJECT,
            "refused",
        );
        reconciler.settle_plane_fault(
            "packet_filter_failed",
            "apply_rules",
            Some(Err("the ruleset was refused".into())),
        );
        assert!(filed(ISOLATION_FAULT_SUBJECT) && filed(""));

        reconciler.settle_plane_fault("packet_filter_failed", "apply_rules", Some(Ok(())));
        assert!(
            filed(ISOLATION_FAULT_SUBJECT),
            "an apply clears only its own fault"
        );
        assert!(!filed(""));

        reconciler.clear_system_fault_for("packet_filter_failed", ISOLATION_FAULT_SUBJECT);
        assert!(!filed(ISOLATION_FAULT_SUBJECT));
    }

    // spec: DNS#resolver-lifecycle
    #[test]
    fn no_workload_is_started_while_the_resolver_is_not_up() {
        // Reachable backends that cannot bring a resolver up: its container
        // runtime fails every call, as one that has gone away mid-run does.
        let dir = tempfile::tempdir().unwrap();
        let container: Arc<dyn crate::system::ContainerRuntime> =
            Arc::new(crate::system::unavailable::UnavailableContainerRuntime);
        let process = Arc::new(crate::system::stub::StubProcessManager::new(Arc::new(
            crate::system::stub::StubContainerRuntime::new(dir.path().join("vols")),
        )));
        let backends = crate::system::backends::Backends::working(container, process);
        let system = Arc::new(crate::system::System::for_tests(
            backends,
            Arc::new(crate::system::stub::StubNetworkProxy),
            dir.path(),
        ));
        let oi = crate::oi::test_support::TestOi::with_driver(system);
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"app.deployment("web").image("docker.io/library/nginx:1.29");"#,
            }),
        )
        .unwrap();
        *oi.state.registry.read().get("demo").unwrap().phase.lock() = AppPhase::Installed;
        let mut reconciler = test_reconciler(&oi);

        assert!(oi.block_on(reconciler.tick()));
        let held = active_kinds(&oi, "_system");
        assert!(held.iter().any(|(k, _)| k == "resolver_failed"), "{held:?}");
        assert!(
            active_kinds(&oi, "demo").is_empty(),
            "a workload was started without a healthy resolver"
        );
    }

    // spec: INFRA#unavailable-backends
    // spec: FLT#host-faults
    #[test]
    fn container_runtime_unavailable_is_filed_while_down_and_cleared_once_both_are_up() {
        use crate::system::backends::tests::{Flaky, down_backends};

        let flaky = Flaky::down();
        let backends = down_backends(&flaky);
        let dir = tempfile::tempdir().unwrap();
        let system = Arc::new(crate::system::System::for_tests(
            backends,
            Arc::new(crate::system::stub::StubNetworkProxy),
            dir.path(),
        ));
        let oi = crate::oi::test_support::TestOi::with_driver(Arc::clone(&system));
        let mut reconciler = test_reconciler(&oi);
        let held = |oi: &crate::oi::test_support::TestOi| -> bool {
            oi.state.db.call(|db| {
                crate::runtime::faults::list_active_faults(
                    db,
                    Some(&AppName::new_unchecked("_system")),
                )
                .unwrap()
                .iter()
                .any(|f| f.kind == "container_runtime_unavailable")
            })
        };

        assert!(oi.block_on(reconciler.tick()));
        assert!(held(&oi), "filed while a backend is unreachable");

        flaky.up.store(true, std::sync::atomic::Ordering::SeqCst);
        system.backends.probe_now();
        oi.block_on(reconciler.tick());
        assert!(!held(&oi), "cleared once both are reachable");
    }

    /// A host whose probes fail until told to answer, and then say the host has
    /// none of NAT64, IPv4 or IPv6 and a UTC time zone. The facts match the defaults every other
    /// test reads, so determining them here moves nothing there.
    struct ProbesThatSettle(std::sync::atomic::AtomicBool);

    impl crate::system::host_probes::ProbeSource for ProbesThatSettle {
        fn external_nat64(&self) -> crate::system::BoxFuture<'_, Result<bool, String>> {
            Box::pin(async move { self.answer() })
        }
        fn ipv4_egress(&self) -> crate::system::BoxFuture<'_, Result<bool, String>> {
            Box::pin(async move { self.answer() })
        }
        fn ipv6_egress(&self) -> crate::system::BoxFuture<'_, Result<bool, String>> {
            Box::pin(async move { self.answer() })
        }
        fn timezone(&self) -> crate::system::BoxFuture<'_, Result<String, String>> {
            Box::pin(async move { self.answer().map(|_| "UTC".to_owned()) })
        }
        fn available_threads(&self) -> crate::system::BoxFuture<'_, Result<i64, String>> {
            Box::pin(async move { self.answer().map(|_| 2) })
        }
        fn available_memory(&self) -> crate::system::BoxFuture<'_, Result<i64, String>> {
            Box::pin(async move { self.answer().map(|_| 1 << 30) })
        }
        fn node_name(&self) -> crate::system::BoxFuture<'_, Result<String, String>> {
            Box::pin(async move { self.answer().map(|_| "host".to_owned()) })
        }
    }

    impl ProbesThatSettle {
        fn answer(&self) -> Result<bool, String> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(false)
            } else {
                Err("cannot be read".to_owned())
            }
        }
    }

    // spec: SITE#endpoint-addressing
    #[test]
    fn site_endpoints_route_as_without_ipv6_egress_unless_it_is_determined() {
        use crate::system::host_probes::{Fact, HostProbes};

        let oi = crate::oi::test_support::TestOi::new();
        let probes = |ipv6: Fact<bool>| {
            HostProbes::new(
                Arc::new(ProbesThatSettle(std::sync::atomic::AtomicBool::new(false))),
                Some(Fact::Determined(false)),
                Fact::Determined(true),
                ipv6,
                Fact::Determined("UTC".to_owned()),
                crate::system::host_probes::HostConstants::settled(),
            )
        };
        let undetermined =
            test_reconciler(&oi).with_host_probes(probes(Fact::Undetermined("no routes".into())));
        assert!(!undetermined.routes_with_ipv6_egress());
        let without = test_reconciler(&oi).with_host_probes(probes(Fact::Determined(false)));
        assert!(!without.routes_with_ipv6_egress());
        let with = test_reconciler(&oi).with_host_probes(probes(Fact::Determined(true)));
        assert!(with.routes_with_ipv6_egress());
    }

    // spec: NAT64#detection
    // spec: NAT64#ipv6-egress
    // spec: CONST#determining-host-constants
    // spec: FLT#host-faults
    #[test]
    fn undetermined_host_facts_are_faulted_and_looked_for_on_each_tick() {
        use crate::system::host_probes::{Fact, HostProbes};

        let source = Arc::new(ProbesThatSettle(std::sync::atomic::AtomicBool::new(false)));
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi).with_host_probes(HostProbes::new(
            source.clone(),
            Some(Fact::Undetermined("no answer".into())),
            Fact::Undetermined("no routes".into()),
            Fact::Undetermined("no routes".into()),
            Fact::Undetermined("no zone".into()),
            crate::system::host_probes::HostConstants {
                available_threads: Fact::Undetermined("no answer".into()),
                available_memory: Fact::Undetermined("no answer".into()),
                node_name: Fact::Undetermined("no answer".into()),
            },
        ));
        let kinds = |oi: &crate::oi::test_support::TestOi| -> Vec<String> {
            let mut kinds: Vec<String> = active_kinds(oi, "_system")
                .into_iter()
                .map(|(k, _)| k)
                .filter(|k| {
                    k.contains("egress")
                        || k == "nat64_detection_failed"
                        || k == "timezone_undetermined"
                        || k == "host_constant_undetermined"
                })
                .collect();
            kinds.sort();
            kinds
        };

        oi.block_on(reconciler.tick());
        assert_eq!(
            kinds(&oi),
            [
                "host_constant_undetermined",
                "ipv4_egress_undetermined",
                "ipv6_egress_undetermined",
                "nat64_detection_failed",
                "timezone_undetermined"
            ],
        );
        let described: Vec<String> = oi.state.db.call(|db| {
            crate::runtime::faults::list_active_faults(db, Some(&AppName::new_unchecked("_system")))
                .unwrap()
                .into_iter()
                .filter(|f| f.kind == "host_constant_undetermined")
                .map(|f| f.description)
                .collect()
        });
        assert_eq!(described.len(), 1);
        for name in ["AVAILABLE_THREADS", "AVAILABLE_MEMORY", "NODE_NAME"] {
            assert!(described[0].contains(name), "{described:?}");
        }
        assert!(!reconciler.take_host_facts_changed());

        source.0.store(true, std::sync::atomic::Ordering::SeqCst);
        oi.block_on(reconciler.tick());
        assert!(kinds(&oi).is_empty(), "{:?}", kinds(&oi));
        assert!(
            reconciler.take_host_facts_changed(),
            "a determined fact is due to re-evaluate the installed apps"
        );
        assert!(!reconciler.take_host_facts_changed());
    }

    // spec: CTL#resource-stop
    // spec: FLT#app-faults
    #[test]
    fn an_app_whose_stops_cannot_be_read_is_not_actuated() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        oi.install("demo");
        let reconciler = test_reconciler(&oi);
        oi.state.db.call(|db| {
            db.conn
                .execute_batch("DROP TABLE stopped_resources;")
                .unwrap()
        });

        let (apps, skipped) = reconciler.snapshot_all_apps();
        assert!(
            apps.is_empty(),
            "an unreadable set of stops would read as nothing stopped"
        );
        assert_eq!(skipped, 1);
        let kinds: Vec<String> = oi.state.db.call(|db| {
            crate::runtime::faults::list_active_faults(db, None)
                .unwrap()
                .into_iter()
                .map(|f| f.kind)
                .collect()
        });
        assert_eq!(kinds, vec!["desired_state_unreadable".to_owned()]);
    }

    // spec: AUTO#unreadable-scale
    #[test]
    fn a_tick_that_could_not_read_the_scale_is_not_idle() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        oi.install("demo");
        let mut reconciler = test_reconciler(&oi);
        oi.state.db.call(|db| {
            db.conn
                .execute_batch("DROP TABLE scaling_decisions;")
                .unwrap()
        });

        let active = oi.block_on(reconciler.tick());
        assert!(
            active,
            "an idle loop waits for a notification that nothing sends when a read fails"
        );
    }

    // spec: LCY#external-volumes
    // spec: HIST#observations
    #[test]
    fn an_external_volume_takes_its_state_from_the_desired_state_and_is_never_observed() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "demo", "script": r#"app.external_volume("data");"# }),
        )
        .unwrap();
        oi.install("demo");
        let mut reconciler = test_reconciler(&oi);
        let app = AppName::new("demo").unwrap();
        async fn volumes_phase(reconciler: &mut Reconciler) {
            let (apps, _) = reconciler.snapshot_all_apps();
            let updates = phases::run_volumes_phase(
                &reconciler.observer,
                &reconciler.actuator,
                &reconciler.db,
                &apps,
            )
            .await;
            reconciler.ingest_volume_results(&apps, updates);
        }
        let instance = |oi: &crate::oi::test_support::TestOi| {
            let app = app.clone();
            oi.state
                .db
                .call(move |db| {
                    crate::runtime::history::find_instances_for_group(
                        db,
                        &app,
                        crate::defs::resource::ResourceKind::ExternalVolume,
                        Some("data"),
                    )
                })
                .unwrap()
                .pop()
        };
        let read = |oi: &crate::oi::test_support::TestOi,
                    inst: crate::runtime::ResourceInstance| {
            oi.state.db.call(move |db| {
                let state = crate::runtime::history::read_lifecycle(db, &inst)
                    .unwrap()
                    .0;
                let observations = crate::runtime::history::query_observations(db, &inst)
                    .unwrap()
                    .len();
                (state, observations)
            })
        };

        oi.block_on(volumes_phase(&mut reconciler));
        let inst = instance(&oi).expect("the reconciler gave the declaration an instance");
        assert_eq!(read(&oi, inst.clone()), (LifecycleState::Ready, 0));

        let app_for_stop = app.clone();
        oi.state.db.call(move |db| {
            crate::runtime::stopped::stop_resource(
                db,
                &app_for_stop,
                crate::defs::resource::ResourceKind::ExternalVolume,
                "data",
            )
            .unwrap();
        });
        oi.block_on(volumes_phase(&mut reconciler));
        assert_eq!(read(&oi, inst.clone()), (LifecycleState::Unscheduled, 0));

        let app_for_unstop = app.clone();
        oi.state.db.call(move |db| {
            crate::runtime::stopped::unstop_all(db, &app_for_unstop).unwrap();
        });
        oi.block_on(volumes_phase(&mut reconciler));
        assert_eq!(read(&oi, inst.clone()), (LifecycleState::Ready, 0));

        *oi.state.registry.read().get("demo").unwrap().phase.lock() = AppPhase::Uninstalling;
        oi.block_on(volumes_phase(&mut reconciler));
        assert_eq!(read(&oi, inst), (LifecycleState::Unscheduled, 0));
    }

    /// An uninstalling app named `name` registered in `oi`, with the recorded
    /// instances `instances` (kind and name).
    fn uninstalling(
        oi: &crate::oi::test_support::TestOi,
        name: &str,
        script: &str,
        instances: &[(crate::defs::resource::ResourceKind, &str)],
    ) -> Vec<crate::runtime::identity::ResourceInstance> {
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": name, "script": script }),
        )
        .unwrap();
        let app = AppName::new(name).unwrap();
        let recorded = instances
            .iter()
            .map(|(kind, instance_name)| {
                let (app, kind, instance_name) = (app.clone(), *kind, (*instance_name).to_owned());
                oi.state.db.call(move |db| {
                    crate::runtime::history::get_or_create_singleton(
                        db,
                        &app,
                        kind,
                        Some(&instance_name),
                    )
                    .unwrap()
                })
            })
            .collect();
        *oi.state.registry.read().get(name).unwrap().phase.lock() = AppPhase::Uninstalling;
        recorded
    }

    fn uninstall_phase(oi: &crate::oi::test_support::TestOi, reconciler: &mut Reconciler) {
        let (apps, _) = reconciler.snapshot_all_apps();
        oi.block_on(reconciler.run_uninstall_phase(&apps, &HashMap::new()));
    }

    fn phase_of(oi: &crate::oi::test_support::TestOi, name: &str) -> AppPhase {
        oi.state
            .registry
            .read()
            .get(name)
            .unwrap()
            .phase
            .lock()
            .clone()
    }

    fn start_unit(oi: &crate::oi::test_support::TestOi, name: &str) {
        oi.block_on(oi.state.driver.process.start_unit(name))
            .unwrap();
    }

    // spec: TRD#uninstall-scope
    #[test]
    fn a_unit_matching_another_apps_recorded_identity_does_not_hold_an_uninstall() {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        uninstalling(&oi, "app", crate::oi::test_support::MINIMAL_SCRIPT, &[]);
        uninstalling(
            &oi,
            "app-db",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        *oi.state.registry.read().get("app-db").unwrap().phase.lock() = AppPhase::Installed;
        start_unit(&oi, "seedling-app-db-web.service");
        let mut reconciler = test_reconciler(&oi);

        uninstall_phase(&oi, &mut reconciler);

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        let units = oi
            .block_on(oi.state.driver.process.list_units("seedling-"))
            .unwrap();
        assert!(
            units
                .iter()
                .any(|u| u.name == "seedling-app-db-web.service"),
            "the sibling's unit is left alone"
        );
    }

    // spec: TRD#uninstall-scope
    #[test]
    fn clearing_the_unidentifiable_units_fault_completes_the_teardown_without_filing_it_again() {
        let oi = crate::oi::test_support::TestOi::new();
        uninstalling(&oi, "app", crate::oi::test_support::MINIMAL_SCRIPT, &[]);
        start_unit(&oi, "seedling-app-orphan.service");
        let mut reconciler = test_reconciler(&oi);
        let app = AppName::new("app").unwrap();
        let active = |oi: &crate::oi::test_support::TestOi| {
            let app = app.clone();
            oi.state.db.call(move |db| {
                crate::runtime::faults::list_active_faults(db, Some(&app))
                    .unwrap()
                    .into_iter()
                    .filter(|f| f.kind == "uninstall_unidentifiable_units")
                    .count()
            })
        };

        uninstall_phase(&oi, &mut reconciler);
        assert_eq!(phase_of(&oi, "app"), AppPhase::Uninstalling);
        assert_eq!(active(&oi), 1);
        uninstall_phase(&oi, &mut reconciler);
        assert_eq!(phase_of(&oi, "app"), AppPhase::Uninstalling);
        assert_eq!(active(&oi), 1, "one fault, however many iterations");

        let key = crate::runtime::faults::FaultKey::new(&app, "uninstall_unidentifiable_units", "");
        oi.state
            .db
            .call(move |db| crate::runtime::faults::clear_fault_by_key(db, &key))
            .unwrap();
        uninstall_phase(&oi, &mut reconciler);

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        assert_eq!(active(&oi), 0, "not filed again for that uninstall");
        let units = oi
            .block_on(oi.state.driver.process.list_units("seedling-"))
            .unwrap();
        assert!(
            units
                .iter()
                .any(|u| u.name == "seedling-app-orphan.service"),
            "what was not ruled out is left untouched"
        );
    }

    /// How many `teardown_identities_unreadable` faults have been filed for
    /// `app`, and how many of those are active.
    fn identities_unreadable(oi: &crate::oi::test_support::TestOi, app: &str) -> (i64, i64) {
        let app = app.to_owned();
        oi.state.db.call(move |db| {
            db.conn
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(cleared_at IS NULL), 0) FROM faults
                     WHERE app = ?1 AND kind = 'teardown_identities_unreadable'",
                    [&app],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap()
        })
    }

    /// The reads of the uninstalling apps' recorded identities that one
    /// iteration makes, in its order: the snapshot, then the uninstall phase
    /// with `running` the workloads still running.
    fn teardown_iteration(
        oi: &crate::oi::test_support::TestOi,
        reconciler: &mut Reconciler,
        running: &HashMap<AppName, Vec<RunningPod>>,
    ) {
        let (apps, _) = reconciler.snapshot_all_apps();
        reconciler.note_teardown_identities_read(&apps);
        oi.block_on(reconciler.run_uninstall_phase(&apps, running));
        reconciler.settle_teardown_identities_read();
    }

    /// `instance` as a workload still running.
    fn still_running(instance: &crate::runtime::identity::ResourceInstance) -> RunningPod {
        RunningPod {
            instance: instance.clone(),
            pod_prefix: "fd00:1:2:3::/64".parse().unwrap(),
            pod_ip: "fd00:1:2:3::2".parse().unwrap(),
            resource: crate::runtime::desired::undeclared_pod(instance).unwrap(),
            observed_healthy: true,
        }
    }

    // spec: TRD#uninstall-scope
    // spec: FLT#app-faults
    // The uninstall phase does not run for an app while its workloads are
    // still running, so a clear left to it held the fault through the whole
    // of their stop.
    #[test]
    fn teardown_identities_unreadable_clears_on_the_next_read_while_workloads_still_run() {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        let recorded = uninstalling(
            &oi,
            "app",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let mut reconciler = test_reconciler(&oi);
        rename_table(&oi, "resource_instances", "hidden_resource_instances");
        teardown_iteration(&oi, &mut reconciler, &HashMap::new());
        assert_eq!(identities_unreadable(&oi, "app"), (1, 1));

        rename_table(&oi, "hidden_resource_instances", "resource_instances");
        let running = HashMap::from([(
            AppName::new("app").unwrap(),
            vec![still_running(&recorded[0])],
        )]);
        teardown_iteration(&oi, &mut reconciler, &running);

        assert_eq!(phase_of(&oi, "app"), AppPhase::Uninstalling);
        assert_eq!(
            identities_unreadable(&oi, "app"),
            (1, 0),
            "the recorded identities were read"
        );
    }

    // spec: TRD#uninstall-scope
    // spec: FLT#app-faults
    #[test]
    fn an_iteration_clears_teardown_identities_unreadable_once_it_reads_the_identities() {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        uninstalling(
            &oi,
            "app",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let mut reconciler = test_reconciler(&oi);
        rename_table(&oi, "resource_instances", "hidden_resource_instances");
        oi.block_on(reconciler.tick());
        assert_eq!(identities_unreadable(&oi, "app"), (1, 1));

        rename_table(&oi, "hidden_resource_instances", "resource_instances");
        oi.block_on(reconciler.tick());
        assert_eq!(identities_unreadable(&oi, "app"), (1, 0));
    }

    // spec: TRD#uninstall-scope
    // spec: FLT#app-faults
    #[test]
    fn teardown_identities_unreadable_is_left_by_an_iteration_that_does_not_read_the_apps_identities()
     {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        uninstalling(
            &oi,
            "app",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let other = uninstalling(
            &oi,
            "other",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let mut reconciler = test_reconciler(&oi);
        rename_table(&oi, "resource_instances", "hidden_resource_instances");
        teardown_iteration(&oi, &mut reconciler, &HashMap::new());
        assert_eq!(identities_unreadable(&oi, "app"), (1, 1));
        assert_eq!(identities_unreadable(&oi, "other"), (1, 1));

        rename_table(&oi, "hidden_resource_instances", "resource_instances");
        // `app` is held back from the iteration before its desired state is
        // computed, so its recorded identities are not read.
        oi.state.db.call(|db| {
            db.conn
                .execute(
                    "INSERT INTO app_priorities (app, priority, updated_at) VALUES ('app', 'urgent', '')",
                    [],
                )
                .unwrap();
        });
        let running = HashMap::from([(
            AppName::new("other").unwrap(),
            vec![still_running(&other[0])],
        )]);
        teardown_iteration(&oi, &mut reconciler, &running);

        assert_eq!(identities_unreadable(&oi, "other"), (1, 0));
        assert_eq!(
            identities_unreadable(&oi, "app"),
            (1, 1),
            "another app's read says nothing of this one's"
        );
    }

    // spec: TRD#uninstall-scope
    // spec: FLT#filing-and-clearing
    // The snapshot reads the recorded instances, and the uninstall phase then
    // fails to read their lifecycles. Clearing on the first read and filing on
    // the second filed the fault afresh on every iteration.
    #[test]
    fn a_later_failed_read_in_the_same_iteration_keeps_teardown_identities_unreadable_filed() {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        uninstalling(
            &oi,
            "app",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let mut reconciler = test_reconciler(&oi);
        rename_table(&oi, "world_observations", "hidden_world_observations");

        teardown_iteration(&oi, &mut reconciler, &HashMap::new());
        teardown_iteration(&oi, &mut reconciler, &HashMap::new());

        assert_eq!(phase_of(&oi, "app"), AppPhase::Uninstalling);
        assert_eq!(
            identities_unreadable(&oi, "app"),
            (1, 1),
            "one fault, held across the iterations"
        );

        rename_table(&oi, "hidden_world_observations", "world_observations");
        teardown_iteration(&oi, &mut reconciler, &HashMap::new());
        assert_eq!(identities_unreadable(&oi, "app").1, 0);
    }

    // spec: TRD#teardown
    #[test]
    fn teardown_completes_only_once_every_recorded_instance_is_unscheduled() {
        use crate::defs::resource::ResourceKind::Deployment;

        let oi = crate::oi::test_support::TestOi::new();
        let recorded = uninstalling(
            &oi,
            "app",
            crate::oi::test_support::MINIMAL_SCRIPT,
            &[(Deployment, "web")],
        );
        let web = recorded[0].clone();
        let observe = |kind: &'static str| {
            let web = web.clone();
            oi.state.db.call(move |db| {
                crate::runtime::history::insert_observation(db, &web, kind, &serde_json::json!({}))
                    .unwrap();
            });
        };
        observe("container_running");
        let mut reconciler = test_reconciler(&oi);

        uninstall_phase(&oi, &mut reconciler);
        assert_eq!(
            phase_of(&oi, "app"),
            AppPhase::Uninstalling,
            "no unit is loaded, but the instance is still running as far as is recorded"
        );

        observe("stop_sent");
        observe("container_exited");
        observe("container_removed");
        uninstall_phase(&oi, &mut reconciler);
        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        let app = AppName::new("app").unwrap();
        let remaining = oi
            .state
            .db
            .call(move |db| crate::runtime::history::instances_for_app(db, &app))
            .unwrap();
        assert!(remaining.is_empty(), "the recorded instances are forgotten");
    }

    // spec: TRD#teardown
    #[test]
    fn an_uninstall_brings_volumes_services_and_ingresses_to_unscheduled() {
        use crate::defs::resource::ResourceKind::{Ingress, Service, Volume};

        let oi = crate::oi::test_support::TestOi::new();
        let recorded = uninstalling(
            &oi,
            "app",
            r#"
            let svc = app.service("web").http(80);
            svc.ingress("clinic.example.com", 443).tls(Terminate.Https, Output.Http1);
            app.volume("data");
            "#,
            &[
                (Service, "web"),
                (Ingress, "clinic.example.com:443"),
                (Volume, "data"),
            ],
        );
        let mut reconciler = test_reconciler(&oi);

        uninstall_phase(&oi, &mut reconciler);

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        let remaining = oi
            .state
            .db
            .call(move |db| {
                crate::runtime::history::instances_for_app(db, &AppName::new("app").unwrap())
            })
            .unwrap();
        assert!(remaining.is_empty(), "{recorded:?}");
    }

    /// Run the phases that tear an uninstalling app down, the pods phase and
    /// the uninstall phase, until `app` is `NotInstalled` or well past what a
    /// teardown takes.
    fn tear_down(oi: &crate::oi::test_support::TestOi, reconciler: &mut Reconciler, app: &str) {
        for _ in 0..5 {
            if phase_of(oi, app) == AppPhase::NotInstalled {
                return;
            }
            let (apps, _) = reconciler.snapshot_all_apps();
            let pod_updates = oi.block_on(phases::run_pods_phase(
                &reconciler.observer,
                &reconciler.actuator,
                &reconciler.driver,
                &reconciler.db,
                &apps,
                &reconciler.written_obs,
                &reconciler.started_jobs,
                &reconciler.completed_jobs,
                &reconciler.crash_looped,
            ));
            let (running, _) = reconciler.ingest_pod_results(&apps, pod_updates);
            oi.block_on(reconciler.run_uninstall_phase(&apps, &running));
        }
    }

    fn network_exists(oi: &crate::oi::test_support::TestOi, network: &str) -> bool {
        oi.block_on(oi.state.driver.container.network_exists(network))
            .unwrap()
    }

    /// A harness over the stub system whose container runtime is returned
    /// beside it, so a test can stand containers up on it directly.
    fn with_stub_containers() -> (
        crate::oi::test_support::TestOi,
        Arc<crate::system::stub::StubContainerRuntime>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let container = Arc::new(crate::system::stub::StubContainerRuntime::new(
            dir.path().join("vols"),
        ));
        let process = Arc::new(crate::system::stub::StubProcessManager::new(Arc::clone(
            &container,
        )));
        let backends = crate::system::backends::Backends::working(
            Arc::clone(&container) as Arc<dyn crate::system::ContainerRuntime>,
            process,
        );
        let system = Arc::new(crate::system::System::for_tests(
            backends,
            Arc::new(crate::system::stub::StubNetworkProxy),
            dir.path(),
        ));
        (
            crate::oi::test_support::TestOi::with_driver(system),
            container,
            dir,
        )
    }

    /// Stand a pod instance up on the host, its container running on its pod
    /// network, returning the network's name.
    ///
    /// The stub supervisor keeps a unit loaded once it is stopped, where
    /// systemd unloads a stopped transient unit, so the unit is left out.
    fn run_pod(
        oi: &crate::oi::test_support::TestOi,
        containers: &crate::system::stub::StubContainerRuntime,
        instance: &crate::runtime::identity::ResourceInstance,
    ) -> String {
        let network = format!("seedling-{}", instance.display_name);
        containers.seed_container(&instance.display_name, &[]);
        oi.block_on(oi.state.driver.container.create_network(
            &network,
            "sdp-0001",
            "fd00:0:0:1::/64".parse().unwrap(),
            None,
        ))
        .unwrap();
        network
    }

    fn container_exists(oi: &crate::oi::test_support::TestOi, name: &str) -> bool {
        oi.block_on(oi.state.driver.container.inspect(name))
            .unwrap()
            .is_some()
    }

    fn record(
        oi: &crate::oi::test_support::TestOi,
        app: &str,
        kind: crate::defs::resource::ResourceKind,
        name: Option<&str>,
    ) -> crate::runtime::identity::ResourceInstance {
        let app = AppName::new(app).unwrap();
        let name = name.map(str::to_owned);
        oi.state.db.call(move |db| {
            crate::runtime::history::get_or_create_singleton(db, &app, kind, name.as_deref())
                .unwrap()
        })
    }

    // spec: TRD#teardown
    // spec: RCN#desired-state
    // An update that drops a resource leaves its running instances as they
    // are, and leaves the uninstall with a definition that no longer declares
    // them. Only the declared resources' instances were in the uninstalling
    // desired state, so these were never stopped and the uninstall never
    // completed.
    #[test]
    fn an_uninstall_stops_the_instances_of_resources_an_update_dropped() {
        use crate::defs::resource::ResourceKind::{Deployment, Volume};

        let (oi, containers, _dir) = with_stub_containers();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "app", "script": r#"
                app.deployment("web").image("docker.io/library/nginx:1.29");
                app.deployment("api").image("docker.io/library/nginx:1.29");
                app.volume("data");
            "# }),
        )
        .unwrap();
        oi.install("app");
        let web = record(&oi, "app", Deployment, Some("web"));
        let data = record(&oi, "app", Volume, Some("data"));
        let network = run_pod(&oi, &containers, &web);
        let storage = crate::runtime::identity::VolumeName::of_instance(&data);
        oi.block_on(oi.state.driver.volume_store.create(&storage))
            .unwrap();

        oi.call(
            "/apps/update",
            serde_json::json!({ "app": "app", "script": r#"
                app.deployment("api").image("docker.io/library/nginx:1.29");
            "# }),
        )
        .unwrap();
        oi.call("/apps/uninstall", serde_json::json!({ "app": "app" }))
            .unwrap();
        let mut reconciler = test_reconciler(&oi);
        tear_down(&oi, &mut reconciler, "app");

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        assert!(!container_exists(&oi, &web.display_name));
        assert!(!network_exists(&oi, &network));
        assert!(
            !oi.state.driver.volume_store.exists(&storage),
            "the dropped volume's data is held, as a declared one's is"
        );
    }

    // spec: TRD#teardown
    // spec: RCN#desired-state
    // An instance an operation created is anonymous, so no definition ever
    // declares it. One left standing when its operation ended is stopped by
    // the uninstall from its recorded identity.
    #[test]
    fn an_uninstall_stops_an_instance_an_operation_created() {
        use crate::defs::resource::ResourceKind::Deployment;

        let (oi, containers, _dir) = with_stub_containers();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "app", "script": crate::oi::test_support::MINIMAL_SCRIPT }),
        )
        .unwrap();
        oi.install("app");
        let anonymous = record(&oi, "app", Deployment, None);
        let network = run_pod(&oi, &containers, &anonymous);

        oi.call("/apps/uninstall", serde_json::json!({ "app": "app" }))
            .unwrap();
        let mut reconciler = test_reconciler(&oi);
        tear_down(&oi, &mut reconciler, "app");

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        assert!(!container_exists(&oi, &anonymous.display_name));
        assert!(!network_exists(&oi, &network));
    }

    // spec: ACTU#deployment-and-job-instances
    // spec: TRD#teardown
    // spec: FLT#workload-faults
    // A pod whose container was removed on exit is Unscheduled with its
    // network gone, so nothing stopped it again; teardown completed and the
    // anonymous volume's record, the only trace of its storage, went with the
    // recorded instances.
    #[test]
    fn an_uninstall_waits_for_an_anonymous_volume_whose_removal_fails() {
        use crate::defs::resource::ResourceKind::Job;

        let (oi, containers, _dir) = with_stub_containers();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "app", "script": crate::oi::test_support::MINIMAL_SCRIPT }),
        )
        .unwrap();
        oi.install("app");
        let job = record(&oi, "app", Job, None);
        let volume = "seedling-anon-0123456789abcdef0123456789abcdef-vol-0";
        {
            let job = job.clone();
            let recorded = crate::runtime::anonymous_volumes::AnonymousVolume {
                name: volume.to_owned(),
                tmpfs: false,
            };
            oi.state.db.call(move |db| {
                crate::runtime::anonymous_volumes::record(db, &job, &[recorded]).unwrap();
                for kind in ["container_running", "container_exited", "container_removed"] {
                    crate::runtime::history::insert_observation(
                        db,
                        &job,
                        kind,
                        &serde_json::json!({}),
                    )
                    .unwrap();
                }
            });
        }
        oi.block_on(oi.state.driver.container.create_volume(volume, false))
            .unwrap();
        containers.refuse_volume_removal(true);

        oi.call("/apps/uninstall", serde_json::json!({ "app": "app" }))
            .unwrap();
        let mut reconciler = test_reconciler(&oi);
        tear_down(&oi, &mut reconciler, "app");

        assert_eq!(phase_of(&oi, "app"), AppPhase::Uninstalling);
        assert_eq!(anonymous_volumes_recorded(&oi, &job), vec![volume]);
        assert!(volume_exists(&oi, volume));
        assert_eq!(stop_failed_faults(&oi, &job), 1);

        containers.refuse_volume_removal(false);
        tear_down(&oi, &mut reconciler, "app");

        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        assert!(!volume_exists(&oi, volume));
        assert!(anonymous_volumes_recorded(&oi, &job).is_empty());
        assert_eq!(stop_failed_faults(&oi, &job), 0);
    }

    // spec: RCN#gate-holds
    // spec: GRUN#gate-state
    // What the gates say is read afresh for each iteration: a hold on record
    // keeps its selection at `Unscheduled` whatever the scale, a hold that
    // cannot be read starts none of the selection, and a gate that is
    // satisfied no longer keeps its dependants waiting.
    #[test]
    fn each_iteration_reads_what_the_gates_hold_and_which_workloads_wait() {
        use crate::runtime::{gates::store, secrets::Cipher};
        use seedling_protocol::names::GateName;

        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": r#"
                    let api = app.deployment("api").image("docker.io/library/nginx:1").scale(1..=3);
                    let migrate = app.gate("migrate").key(1).exclusive(api);
                    app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
                "#,
            }),
        )
        .unwrap();
        oi.install("app");
        let (app, gate) = (
            AppName::new("app").unwrap(),
            GateName::new("migrate").unwrap(),
        );
        let reconciler = test_reconciler(&oi);
        let snapshot = || {
            let (apps, _) = reconciler.snapshot_all_apps();
            let snapshot = apps.into_iter().find(|a| a.name.as_str() == "app").unwrap();
            let desired = |name: &str| -> Vec<LifecycleState> {
                snapshot
                    .desired
                    .resources
                    .iter()
                    .filter(|r| r.instance.name.as_deref() == Some(name))
                    .map(|r| r.desired)
                    .collect()
            };
            let (api, web) = (desired("api"), desired("web"));
            (api, web, snapshot.gates)
        };
        let waits =
            |gates: &GateView, name: &str| gates.waiting.keys().any(|id| id.name.as_str() == name);
        let api_id = crate::defs::resource::ResourceId {
            kind: crate::defs::resource::ResourceKind::Deployment,
            name: Arc::new("api".to_owned()),
        };

        let (api, _, gates) = snapshot();
        assert!(gates.held.is_empty(), "nothing holds the selection yet");
        assert_eq!(api, vec![LifecycleState::Ready]);
        assert!(waits(&gates, "web"), "web waits for the unsatisfied gate");

        let (a, g, id) = (app.clone(), gate.clone(), api_id.clone());
        oi.state.db.call(move |db| {
            store::set_hold(db, &a, &g, &[id].into()).unwrap();
        });
        let (api, _, gates) = snapshot();
        assert_eq!(
            gates.held.keys().cloned().collect::<Vec<_>>(),
            vec![api_id.clone()]
        );
        assert!(
            !api.is_empty() && api.iter().all(|d| *d == LifecycleState::Unscheduled),
            "a held selection stays down: {api:?}"
        );

        oi.state.db.call(|db| {
            db.conn
                .execute("ALTER TABLE gate_holds RENAME TO gate_holds_away", [])
                .unwrap();
        });
        let (_, _, gates) = snapshot();
        assert!(
            gates.held.is_empty(),
            "nothing is stopped for what cannot be read"
        );
        assert!(
            waits(&gates, "api") && waits(&gates, "web"),
            "nothing of the selection starts while its hold cannot be read"
        );
        oi.state.db.call(|db| {
            db.conn
                .execute("ALTER TABLE gate_holds_away RENAME TO gate_holds", [])
                .unwrap();
        });

        let def = oi
            .state
            .registry
            .read()
            .get("app")
            .unwrap()
            .app
            .def
            .load_full();
        let digest = def.gates["migrate"]
            .key
            .digest(&Cipher::for_tests())
            .unwrap();
        let (a, g) = (app.clone(), gate.clone());
        oi.state.db.call(move |db| {
            store::record_success(db, &a, &g, &digest, 1).unwrap();
        });
        let (_, _, gates) = snapshot();
        assert!(
            !waits(&gates, "web"),
            "web no longer waits once the gate is satisfied"
        );
    }

    // spec: RCN#gate-holds
    // The reconciler acts on the gate state the iteration's drive worked out,
    // holds included as drive left them, without reading it again, and reads
    // it afresh where no drive has worked it out since.
    #[test]
    fn an_iteration_reads_gate_state_once_and_acts_on_what_drive_left() {
        use crate::runtime::{
            gates::{self, store},
            secrets::Cipher,
        };
        use seedling_protocol::names::GateName;

        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": r#"
                    let api = app.deployment("api").image("docker.io/library/nginx:1");
                    let migrate = app.gate("migrate").key(1).exclusive(api);
                    app.deployment("web").image("docker.io/library/nginx:1").after(migrate);
                "#,
            }),
        )
        .unwrap();
        oi.install("app");
        let (app, gate) = (
            AppName::new("app").unwrap(),
            GateName::new("migrate").unwrap(),
        );
        let api_id = crate::defs::resource::ResourceId {
            kind: crate::defs::resource::ResourceKind::Deployment,
            name: Arc::new("api".to_owned()),
        };
        let def = oi
            .state
            .registry
            .read()
            .get("app")
            .unwrap()
            .app
            .def
            .load_full();
        let digest = def.gates["migrate"]
            .key
            .digest(&Cipher::for_tests())
            .unwrap();
        let hold = || {
            let (a, g, id) = (app.clone(), gate.clone(), api_id.clone());
            oi.state.db.call(move |db| {
                store::set_hold(db, &a, &g, &[id].into()).unwrap();
            });
        };
        let (a, g) = (app.clone(), gate.clone());
        oi.state.db.call(move |db| {
            store::record_success(db, &a, &g, &digest, 1).unwrap();
        });
        hold();

        let reconciler = test_reconciler(&oi);
        let reads = || oi.state.db.call(|_| gates::reads());
        let view = || {
            let (apps, _) = reconciler.snapshot_all_apps();
            apps.into_iter()
                .find(|a| a.name.as_str() == "app")
                .unwrap()
                .gates
        };

        let before = reads();
        oi.block_on(gates::drive::drive(&oi.state.engine));
        let driven = view();
        let after = reads();
        assert_eq!(
            (after.0 - before.0, after.1 - before.1),
            (1, 1),
            "gate state and readiness are read once in the iteration"
        );
        assert!(
            driven.held.is_empty(),
            "the hold drive released for the satisfied gate is not acted on"
        );
        assert!(driven.waiting.is_empty(), "{:?}", driven.waiting);

        oi.block_on(gates::drive::drive(&oi.state.engine));
        hold();
        let before = reads();
        assert!(
            view().held.is_empty(),
            "the reconciler acts on the state drive worked out"
        );
        assert_eq!(reads(), before);

        assert_eq!(
            view().held.keys().cloned().collect::<Vec<_>>(),
            vec![api_id],
            "without a drive since, the gates are read afresh"
        );
        let after = reads();
        assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1));
    }

    // spec: GATE#body
    // spec: RCN#gate-holds
    #[test]
    fn a_gate_run_starts_what_its_body_started_without_waiting_for_the_gate() {
        use crate::runtime::gates::store;
        use seedling_protocol::names::GateName;

        const IMAGE: &str = "docker.io/library/nginx:1.29";
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": format!(r#"
                    let api = app.deployment("api").image("{IMAGE}");
                    let migrate = app.gate("migrate").key(1).exclusive(api);
                    api.after(migrate);
                    app.deployment("web").image("{IMAGE}").after(migrate);
                "#),
            }),
        )
        .unwrap();
        oi.install("app");
        let (app, gate) = (
            AppName::new("app").unwrap(),
            GateName::new("migrate").unwrap(),
        );
        let api_id = crate::defs::resource::ResourceId {
            kind: crate::defs::resource::ResourceKind::Deployment,
            name: Arc::new("api".to_owned()),
        };
        let reconciler = test_reconciler(&oi);
        let snapshot = || {
            let (apps, _) = reconciler.snapshot_all_apps();
            apps.into_iter().find(|a| a.name.as_str() == "app").unwrap()
        };
        let instance = |snapshot: &AppSnapshot, name: &str| {
            snapshot
                .desired
                .resources
                .iter()
                .find(|r| r.instance.name.as_deref() == Some(name))
                .map(|r| (r.instance.clone(), r.desired))
                .unwrap()
        };

        // The instances exist before the gate holds them down.
        let idle = snapshot();
        assert!(idle.gates.waits_for(&api_id.kind, Some("api")));
        let (a, g, id) = (app.clone(), gate.clone(), api_id.clone());
        oi.state.db.call(move |db| {
            store::set_hold(db, &a, &g, &[id].into()).unwrap();
        });
        let mut progress = crate::runtime::OperationProgress::new();
        progress.gate_run = Some(gate);
        progress.definition = Some(
            oi.state
                .registry
                .read()
                .get("app")
                .unwrap()
                .app
                .def
                .load_full(),
        );
        progress.started(instance(&idle, "api").0);
        *oi.state
            .registry
            .read()
            .get("app")
            .unwrap()
            .active_progress
            .write() = Some(progress);

        let running = snapshot();
        let (api, web) = (instance(&running, "api"), instance(&running, "web"));
        assert_eq!(
            api.1,
            LifecycleState::Ready,
            "the hold does not keep it down"
        );
        assert!(
            !running.gates.waits_for(&api_id.kind, Some("api")),
            "what the body started does not wait on the gate"
        );
        assert!(
            running.gates.waits_for(&api_id.kind, Some("web")),
            "what the body did not start still waits"
        );

        oi.block_on(oi.state.container_runtime.pull_image(IMAGE))
            .unwrap();
        let update = oi.block_on(pods::observe_and_actuate(
            &reconciler.observer,
            &reconciler.actuator,
            &reconciler.driver,
            &reconciler.db,
            &running.desired,
            running.app_priority,
            &running.gate_keys,
            &running.gates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        ));
        let started: Vec<_> = update.started_instances.iter().map(|i| i.id).collect();
        assert!(started.contains(&api.0.id), "{started:?}");
        assert!(!started.contains(&web.0.id), "{started:?}");
    }

    // spec: OPS#changes-to-an-app
    // spec: UPD#spec-hash
    #[test]
    fn an_update_that_drops_a_gate_mid_operation_does_not_fail_a_start_the_operation_placed() {
        const IMAGE: &str = "docker.io/library/nginx:1.29";
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": format!(r#"
                    let migrate = app.gate("migrate").key(1);
                    app.deployment("api").image("{IMAGE}").after(migrate);
                "#),
            }),
        )
        .unwrap();
        oi.install("app");
        let reconciler = test_reconciler(&oi);
        let snapshot = || {
            let (apps, _) = reconciler.snapshot_all_apps();
            apps.into_iter().find(|a| a.name.as_str() == "app").unwrap()
        };
        let api = snapshot()
            .desired
            .resources
            .iter()
            .find(|r| r.instance.name.as_deref() == Some("api"))
            .map(|r| r.instance.clone())
            .unwrap();

        let entry_app = oi.state.registry.read().get("app").unwrap().app.clone();
        let mut progress = crate::runtime::OperationProgress::new();
        progress.definition = Some(entry_app.def.load_full());
        progress.started(api.clone());
        *oi.state
            .registry
            .read()
            .get("app")
            .unwrap()
            .active_progress
            .write() = Some(progress);

        let (updated, err) = crate::runtime::apps::evaluate_script(
            &AppName::new("app").unwrap(),
            &format!(r#"app.deployment("api").image("{IMAGE}");"#),
            &BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "{err:?}");
        entry_app.def.store(updated.def.load_full());

        let running = snapshot();
        assert!(running.app_def.gates.is_empty());
        oi.block_on(oi.state.container_runtime.pull_image(IMAGE))
            .unwrap();
        let update = oi.block_on(pods::observe_and_actuate(
            &reconciler.observer,
            &reconciler.actuator,
            &reconciler.driver,
            &reconciler.db,
            &running.desired,
            running.app_priority,
            &running.gate_keys,
            &running.gates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        ));
        assert!(
            update.start_failures.is_empty(),
            "{:?}",
            update.start_failures
        );
        assert!(
            update.observe_failures.is_empty(),
            "{:?}",
            update.observe_failures
        );
        let started: Vec<_> = update.started_instances.iter().map(|i| i.id).collect();
        assert_eq!(started, vec![api.id]);
    }

    // spec: OPS#changes-to-an-app
    // spec: RCN#desired-state
    #[test]
    fn an_ingress_an_operation_placed_is_routed_as_the_operation_declares_it_after_an_update_drops_it()
     {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": r#"app.service("web").ingress("a.example.com", 2222);"#,
            }),
        )
        .unwrap();
        oi.install("app");
        let reconciler = test_reconciler(&oi);
        let snapshot = || {
            let (apps, _) = reconciler.snapshot_all_apps();
            apps.into_iter().find(|a| a.name.as_str() == "app").unwrap()
        };
        let ingress = snapshot()
            .desired
            .resources
            .iter()
            .find(|r| r.instance.kind == ResourceKind::Ingress)
            .map(|r| r.instance.clone())
            .unwrap();

        let entry_app = oi.state.registry.read().get("app").unwrap().app.clone();
        let mut progress = crate::runtime::OperationProgress::new();
        progress.definition = Some(entry_app.def.load_full());
        progress.started(ingress);
        *oi.state
            .registry
            .read()
            .get("app")
            .unwrap()
            .active_progress
            .write() = Some(progress);

        let (updated, err) = crate::runtime::apps::evaluate_script(
            &AppName::new("app").unwrap(),
            r#"app.service("web");"#,
            &BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "{err:?}");
        entry_app.def.store(updated.def.load_full());

        let running = snapshot();
        let routed = |def: &AppDef| {
            proxy::collect(
                def,
                &running.desired,
                &reconciler.node_prefix,
                &*reconciler.registry,
                &running.name,
                &[],
            )
            .unwrap()
            .ingresses
            .into_iter()
            .map(|(def, _)| def.hostname)
            .collect::<Vec<_>>()
        };
        assert!(routed(&running.app_def).is_empty());
        assert_eq!(routed(&running.declarations), ["a.example.com"]);
    }

    // spec: GRUN#uninstall
    // The records of the app's gates, their holds and the record that it is
    // set up go with the uninstall, so that installing again runs every gate
    // as for the first time. While they cannot be deleted the app stays
    // `Uninstalling`.
    #[test]
    fn an_uninstall_deletes_the_state_of_the_gates_and_waits_while_it_cannot() {
        use seedling_protocol::names::GateName;

        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "app",
                "script": r#"
                    let api = app.deployment("api").image("docker.io/library/nginx:1");
                    app.gate("migrate").key(1).exclusive(api);
                "#,
            }),
        )
        .unwrap();
        oi.install("app");
        let app = AppName::new("app").unwrap();
        let gate = GateName::new("migrate").unwrap();
        {
            let (app, gate) = (app.clone(), gate.clone());
            oi.state.db.call(move |db| {
                crate::runtime::gates::store::begin_run(db, &app, &gate).unwrap();
                crate::runtime::gates::store::set_hold(
                    db,
                    &app,
                    &gate,
                    &[crate::defs::resource::ResourceId {
                        kind: crate::defs::resource::ResourceKind::Deployment,
                        name: Arc::new("api".to_owned()),
                    }]
                    .into(),
                )
                .unwrap();
                crate::runtime::gates::store::mark_set_up(db, &app).unwrap();
                db.conn
                    .execute_batch(
                        "CREATE TRIGGER keep_holds BEFORE DELETE ON gate_holds
                         BEGIN SELECT RAISE(ABORT, 'locked'); END;",
                    )
                    .unwrap();
            });
        }
        let state = |oi: &crate::oi::test_support::TestOi| {
            let app = app.clone();
            oi.state.db.call(move |db| {
                (
                    crate::runtime::gates::store::load_records(db, &app)
                        .unwrap()
                        .len(),
                    crate::runtime::gates::store::load_holds(db, &app)
                        .unwrap()
                        .len(),
                    crate::runtime::gates::store::is_set_up(db, &app).unwrap(),
                )
            })
        };
        assert_eq!(state(&oi), (1, 1, true));

        oi.call("/apps/uninstall", serde_json::json!({ "app": "app" }))
            .unwrap();
        let mut reconciler = test_reconciler(&oi);
        tear_down(&oi, &mut reconciler, "app");
        assert_eq!(
            phase_of(&oi, "app"),
            AppPhase::Uninstalling,
            "the app is not moved to NotInstalled while the gate state remains"
        );
        assert_eq!(
            state(&oi),
            (1, 1, true),
            "a deletion that fails leaves all of it, not part"
        );

        oi.state.db.call(|db| {
            db.conn.execute_batch("DROP TRIGGER keep_holds;").unwrap();
        });
        tear_down(&oi, &mut reconciler, "app");
        assert_eq!(phase_of(&oi, "app"), AppPhase::NotInstalled);
        assert_eq!(state(&oi), (0, 0, false));
    }

    /// How many `stop_failed` faults are active for `instance`.
    fn stop_failed_faults(
        oi: &crate::oi::test_support::TestOi,
        instance: &crate::runtime::identity::ResourceInstance,
    ) -> usize {
        let hex = instance.id.to_hex();
        oi.state.db.call(move |db| {
            crate::runtime::faults::list_active_faults(db, None)
                .unwrap()
                .into_iter()
                .filter(|f| f.kind == "stop_failed" && f.instance_id.as_deref() == Some(&hex))
                .count()
        })
    }

    /// An anonymous Job mounting an anonymous volume, as an action closure
    /// creates one, and the volume's name.
    fn anonymous_job_with_anonymous_volume() -> (Resource, String) {
        let progress = Arc::new(parking_lot::RwLock::new(None));
        let result = crate::tests::run_action_with(
            r#"
            app.on_action("work", |rt, _param| {
                let scratch = app.volume();
                rt.start(app.job().image("docker.io/library/busybox:1.37").mount("/scratch", scratch));
            });
            "#,
            &[],
            "work",
            None,
            crate::tests::ActionHooks {
                active_progress: Some(Arc::clone(&progress)),
                ..Default::default()
            },
        );
        assert!(
            matches!(result, crate::runtime::OperationResult::Completed),
            "{result:?}"
        );
        let job = progress
            .read()
            .as_ref()
            .and_then(|p| p.dynamic_defs.values().next().cloned())
            .expect("the closure started a job");
        let volume = {
            let Resource::Job(def) = &job else {
                panic!("{job:?}")
            };
            let def = def.def.lock();
            let pod = def.pod.lock();
            let container = pod.container.lock();
            container
                .volume_mounts
                .values()
                .find_map(|mount| match mount {
                    crate::defs::container::VolumeMount::Volume(v) => v.anon_id.clone(),
                    _ => None,
                })
                .expect("the job mounts an anonymous volume")
        };
        (job, volume)
    }

    /// Hold `instance` in `app`'s operation progress with `definition`, at
    /// `Ready` or, where `stopped`, at `Unscheduled`.
    fn hold_in_operation(
        oi: &crate::oi::test_support::TestOi,
        app: &str,
        instance: &crate::runtime::identity::ResourceInstance,
        definition: Resource,
        stopped: bool,
    ) {
        let mut progress = crate::runtime::OperationProgress::new();
        if stopped {
            progress.stopped(instance.clone());
            progress.dynamic_defs.insert(instance.clone(), definition);
        } else {
            progress.started_dynamic(instance.clone(), definition);
        }
        let registry = oi.state.registry.read();
        *registry.get(app).unwrap().active_progress.write() = Some(progress);
    }

    /// Place `instance` in the slot of `session`, a shell session open on
    /// `app`, with `definition`, at `Ready` or, where `stopped`, at
    /// `Unscheduled`.
    fn hold_in_session(
        oi: &crate::oi::test_support::TestOi,
        app: &str,
        session: seedling_protocol::names::SessionId,
        instance: &crate::runtime::identity::ResourceInstance,
        definition: Resource,
        stopped: bool,
    ) {
        let mut progress = crate::runtime::OperationProgress::new();
        if stopped {
            progress.stopped(instance.clone());
            progress.dynamic_defs.insert(instance.clone(), definition);
        } else {
            progress.started_dynamic(instance.clone(), definition);
        }
        let sessions = Arc::clone(&oi.state.registry.read().get(app).unwrap().sessions);
        *sessions.open(session).write() = Some(progress);
    }

    /// The instances `app`'s desired state holds, and whether its snapshot
    /// is steady.
    fn desired_instances(
        reconciler: &Reconciler,
        app: &str,
    ) -> (
        std::collections::HashSet<crate::runtime::identity::InstanceId>,
        bool,
    ) {
        let (apps, _) = reconciler.snapshot_all_apps();
        let snapshot = apps
            .iter()
            .find(|snapshot| snapshot.name.as_str() == app)
            .expect("the app is snapshotted");
        (
            snapshot
                .desired
                .resources
                .iter()
                .map(|resource| resource.instance.id)
                .collect(),
            snapshot.steady,
        )
    }

    // spec: RCN#desired-state
    // The reconciler read one progress per app, the operation's, so what a
    // shell's closure started was in no desired state and was never actuated.
    #[test]
    fn what_a_shell_session_started_is_desired_beside_what_the_operation_started() {
        use seedling_protocol::names::SessionId;

        let oi = crate::oi::test_support::TestOi::with_app("app");
        oi.install("app");
        let reconciler = test_reconciler(&oi);
        let (definition, _) = anonymous_job_with_anonymous_volume();
        let anonymous = || {
            crate::runtime::identity::ResourceInstance::new_anonymous(
                AppName::new("app").unwrap(),
                crate::defs::resource::ResourceKind::Job,
            )
        };
        let (operation_job, first_job, second_job) = (anonymous(), anonymous(), anonymous());
        let (first, second) = (SessionId::generate(), SessionId::generate());

        let (steady, _) = desired_instances(&reconciler, "app");
        hold_in_session(&oi, "app", first, &first_job, definition.clone(), false);
        let sessions = Arc::clone(&oi.state.registry.read().get("app").unwrap().sessions);
        let empty = SessionId::generate();
        sessions.open(empty);
        let (_, steady_with_empty_slot) = desired_instances(&reconciler, "app");
        assert!(
            !steady_with_empty_slot,
            "a session that has placed nothing yet still breaks steadiness"
        );
        sessions.close(&empty);
        let (with_session, still_steady) = desired_instances(&reconciler, "app");
        assert!(
            with_session.contains(&first_job.id),
            "a session's resource is desired with no operation running"
        );
        assert_eq!(with_session.len(), steady.len() + 1);
        assert!(!still_steady, "an app with a session is not steady");

        hold_in_operation(&oi, "app", &operation_job, definition.clone(), false);
        hold_in_session(&oi, "app", second, &second_job, definition, false);
        let (all, _) = desired_instances(&reconciler, "app");
        assert_eq!(
            all,
            [operation_job.id, first_job.id, second_job.id].into(),
            "the operation's and each session's resources are all desired"
        );

        sessions.close(&first);
        let (all, _) = desired_instances(&reconciler, "app");
        assert_eq!(
            all,
            [operation_job.id, second_job.id].into(),
            "closing one session drops its resources alone"
        );
    }

    // spec: RCN#desired-state
    // spec: OPS#shell-sessions
    // What a shell's closure started was never actuated: nothing read its slot.
    #[test]
    fn the_job_a_shell_session_placed_in_its_slot_is_started() {
        let (oi, _containers, _dir) = with_stub_containers();
        let mut reconciler = test_reconciler(&oi);
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "app", "script": crate::oi::test_support::MINIMAL_SCRIPT }),
        )
        .unwrap();
        oi.install("app");
        let job = crate::runtime::identity::ResourceInstance::new_anonymous(
            AppName::new("app").unwrap(),
            crate::defs::resource::ResourceKind::Job,
        );
        let (definition, _) = anonymous_job_with_anonymous_volume();
        oi.block_on(
            oi.state
                .driver
                .container
                .pull_image("docker.io/library/busybox:1.37"),
        )
        .unwrap();
        hold_in_session(
            &oi,
            "app",
            seedling_protocol::names::SessionId::generate(),
            &job,
            definition,
            false,
        );
        run_pods(&oi, &mut reconciler, 1);

        assert!(
            oi.block_on(
                oi.state
                    .driver
                    .process
                    .unit_state(&crate::system::actuator::unit_name(&job))
            )
            .unwrap()
            .is_some(),
            "the session's job was started"
        );
    }

    // spec: RCN#desired-state
    // spec: OPS#shell-sessions
    // A session's resources are desired while the app is installed, and held
    // stopped while it uninstalls and once it is no longer installed: the
    // reconciler skipped an app that was not installed, so the teardown of a
    // session that outlasted its app's uninstall was never actuated.
    #[test]
    fn a_session_resource_is_desired_whatever_the_app_phase() {
        let oi = crate::oi::test_support::TestOi::with_app("app");
        oi.install("app");
        let reconciler = test_reconciler(&oi);
        let (definition, _) = anonymous_job_with_anonymous_volume();
        let job = crate::runtime::identity::ResourceInstance::new_anonymous(
            AppName::new("app").unwrap(),
            crate::defs::resource::ResourceKind::Job,
        );
        hold_in_session(
            &oi,
            "app",
            seedling_protocol::names::SessionId::generate(),
            &job,
            definition,
            false,
        );

        for phase in [
            AppPhase::Installed,
            AppPhase::Uninstalling,
            AppPhase::NotInstalled,
        ] {
            *oi.state.registry.read().get("app").unwrap().phase.lock() = phase.clone();
            let (apps, _) = reconciler.snapshot_all_apps();
            let desired = &apps
                .iter()
                .find(|snapshot| snapshot.name.as_str() == "app")
                .unwrap()
                .desired;
            let held = desired
                .resources
                .iter()
                .find(|resource| resource.instance.id == job.id)
                .unwrap_or_else(|| panic!("the session's job is desired while {phase:?}"));
            if phase != AppPhase::Installed {
                assert_eq!(
                    held.desired,
                    LifecycleState::Unscheduled,
                    "an app going or gone holds it stopped"
                );
            }
        }
    }

    /// Run the pods phase `ticks` times.
    fn run_pods(oi: &crate::oi::test_support::TestOi, reconciler: &mut Reconciler, ticks: usize) {
        for _ in 0..ticks {
            let (apps, _) = reconciler.snapshot_all_apps();
            let pod_updates = oi.block_on(phases::run_pods_phase(
                &reconciler.observer,
                &reconciler.actuator,
                &reconciler.driver,
                &reconciler.db,
                &apps,
                &reconciler.written_obs,
                &reconciler.started_jobs,
                &reconciler.completed_jobs,
                &reconciler.crash_looped,
            ));
            reconciler.ingest_pod_results(&apps, pod_updates);
        }
    }

    fn volume_exists(oi: &crate::oi::test_support::TestOi, volume: &str) -> bool {
        oi.block_on(oi.state.driver.container.volume_exists(volume))
            .unwrap()
    }

    fn anonymous_volumes_recorded(
        oi: &crate::oi::test_support::TestOi,
        instance: &crate::runtime::identity::ResourceInstance,
    ) -> Vec<String> {
        let id = instance.id;
        oi.state.db.call(move |db| {
            crate::runtime::anonymous_volumes::recorded_for(db, id)
                .unwrap()
                .into_iter()
                .map(|volume| volume.name)
                .collect()
        })
    }

    /// Install `app`, and run an anonymous Job mounting an anonymous volume as
    /// an operation of it does, returning the Job's instance and the volume.
    fn run_anonymous_job_with_anonymous_volume(
        oi: &crate::oi::test_support::TestOi,
        reconciler: &mut Reconciler,
        app: &str,
    ) -> (crate::runtime::identity::ResourceInstance, String) {
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": app, "script": crate::oi::test_support::MINIMAL_SCRIPT }),
        )
        .unwrap();
        oi.install(app);
        let job = record(oi, app, crate::defs::resource::ResourceKind::Job, None);
        let (definition, volume) = anonymous_job_with_anonymous_volume();
        oi.block_on(
            oi.state
                .driver
                .container
                .pull_image("docker.io/library/busybox:1.37"),
        )
        .unwrap();
        hold_in_operation(oi, app, &job, definition, false);
        run_pods(oi, reconciler, 1);
        assert!(
            oi.block_on(
                oi.state
                    .driver
                    .process
                    .unit_state(&crate::system::actuator::unit_name(&job))
            )
            .unwrap()
            .is_some(),
            "the job was started"
        );
        assert!(volume_exists(oi, &volume), "with its anonymous volume");
        assert_eq!(anonymous_volumes_recorded(oi, &job), vec![volume.clone()]);
        (job, volume)
    }

    // spec: ACTU#deployment-and-job-instances
    // At the end of an operation its dynamic pods are stopped through a stand-in
    // carrying no configuration, so the stop could not see the anonymous
    // volumes the pod's definition mounted, and left their storage behind.
    #[test]
    fn an_operation_created_pods_anonymous_volume_is_removed_when_the_operation_ends() {
        let (oi, _containers, _dir) = with_stub_containers();
        let mut reconciler = test_reconciler(&oi);
        let (job, volume) = run_anonymous_job_with_anonymous_volume(&oi, &mut reconciler, "app");

        // As the end-of-operation cleanup holds an operation's dynamic pods.
        let stand_in = crate::runtime::desired::undeclared_pod(&job).unwrap();
        hold_in_operation(&oi, "app", &job, stand_in, true);
        run_pods(&oi, &mut reconciler, 3);

        assert!(!network_exists(
            &oi,
            &format!("seedling-{}", job.display_name)
        ));
        assert!(!volume_exists(&oi, &volume), "the volume went with its pod");
        assert!(anonymous_volumes_recorded(&oi, &job).is_empty());
    }

    // spec: ACTU#deployment-and-job-instances
    // spec: TRD#teardown
    // An uninstall stops a pod an operation left standing from its recorded
    // identity, which carried nothing of the anonymous volumes created for it.
    #[test]
    fn an_uninstall_removes_the_anonymous_volume_of_a_pod_an_operation_created() {
        let (oi, _containers, _dir) = with_stub_containers();
        let mut reconciler = test_reconciler(&oi);
        let (job, volume) = run_anonymous_job_with_anonymous_volume(&oi, &mut reconciler, "app");

        // The operation ended without its cleanup stopping the job.
        *oi.state
            .registry
            .read()
            .get("app")
            .unwrap()
            .active_progress
            .write() = None;
        oi.call("/apps/uninstall", serde_json::json!({ "app": "app" }))
            .unwrap();
        // The stub supervisor keeps a stopped unit loaded, where systemd
        // unloads a stopped transient unit, so the uninstall itself does not
        // complete here; the pod's teardown is what is under test.
        tear_down(&oi, &mut reconciler, "app");

        assert!(!network_exists(
            &oi,
            &format!("seedling-{}", job.display_name)
        ));
        assert!(!volume_exists(&oi, &volume), "the volume went with its pod");
        assert!(anonymous_volumes_recorded(&oi, &job).is_empty());
    }

    // spec: PRIO#unreadable-priorities
    #[test]
    fn an_app_whose_stored_priority_cannot_be_read_is_held_back_with_a_fault() {
        let oi = crate::oi::test_support::TestOi::with_app("fine");
        oi.install("fine");
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "odd", "script": crate::oi::test_support::MINIMAL_SCRIPT }),
        )
        .unwrap();
        oi.install("odd");
        oi.state.db.call(|db| {
            db.conn
                .execute(
                    "INSERT INTO app_priorities (app, priority, updated_at) VALUES ('odd', 'urgent', '')",
                    [],
                )
                .unwrap();
        });
        let reconciler = test_reconciler(&oi);

        let (snapshots, dropped) = reconciler.snapshot_all_apps();

        let names: Vec<_> = snapshots.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["fine"], "the other apps are still actuated");
        assert_eq!(dropped, 1, "the held-back app is not accounted for");
        let faults = oi
            .state
            .db
            .call(|db| crate::runtime::faults::list_active_faults(db, None).unwrap());
        let fault = faults
            .iter()
            .find(|f| f.kind == "app_priorities_unreadable")
            .expect("filed");
        assert!(fault.description.contains("odd"), "{}", fault.description);
    }

    // spec: FLT#certificate-faults
    // spec: TLS#certificate-status
    // The deadline is for an acquisition that is not happening. Only an open
    // Canopy order used to count as one, so a hostname the proxy was still
    // ordering for was faulted at the deadline as though nothing were.
    #[test]
    fn cert_acquisition_failed_waits_for_any_acquisition_in_progress() {
        const SCRIPT: &str = r#"
            let web = app.service("web").exported().http(80);
            web.ingress("shop.example.com", 443).tls(Terminate.Https, Output.Http1);
            web.ingress("canopy.example.com", 443).tls(Terminate.Https, Output.Http1);
            app.deployment("web")
                .image("docker.io/library/nginx:1.29")
                .http(80, web.route("/"));
        "#;
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "shop", "script": SCRIPT }),
        )
        .unwrap();
        oi.install("shop");
        // A Canopy-only hostname no typed app declares: Canopy cannot be asked,
        // so nothing is acquiring its certificate.
        oi.call(
            "/tls/policies/set-canopy",
            serde_json::json!({ "hostname": "canopy.example.com" }),
        )
        .unwrap();

        let mut reconciler = test_reconciler(&oi);
        reconciler.tls_coordinator = Some(Arc::clone(&oi.state.tls_coordinator));
        let mut snapshot = snapshot_with_priorities("shop", AppPriority::Normal, SCRIPT);
        snapshot.warm_cert_hostnames = ["shop.example.com:443", "canopy.example.com:443"]
            .map(str::to_owned)
            .into();
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(600);
        for hostname in ["shop.example.com", "canopy.example.com"] {
            reconciler
                .warm_cert_first_seen
                .insert(hostname.to_owned(), long_ago);
        }

        oi.block_on(reconciler.observe_warm_certs(std::slice::from_ref(&snapshot)));

        let filed: Vec<String> = oi.state.db.call(|db| {
            crate::runtime::faults::list_active_faults(db, None)
                .unwrap()
                .into_iter()
                .filter(|f| f.kind == "cert_acquisition_failed")
                .map(|f| f.description)
                .collect()
        });
        assert_eq!(filed.len(), 1, "{filed:?}");
        assert!(filed[0].contains("canopy.example.com"), "{filed:?}");
        assert!(
            !filed[0].contains("shop.example.com"),
            "the proxy ordering a certificate is an acquisition in progress: {filed:?}"
        );
    }

    /// Records the ingress ports of every rule set applied, every route set
    /// applied, and how many proxy configurations were applied.
    #[derive(Default)]
    struct RecordingPlanes {
        rules: parking_lot::Mutex<Vec<Vec<u16>>>,
        routes: parking_lot::Mutex<Vec<Vec<crate::system::types::ServiceRoute>>>,
        proxy_applies: std::sync::atomic::AtomicUsize,
    }

    impl crate::system::DataPlane for RecordingPlanes {
        fn apply_rules<'a>(
            &'a self,
            rules: &'a crate::system::types::DataPlaneRules,
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            self.rules
                .lock()
                .push(rules.ingress.iter().map(|r| r.external_port).collect());
            Box::pin(async { Ok(()) })
        }

        fn apply_routes<'a>(
            &'a self,
            routes: &'a [crate::system::types::ServiceRoute],
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            self.routes.lock().push(routes.to_vec());
            Box::pin(async { Ok(()) })
        }

        fn ensure_isolation<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            Box::pin(async { Ok(()) })
        }

        fn clear_all<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl crate::system::NetworkProxy for RecordingPlanes {
        fn is_healthy<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<'a, Result<bool, crate::system::BoxError>> {
            Box::pin(async { Ok(true) })
        }

        fn apply_config<'a>(
            &'a self,
            _config: &'a crate::system::types::ProxyConfig,
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            self.proxy_applies
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn dtls_refusals<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<
            'a,
            Result<std::collections::BTreeMap<String, jiff::Timestamp>, crate::system::BoxError>,
        > {
            Box::pin(async { Ok(Default::default()) })
        }
    }

    fn site_ingress_with_attachment(db: &crate::runtime::db::Db, port: u16) {
        use crate::runtime::{
            site_ingress_attachments::{
                self, AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
            },
            site_ingresses::{self, SiteIngressDef, SiteIngressSource, TlsProvider},
        };
        use seedling_protocol::names::{AppServiceName, SiteIngressName};

        let gate = SiteIngressName::new("gate").unwrap();
        site_ingresses::create(
            db,
            &SiteIngressDef {
                name: gate.clone(),
                hostname: "gate.example.com".into(),
                description: None,
                source: SiteIngressSource::Manual,
                tls_provider: TlsProvider::None,
                stale: false,
                created_at: "2026-11-17T00:00:00Z".into(),
            },
        )
        .unwrap();
        site_ingress_attachments::attach(
            db,
            &SiteIngressAttachment {
                site_ingress: gate,
                port,
                protocol: AttachmentProtocol::Tcp,
                target: AttachmentTarget::Forward {
                    app: AppName::new("web").unwrap(),
                    service: AppServiceName::new("api").unwrap(),
                },
                created_at: "2026-11-17T00:00:00Z".into(),
            },
        )
        .unwrap();
    }

    fn rename_table(oi: &crate::oi::test_support::TestOi, from: &'static str, to: &'static str) {
        oi.state.db.call(move |db| {
            db.conn
                .execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
                .unwrap();
        });
    }

    // spec: SITE#unreadable-site-ingresses
    // spec: FLT#host-faults
    #[test]
    fn site_ingresses_unreadable_is_held_until_they_are_read() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.state
            .db
            .call(|db| site_ingress_with_attachment(db, 8443));
        let reconciler = test_reconciler(&oi);
        let held = |oi: &crate::oi::test_support::TestOi| {
            active_kinds(oi, "_system")
                .iter()
                .any(|(kind, _)| kind == "site_ingresses_unreadable")
        };

        rename_table(&oi, "site_ingress_attachments", "hidden_attachments");
        assert!(
            reconciler.load_site_ingresses().is_none(),
            "a failed read is not an empty one"
        );
        assert!(held(&oi), "filed while the attachments cannot be read");

        rename_table(&oi, "hidden_attachments", "site_ingress_attachments");
        let snapshot = reconciler.load_site_ingresses().expect("read");
        assert_eq!(snapshot.attachments.len(), 1);
        assert!(!held(&oi), "cleared on the next read");
    }

    // spec: SITE#unreadable-site-ingresses
    // spec: RCN#applying-absolute-state
    // A failed read of the site ingresses used to stand in an empty list, so
    // the iteration applied a proxy configuration and rules with every site
    // ingress port withdrawn, and cleared the faults about the attachments.
    #[test]
    fn unreadable_site_ingresses_withhold_the_proxy_and_rules() {
        const PORT: u16 = 8443;
        let oi = crate::oi::test_support::TestOi::new();
        // A target that resolves, so the attachment's port is one the proxy
        // serves and the rules redirect.
        let apps = vec![snapshot_with_priorities(
            "web",
            AppPriority::default(),
            r#"
            let api = app.service("api");
            app.deployment("web").image("docker.io/library/nginx:1.29").tcp(80, api.port(8080));
            "#,
        )];
        let registry = crate::runtime::registry::EphemeralInstanceRegistry::new();
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &empty_lookup,
        };
        let node_prefix: Ipv6Net = "fd5e:ed11:9000::/48".parse().unwrap();
        let external =
            crate::runtime::external_service_mappings::ExternalServiceSnapshot::default();
        let planes = RecordingPlanes::default();
        let apply = |site: Option<&site_proxy::SiteIngressSnapshot>| {
            let proxy = phases::compute_proxy_config(
                &apps,
                Coverage::Complete,
                site,
                &HashMap::new(),
                &node_prefix,
                &registry,
                None,
                &std::collections::BTreeMap::new(),
            );
            let nft = phases::compute_nftables_rules(
                &apps,
                &HashMap::new(),
                std::net::Ipv6Addr::LOCALHOST,
                None,
                &node_prefix,
                &registry,
                Some(&external),
                &resolve_ctx,
                &proxy,
            );
            let rules = oi.block_on(apply_rules_plane(&planes, &nft.rules, nft.coverage));
            let config = oi.block_on(apply_proxy_plane(&planes, &proxy.config, proxy.coverage));
            (rules.is_some(), config.is_some(), proxy)
        };
        let applied_ports = || planes.rules.lock().last().cloned().unwrap_or_default();
        let proxy_applies = || {
            planes
                .proxy_applies
                .load(std::sync::atomic::Ordering::SeqCst)
        };

        oi.state
            .db
            .call(|db| site_ingress_with_attachment(db, PORT));
        let read = oi.state.db.call(site_proxy::load).unwrap();
        let (rules, config, proxy) = apply(Some(&read));
        assert!(rules && config);
        assert!(applied_ports().contains(&PORT));
        assert_eq!(
            proxy.unresolved_site_attachments.map(|u| u.len()),
            Some(0),
            "the attachment's target resolves"
        );
        assert!(proxy.conflicts.is_some());

        let (rules, config, proxy) = apply(None);
        assert!(!rules && !config, "both applies are withheld");
        assert_eq!(planes.rules.lock().len(), 1, "no rule set was applied");
        assert_eq!(proxy_applies(), 1, "no proxy configuration was applied");
        assert!(
            proxy.unresolved_site_attachments.is_none() && proxy.conflicts.is_none(),
            "nothing is concluded about the attachments' faults"
        );

        // Read and empty is an answer: the port is withdrawn.
        oi.state.db.call(|db| {
            db.conn
                .execute("DELETE FROM site_ingress_attachments", [])
                .unwrap();
        });
        let read = oi.state.db.call(site_proxy::load).unwrap();
        let (rules, config, proxy) = apply(Some(&read));
        assert!(rules && config);
        assert!(!applied_ports().contains(&PORT));
        assert_eq!(proxy_applies(), 2);
        assert_eq!(
            proxy.unresolved_site_attachments.map(|u| u.len()),
            Some(0),
            "the removed attachment's fault clears"
        );
    }

    // spec: FLT#filing-and-clearing
    // spec: ING#claims-across-apps
    // spec: RCN#applying-absolute-state
    // An app held back from a tick used to be read as claiming nothing, so the
    // conflict faults were converged without it: its own cleared, and an app it
    // outranks had its cleared too, as though it had won the port.
    #[test]
    fn an_app_held_back_from_a_tick_keeps_the_ingress_conflicts_as_they_were() {
        const SCRIPT: &str = r#"
            let web = app.service("web").exported().http(80);
            web.ingress("shop.example.com", 443).tls(Terminate.Https, Output.Http1);
            app.deployment("web")
                .image("docker.io/library/nginx:1.29")
                .http(80, web.route("/"));
        "#;
        let oi = crate::oi::test_support::TestOi::new();
        for app in ["first", "second"] {
            oi.call(
                "/apps/create",
                serde_json::json!({ "app": app, "script": SCRIPT }),
            )
            .unwrap();
            oi.install(app);
        }
        let mut reconciler = test_reconciler(&oi);
        let node_prefix = oi.state.node_prefix;
        let mut evaluate = || {
            let (apps, dropped) = reconciler.snapshot_all_apps();
            let site = oi.state.db.call(site_proxy::load).unwrap();
            let build = phases::compute_proxy_config(
                &apps,
                Coverage::of(dropped),
                Some(&site),
                &HashMap::new(),
                &node_prefix,
                &*reconciler.registry,
                None,
                &std::collections::BTreeMap::new(),
            );
            if let Some(conflicts) = &build.conflicts {
                reconciler.reconcile_ingress_conflicts(conflicts);
            }
            build
        };
        let hold_back = |app: Option<&'static str>| {
            oi.state.db.call(move |db| {
                db.conn.execute("DELETE FROM app_priorities", []).unwrap();
                if let Some(app) = app {
                    db.conn
                        .execute(
                            "INSERT INTO app_priorities (app, priority, updated_at) \
                             VALUES (?1, 'urgent', '')",
                            [app],
                        )
                        .unwrap();
                }
            });
        };
        let conflict = vec![(
            "ingress_conflict".to_owned(),
            "shop.example.com:443".to_owned(),
        )];

        let build = evaluate();
        assert!(build.conflicts.is_some());
        assert_eq!(
            active_kinds(&oi, "second"),
            conflict,
            "the later install loses"
        );
        assert!(active_kinds(&oi, "first").is_empty());

        hold_back(Some("second"));
        let build = evaluate();
        assert!(build.conflicts.is_none() && build.unresolved_site_attachments.is_none());
        assert_eq!(
            active_kinds(&oi, "second"),
            conflict,
            "the held-back app's conflict is not resolved by its absence"
        );

        hold_back(Some("first"));
        evaluate();
        assert_eq!(
            active_kinds(&oi, "second"),
            conflict,
            "the app installed first still holds the port while it is held back"
        );

        hold_back(None);
        *oi.state.registry.read().get("first").unwrap().phase.lock() = AppPhase::NotInstalled;
        let build = evaluate();
        assert!(build.conflicts.is_some());
        assert!(
            active_kinds(&oi, "second").is_empty(),
            "evaluated again with the conflict gone, the fault clears"
        );
    }

    const SITE_SERVICE_APP_SCRIPT: &str = r#"
        let db = app.external_service("database");
        app.deployment("web").image("docker.io/library/nginx:1.29").mount(db.port(5432));
    "#;

    /// Map `app`'s `database` external service to a `shared-db` site service
    /// with one endpoint at `remote_host`.
    fn site_service_mapped_from(db: &crate::runtime::db::Db, app: &str, remote_host: &str) {
        use crate::runtime::{
            external_service_mappings::{self, ExternalServiceMapping},
            site_services::{self, SiteServiceDef, SiteServiceEndpoint, SiteServiceProtocol},
        };
        use seedling_protocol::names::{ExternalServiceName, ServiceRef, SiteServiceName};

        let name = SiteServiceName::new("shared-db").unwrap();
        site_services::create(
            db,
            &SiteServiceDef {
                name: name.clone(),
                description: None,
                endpoints: vec![SiteServiceEndpoint {
                    service_port: 5432,
                    protocol: SiteServiceProtocol::Tcp,
                    remote_host: remote_host.to_owned(),
                    remote_port: 5432,
                }],
                created_at: "2026-11-17T00:00:00Z".into(),
            },
        )
        .unwrap();
        external_service_mappings::create(
            db,
            &ExternalServiceMapping {
                app: AppName::new(app).unwrap(),
                external_name: ExternalServiceName::new("database").unwrap(),
                target: ServiceRef::Site { name },
            },
        )
        .unwrap();
    }

    // spec: SITE#unreadable-site-services
    // spec: FLT#host-faults
    #[test]
    fn site_services_unreadable_is_held_until_they_are_read() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "demo", "script": SITE_SERVICE_APP_SCRIPT }),
        )
        .unwrap();
        oi.state
            .db
            .call(|db| site_service_mapped_from(db, "demo", "2001:db8::10"));
        let reconciler = test_reconciler(&oi);
        let held = |oi: &crate::oi::test_support::TestOi| {
            active_kinds(oi, "_system")
                .iter()
                .any(|(kind, _)| kind == "site_services_unreadable")
        };

        rename_table(&oi, "site_service_endpoints", "hidden_endpoints");
        assert!(
            reconciler.load_external_services().is_none(),
            "a failed read is not an empty one"
        );
        assert!(held(&oi), "filed while the site services cannot be read");

        rename_table(&oi, "hidden_endpoints", "site_service_endpoints");
        let snapshot = reconciler.load_external_services().expect("read");
        assert_eq!(snapshot.mappings.len(), 1);
        assert_eq!(snapshot.site_endpoints.len(), 1);
        assert!(!held(&oi), "cleared on the next read");
    }

    // spec: SITE#unreadable-site-services
    // spec: RCN#applying-absolute-state
    // spec: LCY#services
    // A failed read of the site services and external service mappings used
    // to stand in an empty snapshot, so the iteration applied routes and rules
    // with every external service's backends withdrawn, and observed each
    // external service as having lost its backends.
    #[test]
    fn unreadable_site_services_withhold_the_routes_and_rules() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "demo", "script": SITE_SERVICE_APP_SCRIPT }),
        )
        .unwrap();
        let apps = vec![snapshot_with_priorities(
            "demo",
            AppPriority::default(),
            SITE_SERVICE_APP_SCRIPT,
        )];
        let registry = crate::runtime::registry::EphemeralInstanceRegistry::new();
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &empty_lookup,
        };
        let node_prefix: Ipv6Net = "fd5e:ed11:9000::/48".parse().unwrap();
        let planes = RecordingPlanes::default();
        let apply = |external: Option<
            &crate::runtime::external_service_mappings::ExternalServiceSnapshot,
        >| {
            let routes = phases::compute_routes(
                &apps,
                &HashMap::new(),
                &node_prefix,
                &registry,
                external,
                &resolve_ctx,
            );
            let proxy = phases::compute_proxy_config(
                &apps,
                Coverage::Complete,
                Some(&site_proxy::SiteIngressSnapshot {
                    ingresses: Vec::new(),
                    attachments: Vec::new(),
                }),
                &HashMap::new(),
                &node_prefix,
                &registry,
                None,
                &std::collections::BTreeMap::new(),
            );
            let nft = phases::compute_nftables_rules(
                &apps,
                &HashMap::new(),
                std::net::Ipv6Addr::LOCALHOST,
                None,
                &node_prefix,
                &registry,
                external,
                &resolve_ctx,
                &proxy,
            );
            let routes_applied = oi
                .block_on(apply_routes_plane(&planes, &routes.routes, routes.coverage))
                .is_some();
            let rules_applied = oi
                .block_on(apply_rules_plane(&planes, &nft.rules, nft.coverage))
                .is_some();
            let external_observed = routes
                .observations
                .iter()
                .any(|(instance, _, _)| instance.kind == ResourceKind::ExternalService);
            (routes_applied, rules_applied, external_observed)
        };
        let external_backends = || -> Vec<std::net::Ipv6Addr> {
            let routes = planes.routes.lock();
            let last = routes.last().expect("a route set was applied");
            last.iter()
                .flat_map(|route| route.backends.iter().copied())
                .collect()
        };
        let reconciler = test_reconciler(&oi);
        oi.state
            .db
            .call(|db| site_service_mapped_from(db, "demo", "2001:db8::10"));
        let (routes, rules, observed) = apply(reconciler.load_external_services().as_ref());
        assert!(routes && rules && observed);
        assert_eq!(
            external_backends(),
            ["2001:db8::10".parse::<std::net::Ipv6Addr>().unwrap()]
        );

        rename_table(&oi, "site_service_endpoints", "hidden_endpoints");
        let (routes, rules, observed) = apply(reconciler.load_external_services().as_ref());
        assert!(!routes && !rules, "both applies are withheld");
        assert_eq!(planes.routes.lock().len(), 1, "no route set was applied");
        assert_eq!(planes.rules.lock().len(), 1, "no rule set was applied");
        assert!(
            !observed,
            "the external service is not observed as having lost its backends"
        );

        // Read and empty is an answer: the site service's backend is removed.
        rename_table(&oi, "hidden_endpoints", "site_service_endpoints");
        oi.state.db.call(|db| {
            db.conn
                .execute_batch(
                    "DELETE FROM external_service_mappings; DELETE FROM site_service_endpoints;",
                )
                .unwrap();
        });
        let (routes, rules, observed) = apply(reconciler.load_external_services().as_ref());
        assert!(routes && rules && observed);
        assert!(external_backends().is_empty());
        assert_eq!(planes.rules.lock().len(), 2);
    }

    // spec: SITE#unreadable-site-services
    // spec: SITE#endpoint-addressing
    // A failed read of the site services used to stand in an empty snapshot,
    // which classifies no endpoint as unroutable and so cleared every site
    // service endpoint fault.
    #[test]
    fn unreadable_site_services_leave_the_endpoint_faults_as_they_were() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({ "app": "demo", "script": SITE_SERVICE_APP_SCRIPT }),
        )
        .unwrap();
        // An IPv4 literal is unroutable without NAT64.
        oi.state
            .db
            .call(|db| site_service_mapped_from(db, "demo", "192.0.2.10"));
        let mut reconciler = test_reconciler(&oi);
        let empty_lookup = crate::runtime::site_services::resolve::EmptyLookup;
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &empty_lookup,
        };
        let unroutable = |oi: &crate::oi::test_support::TestOi| {
            active_kinds(oi, "_system")
                .iter()
                .any(|(kind, _)| kind == "site_service_endpoint_unroutable")
        };

        let read = reconciler.load_external_services();
        reconciler.converge_site_service_faults(read.as_ref(), &resolve_ctx);
        assert!(unroutable(&oi));

        rename_table(&oi, "site_service_endpoints", "hidden_endpoints");
        let read = reconciler.load_external_services();
        reconciler.converge_site_service_faults(read.as_ref(), &resolve_ctx);
        assert!(
            unroutable(&oi),
            "not cleared while the site services cannot be read"
        );

        rename_table(&oi, "hidden_endpoints", "site_service_endpoints");
        oi.state.db.call(|db| {
            db.conn
                .execute("DELETE FROM site_service_endpoints", [])
                .unwrap();
        });
        let read = reconciler.load_external_services();
        reconciler.converge_site_service_faults(read.as_ref(), &resolve_ctx);
        assert!(
            !unroutable(&oi),
            "an endpoint read as removed clears its fault"
        );
    }

    /// A reconciler over `oi`'s stub backends and database.
    pub(super) fn test_reconciler(oi: &crate::oi::test_support::TestOi) -> Reconciler {
        Reconciler::new(
            Arc::clone(&oi.state.driver),
            oi.state.node_prefix,
            Arc::new(crate::runtime::registry::DbInstanceRegistry::new(
                oi.state.db.clone(),
            )),
            Arc::new(AsyncRwLock::new(reqwest::Client::new())),
            std::env::temp_dir(),
            oi.state.db.clone(),
            Arc::clone(&oi.state.registry),
            Arc::clone(&oi.state.cipher),
            oi.state.event_tx.clone(),
            Vec::new(),
            Vec::new(),
            false,
            false,
            Arc::clone(&oi.state.shells),
            None,
            None,
            None,
            crate::runtime::gc::InstanceGcConfig {
                interval: std::time::Duration::from_secs(3600),
                retain: std::time::Duration::from_secs(3600),
                retain_restart_records: 50,
            },
            Arc::clone(&oi.state.gate_runtime),
        )
    }

    /// A proxy whose answer about refused DTLS sessions the test sets.
    struct RefusingProxy(
        parking_lot::Mutex<Result<std::collections::BTreeMap<String, jiff::Timestamp>, String>>,
    );

    impl crate::system::NetworkProxy for RefusingProxy {
        fn is_healthy<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<'a, Result<bool, crate::system::BoxError>> {
            Box::pin(async { Ok(true) })
        }

        fn apply_config<'a>(
            &'a self,
            _config: &'a crate::system::types::ProxyConfig,
        ) -> crate::system::BoxFuture<'a, Result<(), crate::system::BoxError>> {
            Box::pin(async { Ok(()) })
        }

        fn dtls_refusals<'a>(
            &'a self,
        ) -> crate::system::BoxFuture<
            'a,
            Result<std::collections::BTreeMap<String, jiff::Timestamp>, crate::system::BoxError>,
        > {
            let answer = self.0.lock().clone();
            Box::pin(async move { answer.map_err(Into::into) })
        }
    }

    // spec: FLT#ingress-and-service-faults
    // spec: PRX#dtls-terminating-ingresses
    #[test]
    fn dtls_session_refused_is_filed_on_a_recent_refusal_and_cleared_when_it_is_not_one() {
        use crate::system::{
            ContainerRuntime, ProcessManager,
            stub::{StubContainerRuntime, StubProcessManager},
        };

        let dir = tempfile::tempdir().unwrap();
        let volumes = dir.path().join("stub-volumes");
        std::fs::create_dir_all(&volumes).unwrap();
        let container = Arc::new(StubContainerRuntime::new(volumes));
        let process = Arc::new(StubProcessManager::new(Arc::clone(&container)));
        let backends = crate::system::backends::Backends::working(
            Arc::clone(&container) as Arc<dyn ContainerRuntime>,
            Arc::clone(&process) as Arc<dyn ProcessManager>,
        );
        let proxy = Arc::new(RefusingProxy(parking_lot::Mutex::new(Ok(
            Default::default(),
        ))));
        let system = Arc::new(System::for_tests(
            backends,
            Arc::clone(&proxy) as Arc<dyn crate::system::NetworkProxy>,
            dir.path(),
        ));
        let oi = crate::oi::test_support::TestOi::with_driver(system);
        let reconciler = test_reconciler(&oi);

        let ingress = "voip.example.com:5684".to_owned();
        let served = vec![(AppName::new_unchecked("voip"), ingress.clone())];
        let refused_ago = |secs: i64| {
            Ok(std::collections::BTreeMap::from([(
                ingress.clone(),
                jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - secs).unwrap(),
            )]))
        };
        let held = || -> Vec<String> {
            active_kinds(&oi, "voip")
                .into_iter()
                .filter(|(k, _)| k == "dtls_session_refused")
                .map(|(_, s)| s)
                .collect()
        };
        let reconcile = |served: &[(AppName, String)]| {
            oi.block_on(reconciler.reconcile_dtls_refusals(served));
        };

        reconcile(&served);
        assert!(held().is_empty(), "nothing refused");

        *proxy.0.lock() = refused_ago(10);
        reconcile(&served);
        assert_eq!(held(), vec![ingress.clone()], "a recent refusal is held");

        // The route cannot be read: not "none refused", so nothing clears.
        *proxy.0.lock() = Err("admin API unreachable".to_owned());
        reconcile(&served);
        assert_eq!(
            held(),
            vec![ingress.clone()],
            "an unreadable route clears nothing"
        );

        // Five quiet minutes.
        *proxy.0.lock() = refused_ago(6 * 60);
        reconcile(&served);
        assert!(held().is_empty(), "cleared after the quiet period");

        // Removed from the definition while refusing.
        *proxy.0.lock() = refused_ago(10);
        reconcile(&served);
        assert_eq!(held().len(), 1);
        reconcile(&[]);
        assert!(held().is_empty(), "cleared when the ingress is gone");
    }

    // spec: AUTO#restart-pacing
    #[test]
    fn a_crash_looping_instance_the_supervisor_would_restart_is_stopped() {
        use crate::system::{
            ContainerRuntime, ProcessManager,
            stub::{StubContainerRuntime, StubNetworkProxy, StubProcessManager},
            types::ActiveState,
        };

        let dir = tempfile::tempdir().unwrap();
        let volumes = dir.path().join("stub-volumes");
        std::fs::create_dir_all(&volumes).unwrap();
        let container = Arc::new(StubContainerRuntime::new(volumes));
        let process = Arc::new(StubProcessManager::new(Arc::clone(&container)));
        let backends = crate::system::backends::Backends::working(
            Arc::clone(&container) as Arc<dyn ContainerRuntime>,
            Arc::clone(&process) as Arc<dyn ProcessManager>,
        );
        let system = Arc::new(System::for_tests(
            backends,
            Arc::new(StubNetworkProxy),
            dir.path(),
        ));
        let oi = crate::oi::test_support::TestOi::with_driver(system);
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"app.deployment("web").image("docker.io/library/nginx:1.30");"#,
            }),
        )
        .unwrap();
        oi.install("demo");
        let reconciler = test_reconciler(&oi);
        let (apps, _) = reconciler.snapshot_all_apps();
        let app = apps.iter().find(|a| a.name.as_str() == "demo").unwrap();
        let actuate = |crash_looped: &HashSet<InstanceId>| {
            oi.block_on(pods::observe_and_actuate(
                &reconciler.observer,
                &reconciler.actuator,
                &reconciler.driver,
                &reconciler.db,
                &app.desired,
                app.app_priority,
                &app.gate_keys,
                &app.gates,
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                crash_looped,
            ))
        };
        oi.block_on(container.pull_image("docker.io/library/nginx:1.30"))
            .unwrap();
        let _ = actuate(&HashSet::new());
        let instance = app.desired.resources[0].instance.clone();
        let unit = crate::system::actuator::unit_name(&instance);
        let state = |oi: &crate::oi::test_support::TestOi| {
            oi.block_on(process.unit_state(&unit))
                .unwrap()
                .map(|s| s.active)
        };
        assert_eq!(state(&oi), Some(ActiveState::Active), "started");

        process.simulate_auto_restart_pause(&unit);
        let _ = actuate(&HashSet::from([instance.id]));

        assert_ne!(
            state(&oi),
            Some(ActiveState::Activating),
            "the supervisor's pending restart is cancelled"
        );
        assert!(
            oi.block_on(container.inspect(&instance.display_name))
                .unwrap()
                .is_none(),
            "and nothing started it again"
        );
    }

    // spec: UPD#stale-instances
    #[test]
    fn a_gate_key_that_cannot_be_found_is_an_error_rather_than_no_spec_hash() {
        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"
                    let migrate = app.gate("migrate").key("1.0.0");
                    app.deployment("web").image("docker.io/library/nginx:1.30").after(migrate);
                    app.deployment("plain").image("docker.io/library/nginx:1.30");
                "#,
            }),
        )
        .unwrap();
        oi.install("demo");
        let reconciler = test_reconciler(&oi);
        let (apps, _) = reconciler.snapshot_all_apps();
        let app = apps.iter().find(|a| a.name.as_str() == "demo").unwrap();
        let hash = |name: &str| {
            let dr = app
                .desired
                .resources
                .iter()
                .find(|r| r.instance.name.as_deref() == Some(name))
                .unwrap();
            reconciler.actuator.desired_spec_hash(
                &dr.instance,
                &dr.definition,
                &GateKeyDigests::default(),
            )
        };
        assert!(
            matches!(
                hash("web"),
                Err(crate::system::actuator::SpecHashError::GateKey { .. })
            ),
            "web is ordered after a gate the digests hold nothing for"
        );
        assert!(
            hash("plain").is_ok(),
            "plain is ordered after no gate, so no key is missing"
        );
    }

    // spec: UPD#spec-hash
    #[test]
    fn a_workloads_spec_hash_moves_with_the_key_of_a_gate_it_is_ordered_directly_after_only() {
        use crate::system::{
            ContainerRuntime, ProcessManager,
            stub::{StubContainerRuntime, StubNetworkProxy, StubProcessManager},
        };

        let dir = tempfile::tempdir().unwrap();
        let volumes = dir.path().join("stub-volumes");
        std::fs::create_dir_all(&volumes).unwrap();
        let container = Arc::new(StubContainerRuntime::new(volumes));
        let process = Arc::new(StubProcessManager::new(Arc::clone(&container)));
        let backends = crate::system::backends::Backends::working(
            Arc::clone(&container) as Arc<dyn ContainerRuntime>,
            Arc::clone(&process) as Arc<dyn ProcessManager>,
        );
        let system = Arc::new(System::for_tests(
            backends,
            Arc::new(StubNetworkProxy),
            dir.path(),
        ));
        let oi = crate::oi::test_support::TestOi::with_driver(system);
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"
                    let migrate = app.gate("migrate").key("1.0.0");
                    let web = app.deployment("web").image("docker.io/library/nginx:1.30").after(migrate);
                    app.deployment("worker").image("docker.io/library/nginx:1.30").after(web);
                    app.job("report").image("docker.io/library/nginx:1.30").after(migrate);
                    app.deployment("plain").image("docker.io/library/nginx:1.30");
                "#,
            }),
        )
        .unwrap();
        oi.install("demo");
        let reconciler = test_reconciler(&oi);
        let (apps, _) = reconciler.snapshot_all_apps();
        let app = apps.iter().find(|a| a.name.as_str() == "demo").unwrap();
        // Nothing is started while the gate is unsatisfied, so each workload's
        // pod network is recorded here, as a start would record it.
        for resource in &app.desired.resources {
            reconciler
                .actuator
                .allocate_pod_prefix(&resource.instance)
                .unwrap();
        }

        let moved = GateKeyDigests::of(
            &{
                let mut def = (*app.app_def).clone();
                def.gates.get_mut("migrate").unwrap().key =
                    crate::defs::gate::GateKey::from_script("2.0.0".into()).unwrap();
                def
            },
            &oi.state.cipher,
        )
        .unwrap();
        assert_ne!(&moved, app.gate_keys.current());

        let hash = |name: &str, keys: &GateKeyDigests| {
            let dr = app
                .desired
                .resources
                .iter()
                .find(|r| r.instance.name.as_deref() == Some(name))
                .unwrap();
            reconciler
                .actuator
                .desired_spec_hash(&dr.instance, &dr.definition, keys)
                .unwrap()
                .unwrap_or_else(|| panic!("{name} has a spec hash"))
        };
        for directly_after in ["web", "report"] {
            assert_ne!(
                hash(directly_after, app.gate_keys.current()),
                hash(directly_after, &moved),
                "{directly_after} is ordered directly after the gate"
            );
        }
        for not_directly_after in ["worker", "plain"] {
            assert_eq!(
                hash(not_directly_after, app.gate_keys.current()),
                hash(not_directly_after, &moved),
                "{not_directly_after} is not ordered directly after the gate"
            );
        }
    }

    // spec: APPS#resources
    #[test]
    fn a_change_of_an_instances_health_is_an_event_and_an_unchanged_one_is_not() {
        use crate::runtime::history::insert_observation;

        let oi = crate::oi::test_support::TestOi::new();
        oi.call(
            "/apps/create",
            serde_json::json!({
                "app": "demo",
                "script": r#"
                    app.deployment("web")
                        .image("docker.io/library/nginx:1.30")
                        .healthcheck(#{ kind: "command", cmd: ["true"] });
                "#,
            }),
        )
        .unwrap();
        oi.install("demo");
        let mut reconciler = test_reconciler(&oi);
        let mut events = oi.state.event_tx.subscribe();
        let instance = crate::runtime::identity::ResourceInstance::new_singleton(
            AppName::new("demo").unwrap(),
            crate::defs::resource::ResourceKind::Deployment,
            "web",
        );

        let mut tick = |kinds: &[&str]| {
            let kinds: Vec<String> = kinds.iter().map(|k| (*k).to_owned()).collect();
            let instance = instance.clone();
            oi.state.db.call(move |db| {
                for kind in kinds {
                    insert_observation(db, &instance, &kind, &serde_json::json!({})).unwrap();
                }
            });
            let (apps, _) = reconciler.snapshot_all_apps();
            reconciler.emit_state_changes(&apps);
            let mut health = Vec::new();
            while let Ok(event) = events.try_recv() {
                let event = serde_json::to_value(&event).unwrap();
                if event["type"] == "InstanceHealthChanged" {
                    assert_eq!(event["resource_type"], "deployment");
                    assert_eq!(event["resource_name"], "web");
                    health.push(event["health"].clone());
                }
            }
            health
        };

        assert!(tick(&[]).is_empty(), "the first sight is not a change");
        assert!(
            tick(&["container_created", "container_running"]).is_empty(),
            "no check has been observed"
        );
        assert_eq!(
            tick(&["health_check_starting"]),
            vec![serde_json::json!("starting")]
        );
        assert!(tick(&[]).is_empty(), "unchanged");
        assert_eq!(
            tick(&["health_check_pass"]),
            vec![serde_json::json!("passing")]
        );
        assert_eq!(
            tick(&["health_check_fail"]),
            vec![serde_json::json!("failing")]
        );
        assert_eq!(
            tick(&["container_exited", "container_removed"]),
            vec![serde_json::json!(null)],
            "with no running container the health is null"
        );
    }

    // spec: OBS#recording
    #[test]
    fn a_fact_that_flips_back_is_recorded_again_and_an_unchanged_one_is_not() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi);
        let instance = crate::runtime::identity::ResourceInstance::new_singleton(
            AppName::new("demo").unwrap(),
            crate::defs::resource::ResourceKind::Deployment,
            "web",
        );
        let count = |kind: &'static str| {
            let instance = instance.clone();
            oi.state.db.call(move |db| {
                crate::runtime::history::query_observations(db, &instance)
                    .unwrap()
                    .iter()
                    .filter(|o| o.obs_kind == kind)
                    .count()
            })
        };
        let mut observe = |kind: &'static str| {
            reconciler.persist_obs(vec![(instance.clone(), kind, serde_json::json!({}))]);
        };

        observe("health_check_pass");
        observe("health_check_pass");
        assert_eq!(count("health_check_pass"), 1, "unchanged: recorded once");

        observe("health_check_fail");
        observe("health_check_pass");
        assert_eq!(count("health_check_fail"), 1);
        assert_eq!(
            count("health_check_pass"),
            2,
            "it flipped back, so it is news again"
        );
    }

    // spec: OBS#recording
    #[test]
    fn a_fact_that_flips_back_after_a_restart_is_recorded_again() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let instance = crate::runtime::identity::ResourceInstance::new_singleton(
            AppName::new("demo").unwrap(),
            crate::defs::resource::ResourceKind::Deployment,
            "web",
        );
        let mut first = test_reconciler(&oi);
        for kind in ["health_check_pass", "health_check_fail"] {
            first.persist_obs(vec![(instance.clone(), kind, serde_json::json!({}))]);
        }

        let mut restarted = test_reconciler(&oi);
        restarted.persist_obs(vec![(
            instance.clone(),
            "health_check_pass",
            serde_json::json!({}),
        )]);

        let passes = oi.state.db.call(move |db| {
            crate::runtime::history::query_observations(db, &instance)
                .unwrap()
                .iter()
                .filter(|o| o.obs_kind == "health_check_pass")
                .count()
        });
        assert_eq!(passes, 2, "the last fact on record was the failure");
    }

    // spec: OBS#recording
    #[test]
    fn an_observation_that_failed_to_write_is_written_when_next_observed() {
        let oi = crate::oi::test_support::TestOi::with_app("demo");
        let mut reconciler = test_reconciler(&oi);
        let instance = crate::runtime::identity::ResourceInstance::new_singleton(
            AppName::new("demo").unwrap(),
            crate::defs::resource::ResourceKind::Deployment,
            "web",
        );
        let batch = || vec![(instance.clone(), "container_running", serde_json::json!({}))];
        let count = || {
            let instance = instance.clone();
            oi.state.db.call(move |db| {
                crate::runtime::history::query_observations(db, &instance)
                    .map(|o| o.len())
                    .unwrap_or(0)
            })
        };

        oi.state.db.call(|db| {
            db.conn
                .execute_batch("ALTER TABLE world_observations RENAME TO world_observations_away;")
                .unwrap()
        });
        let recorded = reconciler.persist_obs(batch());
        assert!(
            recorded.is_empty(),
            "nothing was written, so nothing is on record"
        );

        oi.state.db.call(|db| {
            db.conn
                .execute_batch("ALTER TABLE world_observations_away RENAME TO world_observations;")
                .unwrap()
        });
        reconciler.persist_obs(batch());
        assert_eq!(count(), 1, "the next observation of it is written");
    }

    // spec: PRIO#resource-control-groups
    #[test]
    fn an_app_stands_at_each_declared_level_and_at_normal() {
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
        let standings = desired_standings(&apps);

        assert_eq!(standings.len(), 1);
        // Normal is always there: Jobs and other non-Deployment workloads
        // stand at it. Elevated is not declared by any Deployment, so it is
        // not provided for.
        assert_eq!(
            standings[0].levels.iter().copied().collect::<Vec<_>>(),
            [Priority::Low, Priority::Normal, Priority::Critical]
        );
    }

    // spec: PRIO#priority-setting
    // The same definition at a different app priority must produce a
    // different standing: that difference is what a reweight applies.
    #[test]
    fn changing_the_app_priority_changes_the_apps_standing() {
        let script = r#"app.deployment("api").image("docker.io/library/nginx:1");"#;
        let high =
            desired_standings(&[snapshot_with_priorities("demo", AppPriority::High, script)]);
        let low = desired_standings(&[snapshot_with_priorities("demo", AppPriority::Low, script)]);
        assert_eq!(high[0].priority, AppPriority::High);
        assert_eq!(low[0].priority, AppPriority::Low);
        assert_eq!(high[0].levels, low[0].levels);
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
            Some(&crate::runtime::external_service_mappings::ExternalServiceSnapshot::default()),
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

    // spec: SITE#endpoint-addressing
    #[test]
    fn an_endpoint_is_unresolvable_after_five_unanswered_lookups_whatever_it_kept() {
        use crate::runtime::site_services::{
            SiteServiceEndpoint, SiteServiceProtocol, resolve::StaticHostnameLookup,
        };

        let name = seedling_protocol::names::SiteServiceName::new("shared-db").unwrap();
        let mut snapshot =
            crate::runtime::external_service_mappings::ExternalServiceSnapshot::default();
        snapshot.site_endpoints.insert(
            name.clone(),
            vec![SiteServiceEndpoint {
                service_port: 5432,
                protocol: SiteServiceProtocol::Tcp,
                remote_host: "db.example".to_owned(),
                remote_port: 5432,
            }],
        );
        // The answer from before the lookups started going unanswered.
        let mut lookup = StaticHostnameLookup::new();
        lookup.insert("db.example", &["2001:db8::5"], &[], false);
        let resolve_ctx = ResolveCtx {
            nat64_active: false,
            has_ipv6_egress: true,
            resolver: &lookup,
        };

        let unresolved = HashSet::from(["db.example".to_owned()]);
        let set = phases::classify_site_service_endpoints(&snapshot, &resolve_ctx, &unresolved);
        assert_eq!(
            set.unresolvable.get(&name),
            Some(&vec!["db.example".to_owned()])
        );

        let set = phases::classify_site_service_endpoints(&snapshot, &resolve_ctx, &HashSet::new());
        assert!(set.unresolvable.is_empty());
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
            Some(&crate::runtime::external_service_mappings::ExternalServiceSnapshot::default()),
            &resolve_ctx,
        );

        assert_eq!(build.coverage, Coverage::Complete);
        assert_eq!(build.routes.len(), 2);
    }

    // spec: NAT64#translator-lifecycle
    #[test]
    fn the_translator_is_set_up_for_apps_and_torn_down_without_them() {
        use Nat64Translator::{Installed, Removed, Unknown};

        assert_eq!(nat64_step(false, Unknown, false), None);
        assert_eq!(nat64_step(false, Installed, true), None);

        assert_eq!(nat64_step(true, Unknown, false), Some(Nat64Step::SetUp));
        assert_eq!(nat64_step(true, Removed, false), Some(Nat64Step::SetUp));
        assert_eq!(nat64_step(true, Installed, false), None);

        assert_eq!(nat64_step(true, Installed, true), Some(Nat64Step::TearDown));
        assert_eq!(
            nat64_step(true, Unknown, true),
            Some(Nat64Step::TearDown),
            "a translator left by an earlier run is not known to be gone"
        );
        assert_eq!(nat64_step(true, Removed, true), None);
    }

    // spec: FLT#host-faults
    #[test]
    fn a_rejected_upgrades_fault_clears_only_when_an_upgrade_succeeds() {
        use caddy::ProxyImageOutcome::{Current, Replaced, UpgradeRejected};

        let rejected = UpgradeRejected("rejected".to_owned());
        assert_eq!(
            caddy_fault_action(&rejected, false),
            (CaddyFaultAction::File("rejected".to_owned()), true)
        );
        // The previous proxy is healthy, which does not clear it.
        assert_eq!(
            caddy_fault_action(&Current, true),
            (CaddyFaultAction::Hold, true)
        );
        assert_eq!(
            caddy_fault_action(&Replaced, true),
            (CaddyFaultAction::Clear, false)
        );
        // A failed bring-up clears once the proxy is found healthy.
        assert_eq!(
            caddy_fault_action(&Current, false),
            (CaddyFaultAction::Clear, false)
        );
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
