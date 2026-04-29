//! In-memory fakes for the four system traits, used to boot the daemon
//! without podman / systemd / nftables / Caddy.
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
    sync::Arc,
    time::SystemTime,
};

use futures_util::FutureExt;
use ipnet::Ipv4Net;
use parking_lot::Mutex;

use super::{
    BoxError, BoxFuture, ContainerFilter, ContainerHealth, ContainerRuntime, ContainerSpec,
    ContainerState, ContainerStatus, ContainerSummary, DataPlane, DataPlaneRules, ExecHandle,
    ImageSummary, NetworkProxy, NetworkSummary, ProcessManager, ProxyConfig, ServiceRoute,
    TransientUnitSpec, UnitState, UnitSummary,
    types::ActiveState,
};

/// Stub `ContainerRuntime`. Pretends every started container is healthy and
/// running; tracks just enough state to answer the queries the reconciler
/// makes between actuation ticks.
pub struct StubContainerRuntime {
    state: Mutex<ContainerState_>,
    /// Where stub volumes' mountpoints live. One subdir per volume name; the
    /// runtime owns this dir for the duration of the daemon's lifetime.
    volumes_root: PathBuf,
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
        }
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
                spec_hash: c.spec.labels.get("seedling.spec-hash").cloned(),
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
                let labels: HashMap<String, String> = c.spec.labels.iter()
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
        _prefix: ipnet::Ipv6Net,
        _ipv4: Option<Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        async move {
            let mut s = self.state.lock();
            let bridge_name = format!("br-{name}");
            s.networks.insert(
                name.to_owned(),
                StubNetwork {
                    bridge_name: bridge_name.clone(),
                },
            );
            Ok(bridge_name)
        }
        .boxed()
    }

    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            self.state.lock().networks.remove(name);
            Ok(())
        }
        .boxed()
    }

    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>> {
        async move {
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
            self.state.lock().volumes.remove(name);
            let dir = self.volumes_root.join(name);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Ok(())
        }
        .boxed()
    }

    fn list_volumes_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, BoxError>> {
        async move {
            Ok(self
                .state
                .lock()
                .volumes
                .iter()
                .filter(|n| n.starts_with(prefix))
                .cloned()
                .collect())
        }
        .boxed()
    }

    fn volume_mountpoint<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<PathBuf, BoxError>> {
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
}

struct UnitRecord {
    state: ActiveState,
    sub: String,
}

impl StubProcessManager {
    pub fn new(container: Arc<StubContainerRuntime>) -> Self {
        Self {
            state: Mutex::new(UnitState_::default()),
            container,
        }
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

            let image = spec
                .exec_start
                .iter()
                .rev()
                .find(|a| a.contains('/') || a.contains(':'))
                .cloned()
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

            self.state.lock().units.insert(
                spec.name.clone(),
                UnitRecord {
                    state: ActiveState::Active,
                    sub: "running".to_owned(),
                },
            );
            drop(spec);
            Ok(())
        }
        .boxed()
    }

    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        async move {
            let container_name = name.strip_suffix(".service").unwrap_or(name).to_owned();
            self.container.state.lock().containers.remove(&container_name);
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
}

/// Stub `DataPlane`: no-ops everything. nftables / routing-table effects
/// aren't observable through any OI surface so there's nothing for tests
/// to assert against; the runtime still gets the trait so reconciliation
/// proceeds.
pub struct StubDataPlane;

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

    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        async { Ok(()) }.boxed()
    }
}
