//! The container runtime and process supervisor, which the daemon may start
//! without and must go on probing for.
//!
//! Each backend is held behind a [`Switchable`], so what the rest of the
//! daemon was handed at startup becomes the working backend once a probe
//! reaches it, with no restart.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::FutureExt;
use ipnet::{Ipv4Net, Ipv6Net};
use parking_lot::{Mutex, RwLock};

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime, ProcessManager,
    types::{
        ContainerFilter, ContainerSpec, ContainerState, ContainerSummary, ExecHandle, ExecOutput,
        ImageSummary, NetworkSummary, SliceSpec, TransientUnitSpec, UnitState, UnitSummary,
    },
};

/// The first delay between probes of an unavailable backend.
const FIRST_PROBE_DELAY: Duration = Duration::from_secs(5);

/// The longest delay between probes. Probing continues at this spacing for as
/// long as a backend stays unavailable.
const MAX_PROBE_DELAY: Duration = Duration::from_secs(300);

/// A backend that can be replaced while the daemon runs.
pub(crate) struct Switchable<T: ?Sized> {
    current: RwLock<Arc<T>>,
}

impl<T: ?Sized> Switchable<T> {
    pub(crate) fn new(initial: Arc<T>) -> Self {
        Self {
            current: RwLock::new(initial),
        }
    }

    fn get(&self) -> Arc<T> {
        Arc::clone(&self.current.read())
    }

    fn replace(&self, next: Arc<T>) {
        *self.current.write() = next;
    }
}

impl ContainerRuntime for Switchable<dyn ContainerRuntime> {
    fn inspect<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerState>, BoxError>> {
        let inner = self.get();
        async move { inner.inspect(name).await }.boxed()
    }

    fn list<'a>(
        &'a self,
        filter: ContainerFilter<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContainerSummary>, BoxError>> {
        let inner = self.get();
        async move { inner.list(filter).await }.boxed()
    }

    fn image_exists<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        let inner = self.get();
        async move { inner.image_exists(reference).await }.boxed()
    }

    fn pull_image<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.pull_image(reference).await }.boxed()
    }

    fn local_image_id<'a>(
        &'a self,
        reference: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
        let inner = self.get();
        async move { inner.local_image_id(reference).await }.boxed()
    }

    fn list_images<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ImageSummary>, BoxError>> {
        let inner = self.get();
        async move { inner.list_images().await }.boxed()
    }

    fn remove_image<'a>(
        &'a self,
        reference: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        let inner = self.get();
        async move { inner.remove_image(reference, force).await }.boxed()
    }

    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        let inner = self.get();
        async move { inner.network_exists(name).await }.boxed()
    }

    fn create_network<'a>(
        &'a self,
        name: &'a str,
        prefix: Ipv6Net,
        ipv4: Option<Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        let inner = self.get();
        async move { inner.create_network(name, prefix, ipv4).await }.boxed()
    }

    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.remove_network(name).await }.boxed()
    }

    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>> {
        let inner = self.get();
        async move { inner.list_networks(prefix).await }.boxed()
    }

    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        let inner = self.get();
        async move { inner.volume_exists(name).await }.boxed()
    }

    fn create_volume<'a>(
        &'a self,
        name: &'a str,
        tmpfs: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.create_volume(name, tmpfs).await }.boxed()
    }

    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.remove_volume(name).await }.boxed()
    }

    fn list_volumes_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, BoxError>> {
        let inner = self.get();
        async move { inner.list_volumes_by_prefix(prefix).await }.boxed()
    }

    fn volume_mountpoint<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, BoxError>> {
        let inner = self.get();
        async move { inner.volume_mountpoint(name).await }.boxed()
    }

    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.remove_container(name, force).await }.boxed()
    }

    fn exec<'a>(&'a self, spec: ContainerSpec) -> BoxFuture<'a, Result<ExecHandle, BoxError>> {
        let inner = self.get();
        async move { inner.exec(spec).await }.boxed()
    }

    fn signal_container<'a>(
        &'a self,
        name: &'a str,
        signal: &'a str,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        let inner = self.get();
        async move { inner.signal_container(name, signal).await }.boxed()
    }

    fn exec_command<'a>(
        &'a self,
        name: &'a str,
        argv: &'a [String],
        extra_env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<ExecOutput, BoxError>> {
        let inner = self.get();
        async move { inner.exec_command(name, argv, extra_env).await }.boxed()
    }
}

impl ProcessManager for Switchable<dyn ProcessManager> {
    fn start_transient<'a>(
        &'a self,
        spec: TransientUnitSpec,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.start_transient(spec).await }.boxed()
    }

    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.stop_unit(name).await }.boxed()
    }

    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.reset_failed_unit(name).await }.boxed()
    }

    fn unit_state<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<UnitState>, BoxError>> {
        let inner = self.get();
        async move { inner.unit_state(name).await }.boxed()
    }

    fn list_units<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<UnitSummary>, BoxError>> {
        let inner = self.get();
        async move { inner.list_units(prefix).await }.boxed()
    }

    fn sync_slices<'a>(
        &'a self,
        desired: Vec<SliceSpec>,
        prune: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.sync_slices(desired, prune).await }.boxed()
    }

    fn write_unit<'a>(
        &'a self,
        name: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.write_unit(name, content).await }.boxed()
    }

    fn remove_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.remove_unit(name).await }.boxed()
    }

    fn daemon_reload<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.daemon_reload().await }.boxed()
    }

    fn start_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        let inner = self.get();
        async move { inner.start_unit(name).await }.boxed()
    }
}

/// How the daemon reaches its backends: the host's podman and systemd, or
/// whatever a test stands in.
pub(crate) trait Connector: Send + Sync + 'static {
    fn container(&self) -> BoxFuture<'_, Result<Arc<dyn ContainerRuntime>, BoxError>>;
    fn process(&self) -> BoxFuture<'_, Result<Arc<dyn ProcessManager>, BoxError>>;
}

/// Connects to the host's podman and systemd.
pub(crate) struct HostConnector;

impl Connector for HostConnector {
    fn container(&self) -> BoxFuture<'_, Result<Arc<dyn ContainerRuntime>, BoxError>> {
        async {
            let podman = crate::system::podman::PodmanRuntime::new().await?;
            Ok(Arc::new(podman) as Arc<dyn ContainerRuntime>)
        }
        .boxed()
    }

    fn process(&self) -> BoxFuture<'_, Result<Arc<dyn ProcessManager>, BoxError>> {
        async {
            let systemd = crate::system::systemd::SystemdManager::connect().await?;
            Ok(Arc::new(systemd) as Arc<dyn ProcessManager>)
        }
        .boxed()
    }
}

/// Which backends are unreachable, and when to look again.
struct Standing {
    /// Why the container runtime is unavailable, while it is.
    container: Option<String>,
    /// Why the process supervisor is unavailable, while it is.
    process: Option<String>,
    /// The delay after the next failed round of probes.
    delay: Duration,
    /// The earliest the next round of probes may run.
    next_probe: Instant,
}

/// The container runtime and process supervisor the daemon works through, and
/// whether each could be reached.
// spec: INFRA#unavailable-backends
pub struct Backends {
    container_switch: Arc<Switchable<dyn ContainerRuntime>>,
    process_switch: Arc<Switchable<dyn ProcessManager>>,
    connector: Arc<dyn Connector>,
    standing: Mutex<Standing>,
}

impl Backends {
    /// Backends that start as `container` and `process`, each failed or
    /// working, and are reached through `connector` thereafter.
    pub(crate) fn new(
        connector: Arc<dyn Connector>,
        container: Result<Arc<dyn ContainerRuntime>, BoxError>,
        process: Result<Arc<dyn ProcessManager>, BoxError>,
    ) -> Self {
        let (container, container_down) = match container {
            Ok(c) => (c, None),
            Err(e) => (
                Arc::new(crate::system::unavailable::UnavailableContainerRuntime)
                    as Arc<dyn ContainerRuntime>,
                Some(format!("container runtime (podman) unavailable: {e}")),
            ),
        };
        let (process, process_down) = match process {
            Ok(p) => (p, None),
            Err(e) => (
                Arc::new(crate::system::unavailable::UnavailableProcessManager)
                    as Arc<dyn ProcessManager>,
                Some(format!("process manager (systemd) unavailable: {e}")),
            ),
        };
        Self {
            container_switch: Arc::new(Switchable::new(container)),
            process_switch: Arc::new(Switchable::new(process)),
            connector,
            standing: Mutex::new(Standing {
                container: container_down,
                process: process_down,
                delay: FIRST_PROBE_DELAY,
                next_probe: Instant::now() + FIRST_PROBE_DELAY,
            }),
        }
    }

    /// Backends that were reached and stay so, as stubs do.
    pub(crate) fn working(
        container: Arc<dyn ContainerRuntime>,
        process: Arc<dyn ProcessManager>,
    ) -> Self {
        Self::new(Arc::new(HostConnector), Ok(container), Ok(process))
    }

    /// Backends that could not be reached, with no one to reach them again.
    pub(crate) fn unreachable(reason: &str) -> Self {
        struct Nowhere;
        impl Connector for Nowhere {
            fn container(&self) -> BoxFuture<'_, Result<Arc<dyn ContainerRuntime>, BoxError>> {
                async { Err("no container runtime".into()) }.boxed()
            }
            fn process(&self) -> BoxFuture<'_, Result<Arc<dyn ProcessManager>, BoxError>> {
                async { Err("no process manager".into()) }.boxed()
            }
        }
        Self::new(Arc::new(Nowhere), Err(reason.into()), Err(reason.into()))
    }

    pub(crate) fn container(&self) -> Arc<dyn ContainerRuntime> {
        Arc::clone(&self.container_switch) as _
    }

    pub(crate) fn process(&self) -> Arc<dyn ProcessManager> {
        Arc::clone(&self.process_switch) as _
    }

    /// Why a backend is unavailable, if one is.
    pub fn unavailable_reason(&self) -> Option<String> {
        let standing = self.standing.lock();
        match (&standing.container, &standing.process) {
            (None, None) => None,
            (Some(c), None) => Some(c.clone()),
            (None, Some(p)) => Some(p.clone()),
            (Some(c), Some(p)) => Some(format!("{c}; {p}")),
        }
    }

    /// Probe each unavailable backend again, if the spacing since the last
    /// round allows, and say why a backend is still unavailable, if one is.
    ///
    /// Each failed round doubles the spacing up to a fixed maximum, at which
    /// it stays, so a backend is probed for as long as it is down.
    pub async fn probe_unavailable(&self) -> Option<String> {
        let (container_down, process_down) = {
            let standing = self.standing.lock();
            if standing.container.is_none() && standing.process.is_none() {
                return None;
            }
            if Instant::now() < standing.next_probe {
                drop(standing);
                return self.unavailable_reason();
            }
            (standing.container.is_some(), standing.process.is_some())
        };

        let mut container = None;
        let mut container_failure = None;
        if container_down {
            match self.connector.container().await {
                Ok(c) => container = Some(c),
                Err(e) => {
                    container_failure = Some(format!("container runtime (podman) unavailable: {e}"))
                }
            }
        }
        let mut process = None;
        let mut process_failure = None;
        if process_down {
            match self.connector.process().await {
                Ok(p) => process = Some(p),
                Err(e) => {
                    process_failure = Some(format!("process manager (systemd) unavailable: {e}"))
                }
            }
        }

        {
            let mut standing = self.standing.lock();
            if let Some(c) = container {
                self.container_switch.replace(c);
                standing.container = None;
                tracing::info!("the container runtime is reachable again");
            } else if container_failure.is_some() {
                standing.container = container_failure;
            }
            if let Some(p) = process {
                self.process_switch.replace(p);
                standing.process = None;
                tracing::info!("the process manager is reachable again");
            } else if process_failure.is_some() {
                standing.process = process_failure;
            }
            standing.next_probe = Instant::now() + standing.delay;
            standing.delay = (standing.delay * 2).min(MAX_PROBE_DELAY);
        }
        self.unavailable_reason()
    }

    /// Let the next probe run at once, whatever the spacing.
    #[cfg(test)]
    pub(crate) fn probe_now(&self) {
        self.standing.lock().next_probe = Instant::now();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::system::stub::{StubContainerRuntime, StubProcessManager};

    /// A connector whose backends come up when told to, counting each attempt.
    pub(crate) struct Flaky {
        pub up: AtomicBool,
        pub attempts: AtomicUsize,
        container: Arc<StubContainerRuntime>,
        _dir: tempfile::TempDir,
    }

    impl Flaky {
        pub(crate) fn down() -> Arc<Self> {
            let dir = tempfile::tempdir().unwrap();
            Arc::new(Self {
                up: AtomicBool::new(false),
                attempts: AtomicUsize::new(0),
                container: Arc::new(StubContainerRuntime::new(dir.path().to_owned())),
                _dir: dir,
            })
        }
    }

    impl Connector for Flaky {
        fn container(&self) -> BoxFuture<'_, Result<Arc<dyn ContainerRuntime>, BoxError>> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let up = self.up.load(Ordering::SeqCst);
            async move {
                if up {
                    Ok(Arc::clone(&self.container) as Arc<dyn ContainerRuntime>)
                } else {
                    Err("podman socket missing".into())
                }
            }
            .boxed()
        }

        fn process(&self) -> BoxFuture<'_, Result<Arc<dyn ProcessManager>, BoxError>> {
            let up = self.up.load(Ordering::SeqCst);
            async move {
                if up {
                    Ok(
                        Arc::new(StubProcessManager::new(Arc::clone(&self.container)))
                            as Arc<dyn ProcessManager>,
                    )
                } else {
                    Err("systemd bus missing".into())
                }
            }
            .boxed()
        }
    }

    pub(crate) fn down_backends(flaky: &Arc<Flaky>) -> Backends {
        Backends::new(
            Arc::clone(flaky) as Arc<dyn Connector>,
            Err("podman socket missing".into()),
            Err("systemd bus missing".into()),
        )
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    // spec: INFRA#unavailable-backends
    #[test]
    fn a_backend_that_becomes_reachable_replaces_the_stub_without_a_restart() {
        let flaky = Flaky::down();
        let backends = down_backends(&flaky);
        let container = backends.container();
        assert!(backends.unavailable_reason().is_some());
        assert!(block_on(container.list(ContainerFilter::default())).is_err());

        flaky.up.store(true, Ordering::SeqCst);
        backends.probe_now();
        assert_eq!(block_on(backends.probe_unavailable()), None);

        // The handle handed out before the probe is the working backend now.
        assert!(block_on(container.list(ContainerFilter::default())).is_ok());
        assert_eq!(backends.unavailable_reason(), None);
    }

    // spec: INFRA#unavailable-backends
    #[test]
    fn probes_are_spaced_and_the_spacing_grows_to_a_cap() {
        let flaky = Flaky::down();
        let backends = down_backends(&flaky);

        // Not due yet: no attempt is made.
        assert!(block_on(backends.probe_unavailable()).is_some());
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 0);

        let mut last = Duration::ZERO;
        for _ in 0..12 {
            backends.probe_now();
            assert!(block_on(backends.probe_unavailable()).is_some());
            let delay = backends.standing.lock().delay;
            assert!(delay >= last, "the spacing never shrinks while down");
            assert!(delay <= MAX_PROBE_DELAY);
            last = delay;
        }
        assert_eq!(last, MAX_PROBE_DELAY, "probing goes on at the cap");
        assert_eq!(flaky.attempts.load(Ordering::SeqCst), 12);
    }
}
