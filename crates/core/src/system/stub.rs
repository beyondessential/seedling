//! In-memory fakes for the system traits, used to boot the daemon without
//! the host's container runtime, process supervisor, packet filter or proxy.
//!
//! Constructed by [`System::setup_stubbed`] when the daemon is started with
//! `--stub-backends`. Every operation succeeds; observed state matches what
//! tests have asked for. The reconciler runs against this stub fleet
//! exactly as it would against the real one, so OI handlers, scheduling,
//! event emission, and DB persistence are all real — only the host-system
//! side effects are faked.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use futures_util::FutureExt;
use ipnet::Ipv4Net;
use parking_lot::Mutex;
use seedling_protocol::names::AppName;

use super::{
    BoxError, BoxFuture, ContainerFilter, ContainerHealth, ContainerRuntime, ContainerSpec,
    ContainerState, ContainerStatus, ContainerSummary, DataPlane, DataPlaneRules, ExecHandle,
    ExecOutput, ImageSummary, NetworkProxy, NetworkSummary, ProcessManager, ProxyConfig,
    ServiceRoute, TransientUnitSpec, UnitState, UnitSummary,
    priority::{AppStandings, UnitStanding},
    types::{ActiveState, SPEC_HASH_LABEL, UnitExit},
};

/// The image reference in a `podman run` argv.
///
/// The image is the first positional argument: `podman_args` emits every
/// flag first, then the image, then entrypoint and command arguments. This
/// used to take the *last* element containing `/` or `:`, which is a command
/// argument whenever one has either — `["sh", "-c", "a/b"]` or
/// `["node", "server/index.js"]` — so a command argument was recorded as the
/// image and a phantom `StubImage` minted for it. Taking the first such
/// element is no better: a volume mount like `-v /host:/ctr` comes earlier.
///
/// So skip flags properly. A `-`-prefixed element is a flag; if it carries
/// `=` its value is attached, otherwise the next element is its value.
fn image_from_run_argv(argv: &[String]) -> Option<String> {
    let mut it = argv.iter().peekable();
    // `podman run`
    while let Some(a) = it.peek() {
        if a.as_str() == "run" {
            it.next();
            break;
        }
        it.next();
    }
    while let Some(a) = it.next() {
        if let Some(flag) = a.strip_prefix('-') {
            if !flag.contains('=') {
                // Separate value form: consume it, unless the next element is
                // itself a flag (a boolean flag such as `--rm`).
                if it.peek().is_some_and(|n| !n.starts_with('-')) {
                    it.next();
                }
            }
            continue;
        }
        return Some(a.clone());
    }
    None
}

/// Stub `ContainerRuntime`. Pretends every started container is healthy and
/// running; tracks just enough state to answer the queries the reconciler
/// makes between actuation ticks.
pub struct StubContainerRuntime {
    state: Mutex<ContainerState_>,
    /// Where stub volumes' mountpoints live. One subdir per volume name; the
    /// runtime owns this dir for the duration of the daemon's lifetime.
    volumes_root: PathBuf,
    /// Whether removing a volume fails, as a container runtime refusing it
    /// does.
    refuse_volume_removal: std::sync::atomic::AtomicBool,
    /// Whether listing networks fails, as an unreadable listing does.
    refuse_network_listing: std::sync::atomic::AtomicBool,
    /// Images whose pulls do not succeed, and how each goes instead.
    pull_faults: Mutex<HashMap<String, PullFault>>,
}

/// How a pull the stub is told to disturb goes instead of succeeding.
#[derive(Clone, Copy)]
pub enum PullFault {
    Refuse,
    Hang,
}

#[derive(Default)]
struct ContainerState_ {
    containers: HashMap<String, StubContainer>,
    networks: HashMap<String, StubNetwork>,
    volumes: HashSet<String>,
    images: HashMap<String, StubImage>,
    next_pid: u32,
}

struct StubContainer {
    spec: ContainerSpec,
    started_at: SystemTime,
    pid: u32,
    state: ContainerStatus,
    image_id: String,
}

struct StubNetwork {
    bridge_name: String,
}

#[derive(Clone)]
struct StubImage {
    id: String,
    tags: Vec<String>,
    digests: Vec<String>,
    manifest_digest: Option<String>,
    size_bytes: i64,
    created_at_secs: i64,
}

impl StubContainerRuntime {
    pub fn new(volumes_root: PathBuf) -> Self {
        Self {
            state: Mutex::new(ContainerState_ {
                next_pid: 10_000,
                ..Default::default()
            }),
            volumes_root,
            refuse_volume_removal: std::sync::atomic::AtomicBool::new(false),
            refuse_network_listing: std::sync::atomic::AtomicBool::new(false),
            pull_faults: Mutex::default(),
        }
    }

    /// Make every later volume removal fail, or succeed again.
    #[cfg(test)]
    pub(crate) fn refuse_volume_removal(&self, refuse: bool) {
        self.refuse_volume_removal
            .store(refuse, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make every later network listing fail, or succeed again.
    #[cfg(test)]
    pub(crate) fn refuse_network_listing(&self, refuse: bool) {
        self.refuse_network_listing
            .store(refuse, std::sync::atomic::Ordering::SeqCst);
    }

    /// Add a network whose bridge is named `bridge_name`, as one left behind
    /// by a previous daemon would be found.
    #[cfg(test)]
    pub(crate) fn seed_network(&self, name: &str, bridge_name: &str) {
        self.state.lock().networks.insert(
            name.to_owned(),
            StubNetwork {
                bridge_name: bridge_name.to_owned(),
            },
        );
    }

    /// Make every later pull of `image` go as `fault` says.
    #[cfg(test)]
    pub(crate) fn disturb_pulls_of(&self, image: &str, fault: PullFault) {
        self.pull_faults.lock().insert(image.to_owned(), fault);
    }

    /// Add a running container carrying `labels`, as one left behind by a
    /// previous daemon would be found.
    #[cfg(test)]
    pub(crate) fn seed_container(&self, name: &str, labels: &[(&str, &str)]) {
        self.seed_container_on(name, "", labels);
    }

    /// [`Self::seed_container`], connected to `network`.
    #[cfg(test)]
    pub(crate) fn seed_container_on(&self, name: &str, network: &str, labels: &[(&str, &str)]) {
        let mut s = self.state.lock();
        let image_id = Self::ensure_image_for(&mut s, "seeded:latest");
        let pid = Self::next_pid(&mut s);
        s.containers.insert(
            name.to_owned(),
            StubContainer {
                spec: ContainerSpec {
                    name: name.to_owned(),
                    image: "seeded:latest".to_owned(),
                    command: vec![],
                    entrypoint: vec![],
                    env: vec![],
                    mounts: vec![],
                    network: network.to_owned(),
                    labels: labels
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect(),
                    health: None,
                    hosts: vec![],
                    dns_servers: vec![],
                    memory: None,
                    cpus: None,
                    extra_caps: vec![],
                    writable_rootfs: false,
                    pids_limit: 0,
                    workdir: None,
                    user: None,
                    stop_signal: None,
                    stop_timeout_secs: None,
                },
                started_at: SystemTime::now(),
                pid,
                state: ContainerStatus::Running,
                image_id,
            },
        );
    }

    fn next_pid(state: &mut ContainerState_) -> u32 {
        let pid = state.next_pid;
        state.next_pid = state.next_pid.wrapping_add(1);
        pid
    }

    fn ensure_image_for(state: &mut ContainerState_, reference: &str) -> String {
        // Fast-resolve a reference to a stable image_id. Used both by
        // pull_image and by start paths so stub-actuated containers carry an
        // image_id the reconciler can pin against.
        if let Some(img) = state.images.values().find(|i| {
            i.tags.iter().any(|t| t == reference)
                || i.digests.iter().any(|d| d == reference)
                || i.id == reference
        }) {
            return img.id.clone();
        }
        let id = format!("sha256:{}", blake_short(reference));
        let img = StubImage {
            id: id.clone(),
            tags: if reference.contains("@sha256:") {
                vec![]
            } else {
                vec![reference.to_owned()]
            },
            digests: if reference.contains("@sha256:") {
                vec![reference.to_owned()]
            } else {
                vec![]
            },
            manifest_digest: Some(id.clone()),
            size_bytes: 1,
            created_at_secs: 0,
        };
        state.images.insert(id.clone(), img);
        id
    }
}

/// Tiny stable hash for stub image IDs. Not cryptographic; just needs to be
/// deterministic and reference-stable across daemon restarts within a test.
fn blake_short(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}{h:016x}{h:016x}{h:016x}")
}

impl ContainerRuntime for StubContainerRuntime {
    fn inspect<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerState>, BoxError>> {
        async move {
            let s = self.state.lock();
            Ok(s.containers.get(name).map(|c| ContainerState {
                status: c.state,
                health: ContainerHealth::Healthy,
                pid: Some(c.pid),
                exit_code: None,
                started_at: Some(c.started_at),
                finished_at: None,
                pod_addr: None,
                pod_addr_v4: None,
                image_id: Some(c.image_id.clone()),
                spec_hash: c.spec.labels.get(SPEC_HASH_LABEL).cloned(),
            }))
        }
        .boxed()
    }

    fn list<'a>(
        &'a self,
        filter: ContainerFilter<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContainerSummary>, BoxError>> {
        async move {
            let s = self.state.lock();
            let mut out = Vec::new();
            for (name, c) in s.containers.iter() {
                if let Some(prefix) = filter.name_prefix
                    && !name.starts_with(prefix)
                {
                    continue;
                }
                if let Some((k, v)) = filter.label
                    && c.spec.labels.get(k).map(String::as_str) != Some(v)
                {
                    continue;
                }
                if let Some(k) = filter.label_key
                    && !c.spec.labels.contains_key(k)
                {
                    continue;
                }
                if let Some(network) = filter.network
                    && c.spec.network != network
                {
                    continue;
                }
                let labels: HashMap<String, String> = c
                    .spec
                    .labels
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                out.push(ContainerSummary {
                    name: name.clone(),
                    status: c.state,
                    labels,
                });
            }
            Ok(out)
        }
        .boxed()
    }

    fn image_exists<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        async move {
            let s = self.state.lock();
            Ok(s.images.values().any(|i| {
                i.tags.iter().any(|t| t == reference)
                    || i.digests.iter().any(|d| d == reference)
                    || i.id == reference
            }))
        }
        .boxed()
    }

    fn pull_image<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let fault = self.pull_faults.lock().get(reference).copied();
            match fault {
                Some(PullFault::Refuse) => {
                    return Err(format!("pull of {reference} refused").into());
                }
                Some(PullFault::Hang) => std::future::pending::<()>().await,
                None => {}
            }
            let mut s = self.state.lock();
            Self::ensure_image_for(&mut s, reference);
            Ok(())
        }
        .boxed()
    }

    fn local_image_id<'a>(
        &'a self,
        reference: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
        async move {
            let s = self.state.lock();
            Ok(s.images
                .values()
                .find(|i| {
                    i.tags.iter().any(|t| t == reference)
                        || i.digests.iter().any(|d| d == reference)
                        || i.id == reference
                })
                .map(|i| i.id.clone()))
        }
        .boxed()
    }

    fn list_images<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ImageSummary>, BoxError>> {
        async move {
            let s = self.state.lock();
            Ok(s.images
                .values()
                .map(|i| ImageSummary {
                    image_id: i.id.clone(),
                    tags: i.tags.clone(),
                    digests: i.digests.clone(),
                    manifest_digest: i.manifest_digest.clone(),
                    size_bytes: i.size_bytes,
                    created_at_secs: i.created_at_secs,
                })
                .collect())
        }
        .boxed()
    }

    fn remove_image<'a>(
        &'a self,
        reference: &'a str,
        _force: bool,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        async move {
            let mut s = self.state.lock();
            let id = s
                .images
                .values()
                .find(|i| {
                    i.tags.iter().any(|t| t == reference)
                        || i.digests.iter().any(|d| d == reference)
                        || i.id == reference
                })
                .map(|i| i.id.clone());
            if let Some(id) = id {
                s.images.remove(&id);
                Ok(true)
            } else {
                Ok(false)
            }
        }
        .boxed()
    }

    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        async move { Ok(self.state.lock().networks.contains_key(name)) }.boxed()
    }

    fn create_network<'a>(
        &'a self,
        name: &'a str,
        bridge_name: &'a str,
        _prefix: ipnet::Ipv6Net,
        _ipv4: Option<Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        async move {
            self.state.lock().networks.insert(
                name.to_owned(),
                StubNetwork {
                    bridge_name: bridge_name.to_owned(),
                },
            );
            Ok(bridge_name.to_owned())
        }
        .boxed()
    }

    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let mut s = self.state.lock();
            if s.containers.values().any(|c| c.spec.network == name) {
                return Err(format!("network {name} is in use").into());
            }
            s.networks.remove(name);
            Ok(())
        }
        .boxed()
    }

    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>> {
        async move {
            if self
                .refuse_network_listing
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("the network listing is refused".into());
            }
            let s = self.state.lock();
            Ok(s.networks
                .iter()
                .filter(|(n, _)| n.starts_with(prefix))
                .map(|(name, net)| NetworkSummary {
                    name: name.clone(),
                    bridge_name: net.bridge_name.clone(),
                })
                .collect())
        }
        .boxed()
    }

    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        async move { Ok(self.state.lock().volumes.contains(name)) }.boxed()
    }

    fn create_volume<'a>(
        &'a self,
        name: &'a str,
        _tmpfs: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let dir = self.volumes_root.join(name);
            std::fs::create_dir_all(&dir)?;
            self.state.lock().volumes.insert(name.to_owned());
            Ok(())
        }
        .boxed()
    }

    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            if self
                .refuse_volume_removal
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(format!("volume {name} is refused removal").into());
            }
            self.state.lock().volumes.remove(name);
            let dir = self.volumes_root.join(name);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Ok(())
        }
        .boxed()
    }

    fn volume_mountpoint<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<PathBuf, BoxError>> {
        async move { Ok(self.volumes_root.join(name)) }.boxed()
    }

    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        _force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            self.state.lock().containers.remove(name);
            Ok(())
        }
        .boxed()
    }

    fn exec<'a>(&'a self, _spec: ContainerSpec) -> BoxFuture<'a, Result<ExecHandle, BoxError>> {
        async move {
            // The shell-session OI handlers reach this when an operator opens
            // a shell. UI tests that exercise the shell page need an actual
            // process behind it; tests that only assert page-rendering behaviour
            // do not. We spawn `/bin/cat` so the test gets a working stdin/
            // stdout pair without depending on podman.
            Err::<ExecHandle, BoxError>(
                "stub ContainerRuntime does not support exec sessions yet"
                    .to_owned()
                    .into(),
            )
        }
        .boxed()
    }

    fn signal_container<'a>(
        &'a self,
        name: &'a str,
        _signal: &'a str,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        async move { Ok(self.state.lock().containers.contains_key(name)) }.boxed()
    }

    fn exec_command<'a>(
        &'a self,
        _name: &'a str,
        _argv: &'a [String],
        _extra_env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<ExecOutput, BoxError>> {
        async move {
            Ok(ExecOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
        .boxed()
    }
}

/// Stub `ProcessManager`. Records transient and persistent unit state in
/// memory; every `start_*` call lands the unit immediately in `Active`.
pub struct StubProcessManager {
    state: Mutex<UnitState_>,
    container: Arc<StubContainerRuntime>,
}

#[derive(Default)]
struct UnitState_ {
    units: BTreeMap<String, UnitRecord>,
    persistent_units: BTreeMap<String, String>,
    /// The standings provided for so far, by app. Overwritten on each sync,
    /// so the map always holds the standings last applied.
    groups: BTreeMap<AppName, AppStandings>,
}

struct UnitRecord {
    state: ActiveState,
    sub: String,
    /// The standing the unit was started with, where the spec carried one.
    standing: Option<UnitStanding>,
    /// Mirrors systemd's `NRestarts`: monotonic while the unit lives, reset
    /// when the unit is recreated. Driven directly by tests.
    restarts: u32,
    last_exit: Option<UnitExit>,
    /// The unit's `Result` property.
    result: Option<String>,
}

impl StubProcessManager {
    pub fn new(container: Arc<StubContainerRuntime>) -> Self {
        Self {
            state: Mutex::new(UnitState_::default()),
            container,
        }
    }

    /// Simulate the supervisor restarting a unit `times` times, the last run
    /// having ended with `exit`. The container keeps running throughout, as it
    /// does on a real host when the restart completes between two observe
    /// ticks — the only trace is the counter.
    pub fn simulate_restarts(&self, unit: &str, times: u32, exit: Option<UnitExit>) {
        if let Some(u) = self.state.lock().units.get_mut(unit) {
            u.restarts += times;
            u.last_exit = exit;
        }
    }

    /// Simulate the supervisor pausing before an automatic restart: the
    /// container has exited and the unit is activating again.
    pub fn simulate_auto_restart_pause(&self, unit: &str) {
        let container_name = unit.strip_suffix(".service").unwrap_or(unit).to_owned();
        self.container
            .state
            .lock()
            .containers
            .remove(&container_name);
        if let Some(u) = self.state.lock().units.get_mut(unit) {
            u.state = ActiveState::Activating;
            u.sub = "auto-restart".to_owned();
        }
    }

    /// Set the counter directly, for the reset case: systemd zeroes
    /// `NRestarts` when a unit is recreated or its failed state is cleared.
    pub fn set_restart_counter(&self, unit: &str, count: u32) {
        if let Some(u) = self.state.lock().units.get_mut(unit) {
            u.restarts = count;
        }
    }

    /// The standings provided for so far, as last applied for each app.
    pub fn groups(&self) -> BTreeMap<AppName, AppStandings> {
        self.state.lock().groups.clone()
    }

    /// The standing a unit was started with.
    pub fn standing(&self, unit: &str) -> Option<UnitStanding> {
        self.state
            .lock()
            .units
            .get(unit)
            .and_then(|u| u.standing.clone())
    }
}

impl ProcessManager for StubProcessManager {
    fn start_transient<'a>(
        &'a self,
        spec: TransientUnitSpec,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            // The transient unit's exec_start is a `podman run [...]` argv;
            // the container's name is the systemd unit name minus the
            // .service suffix. Mirror the real flow loosely so the runtime
            // observer sees a container behind the unit.
            let container_name = spec
                .name
                .strip_suffix(".service")
                .unwrap_or(&spec.name)
                .to_owned();

            let image = image_from_run_argv(&spec.exec_start)
                .unwrap_or_else(|| "stub.local/none:latest".to_owned());

            {
                let mut cs = self.container.state.lock();
                let image_id = StubContainerRuntime::ensure_image_for(&mut cs, &image);
                let pid = StubContainerRuntime::next_pid(&mut cs);
                let labels: std::collections::BTreeMap<String, String> = spec
                    .log_extra_fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                cs.containers.insert(
                    container_name,
                    StubContainer {
                        spec: ContainerSpec {
                            name: spec.name.clone(),
                            image: image.clone(),
                            command: vec![],
                            entrypoint: vec![],
                            env: vec![],
                            mounts: vec![],
                            network: String::new(),
                            labels,
                            health: None,
                            hosts: vec![],
                            dns_servers: vec![],
                            memory: None,
                            cpus: None,
                            extra_caps: vec![],
                            writable_rootfs: false,
                            pids_limit: 0,
                            workdir: None,
                            user: None,
                            stop_signal: None,
                            stop_timeout_secs: None,
                        },
                        started_at: SystemTime::now(),
                        pid,
                        state: ContainerStatus::Running,
                        image_id,
                    },
                );
            }

            // A fresh transient unit starts its restart counter at zero,
            // exactly as systemd does.
            self.state.lock().units.insert(
                spec.name.clone(),
                UnitRecord {
                    state: ActiveState::Active,
                    sub: "running".to_owned(),
                    standing: Some(spec.standing.clone()),
                    restarts: 0,
                    last_exit: None,
                    result: None,
                },
            );
            drop(spec);
            Ok(())
        }
        .boxed()
    }

    fn sync_groups<'a>(
        &'a self,
        apps: Vec<AppStandings>,
        prune: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let mut s = self.state.lock();
            if prune {
                let wanted: std::collections::HashSet<&AppName> =
                    apps.iter().map(|app| &app.app).collect();
                s.groups.retain(|name, _| wanted.contains(name));
            }
            for app in apps {
                s.groups.insert(app.app.clone(), app);
            }
            Ok(())
        }
        .boxed()
    }

    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let container_name = name.strip_suffix(".service").unwrap_or(name).to_owned();
            self.container
                .state
                .lock()
                .containers
                .remove(&container_name);
            if let Some(u) = self.state.lock().units.get_mut(name) {
                u.state = ActiveState::Inactive;
                u.sub = "dead".to_owned();
            }
            Ok(())
        }
        .boxed()
    }

    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            if let Some(u) = self.state.lock().units.get_mut(name)
                && u.state == ActiveState::Failed
            {
                u.state = ActiveState::Inactive;
                u.sub = "dead".to_owned();
                // `systemctl reset-failed` zeroes NRestarts along with the
                // failed state.
                u.restarts = 0;
            }
            Ok(())
        }
        .boxed()
    }

    fn unit_state<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<UnitState>, BoxError>> {
        async move {
            Ok(self.state.lock().units.get(name).map(|u| UnitState {
                active: u.state,
                sub: u.sub.clone(),
                result: u.result.clone(),
                restarts: Some(u.restarts),
                last_exit: u.last_exit,
            }))
        }
        .boxed()
    }

    fn list_units<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<UnitSummary>, BoxError>> {
        async move {
            Ok(self
                .state
                .lock()
                .units
                .iter()
                .filter(|(n, _)| n.starts_with(prefix))
                .map(|(name, u)| UnitSummary {
                    name: name.clone(),
                    state: UnitState {
                        active: u.state,
                        sub: u.sub.clone(),
                        ..Default::default()
                    },
                })
                .collect())
        }
        .boxed()
    }

    fn write_unit<'a>(
        &'a self,
        name: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            self.state
                .lock()
                .persistent_units
                .insert(name.to_owned(), content.to_owned());
            Ok(())
        }
        .boxed()
    }

    fn remove_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            self.state.lock().persistent_units.remove(name);
            Ok(())
        }
        .boxed()
    }

    fn daemon_reload<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        async move { Ok(()) }.boxed()
    }

    fn start_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let mut s = self.state.lock();
            s.units.entry(name.to_owned()).or_insert(UnitRecord {
                state: ActiveState::Active,
                sub: "running".to_owned(),
                standing: None,
                restarts: 0,
                last_exit: None,
                result: None,
            });
            Ok(())
        }
        .boxed()
    }
}

/// Stub `NetworkProxy`: pretends Caddy is up and accepts any config.
pub struct StubNetworkProxy;

impl NetworkProxy for StubNetworkProxy {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
        async { Ok(true) }.boxed()
    }

    fn apply_config<'a>(&'a self, _config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }

    fn dtls_refusals<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<std::collections::BTreeMap<String, jiff::Timestamp>, BoxError>> {
        async { Ok(Default::default()) }.boxed()
    }
}

/// Stub `DataPlane`: no-ops everything. nftables / routing-table effects
/// aren't observable through any OI surface so there's nothing for tests
/// to assert against; the runtime still gets the trait so reconciliation
/// proceeds. It counts installs of the isolation rules, which a test can make
/// fail.
#[derive(Default)]
pub struct StubDataPlane {
    isolation_installs: AtomicUsize,
    refuse_isolation: AtomicBool,
}

impl StubDataPlane {
    /// How many times the isolation rules were installed, failed or not.
    pub fn isolation_installs(&self) -> usize {
        self.isolation_installs.load(Ordering::SeqCst)
    }

    /// Make installing the isolation rules fail, or succeed again.
    pub fn refuse_isolation(&self, refuse: bool) {
        self.refuse_isolation.store(refuse, Ordering::SeqCst);
    }
}

impl DataPlane for StubDataPlane {
    fn apply_rules<'a>(
        &'a self,
        _rules: &'a DataPlaneRules,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }

    fn apply_routes<'a>(
        &'a self,
        _routes: &'a [ServiceRoute],
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }

    fn ensure_isolation<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        self.isolation_installs.fetch_add(1, Ordering::SeqCst);
        let refused = self.refuse_isolation.load(Ordering::SeqCst);
        async move {
            if refused {
                Err("the isolation rules were refused".into())
            } else {
                Ok(())
            }
        }
        .boxed()
    }

    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }
}

/// Stub NAT64 translator: installs and removes nothing.
pub struct StubTranslator;

impl super::AddressTranslator for StubTranslator {
    fn set_up(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }

    fn tear_down(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }
}

/// Stub volume storage: plain directories, none of them snapshottable.
/// Confinement refuses a path whose `..` or symlinks would leave the root, as
/// a host's storage does, though it judges the path before writing rather than
/// as one step, and does not look for mount crossings.
pub struct StubVolumeStorage;

impl super::volume_store::VolumeStorage for StubVolumeStorage {
    fn snapshotting(&self) -> bool {
        false
    }

    fn create<'a>(&'a self, path: &'a std::path::Path) -> BoxFuture<'a, std::io::Result<()>> {
        async move {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::create_dir(path).await
        }
        .boxed()
    }

    fn remove<'a>(&'a self, path: &'a std::path::Path) -> BoxFuture<'a, std::io::Result<()>> {
        tokio::fs::remove_dir_all(path).boxed()
    }

    fn is_snapshottable<'a>(&'a self, _path: &'a std::path::Path) -> BoxFuture<'a, bool> {
        async { false }.boxed()
    }

    fn snapshot<'a>(
        &'a self,
        _source: &'a std::path::Path,
        _dest: &'a std::path::Path,
        _writable: bool,
    ) -> BoxFuture<'a, std::io::Result<()>> {
        async { Err(std::io::Error::other("stub volume storage cannot snapshot")) }.boxed()
    }

    fn write_confined<'a>(
        &'a self,
        root: &'a std::path::Path,
        rel_path: &'a str,
        contents: &'a [u8],
    ) -> BoxFuture<'a, Result<(), super::volume_store::ConfinedWriteError>> {
        use snafu::IntoError;
        use std::path::Component;

        use super::volume_store::{EscapeSnafu, IoSnafu};

        async move {
            let rel = rel_path.trim_start_matches('/');
            let mut depth = 0_usize;
            for component in std::path::Path::new(rel).components() {
                match component {
                    Component::Normal(_) => depth += 1,
                    Component::CurDir => {}
                    Component::ParentDir if depth > 0 => depth -= 1,
                    _ => {
                        return EscapeSnafu {
                            path: root.join(rel),
                        }
                        .fail();
                    }
                }
            }
            if depth == 0 {
                return EscapeSnafu {
                    path: root.join(rel),
                }
                .fail();
            }
            let path = root.join(rel);
            // A symlink along the path, or at its end, is followed to where it
            // leads, which must still be beneath the root.
            let escape = || EscapeSnafu { path: path.clone() }.build();
            let io = |e: std::io::Error| IoSnafu { path: path.clone() }.into_error(e);
            let canonical_root = tokio::fs::canonicalize(root).await.map_err(io)?;
            let mut existing = path.as_path();
            while tokio::fs::symlink_metadata(existing).await.is_err() {
                existing = existing.parent().ok_or_else(escape)?;
            }
            match tokio::fs::canonicalize(existing).await {
                Ok(resolved) if resolved.starts_with(&canonical_root) => {}
                Ok(_) => return Err(escape()),
                // A link whose target is gone leads nowhere the root holds.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(escape()),
                Err(e) => return Err(io(e)),
            }
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| IoSnafu { path: parent }.into_error(e))?;
            }
            tokio::fs::write(&path, contents)
                .await
                .map_err(|e| IoSnafu { path: &path }.into_error(e))
        }
        .boxed()
    }
}

/// Stub journal source: a journal with nothing in it.
pub struct StubJournal;

impl super::journal::JournalSource for StubJournal {
    fn open(
        &self,
        _opts: super::journal::LogStreamOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<super::journal::LogEntry>, BoxError> {
        let (_, rx) = tokio::sync::mpsc::channel(1);
        Ok(rx)
    }
}

#[cfg(all(test, unix))]
mod volume_storage_tests {
    use super::StubVolumeStorage;
    use crate::system::volume_store::{ConfinedWriteError, VolumeStorage};

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn a_symlink_leaving_the_root_is_refused_and_creates_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("data")).unwrap();

        let err = StubVolumeStorage
            .write_confined(dir.path(), "data/newdir/f.txt", b"evil")
            .await
            .unwrap_err();

        assert!(matches!(err, ConfinedWriteError::Escape { .. }), "{err}");
        assert!(!outside.path().join("newdir").exists());
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn a_symlinked_file_leaving_the_root_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("f.txt");
        std::fs::write(&target, b"theirs").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("f.txt")).unwrap();

        let err = StubVolumeStorage
            .write_confined(dir.path(), "f.txt", b"evil")
            .await
            .unwrap_err();

        assert!(matches!(err, ConfinedWriteError::Escape { .. }), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn a_symlink_within_the_root_and_dotdot_inside_it_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink("real", dir.path().join("link")).unwrap();

        StubVolumeStorage
            .write_confined(dir.path(), "link/f.txt", b"inside")
            .await
            .unwrap();
        StubVolumeStorage
            .write_confined(dir.path(), "sub/../sib/g.txt", b"inside")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(dir.path().join("real/f.txt")).unwrap(),
            b"inside"
        );
        assert_eq!(
            std::fs::read(dir.path().join("sib/g.txt")).unwrap(),
            b"inside"
        );
    }

    // spec: ACTU#storage-confinement
    #[tokio::test]
    async fn dotdot_leaving_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = StubVolumeStorage
            .write_confined(dir.path(), "sub/../../escape", b"evil")
            .await
            .unwrap_err();
        assert!(matches!(err, ConfinedWriteError::Escape { .. }), "{err}");
    }
}

#[cfg(test)]
mod image_argv_tests {
    use super::image_from_run_argv;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    // Taking the last element containing `/` or `:` picked up a command
    // argument whenever one had either, minting a phantom image for it.
    #[test]
    fn a_command_argument_is_not_mistaken_for_the_image() {
        let a = argv(&[
            "podman",
            "run",
            "--rm",
            "--log-driver=none",
            "ghcr.io/acme/app:1.2",
            "node",
            "server/index.js",
        ]);
        assert_eq!(
            image_from_run_argv(&a).as_deref(),
            Some("ghcr.io/acme/app:1.2")
        );
    }

    // Taking the first such element instead is no better: a volume mount
    // comes earlier and contains both characters.
    #[test]
    fn a_volume_mount_is_not_mistaken_for_the_image() {
        let a = argv(&[
            "podman",
            "run",
            "--rm",
            "-v",
            "/host/data:/data",
            "--entrypoint",
            "[]",
            "docker.io/library/nginx:latest",
            "sh",
            "-c",
            "exec nginx -g 'daemon off;'",
        ]);
        assert_eq!(
            image_from_run_argv(&a).as_deref(),
            Some("docker.io/library/nginx:latest")
        );
    }

    #[test]
    fn an_argv_with_no_image_yields_nothing() {
        assert_eq!(image_from_run_argv(&argv(&["podman", "run", "--rm"])), None);
    }
}
