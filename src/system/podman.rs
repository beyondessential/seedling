use std::{
    collections::HashMap,
    io,
    net::Ipv6Addr,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};

use chrono::DateTime;

use ipnet::Ipv6Net;
use podman_rest_client::{
    Config, PodmanRestClient,
    v5::{
        models::{NetworkCreateLibpod, Subnet, VolumeCreateOptions},
        params::{
            ContainerDeleteLibpod, ContainerListLibpod, ImagePullLibpod, NetworkDeleteLibpod,
            VolumeDeleteLibpod,
        },
    },
};

use snafu::Snafu;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd},
    process::Command,
};

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime,
    translate::container::podman_args,
    types::{
        ContainerFilter, ContainerHealth, ContainerSpec, ContainerState, ContainerStatus,
        ContainerSummary, ExecHandle, NetworkSummary,
    },
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum PodmanError {
    #[snafu(display("podman API error: {source}"))]
    Api {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("unexpected response from podman: {message}"))]
    Protocol { message: String },
    #[snafu(display("image pull failed: {message}"))]
    Pull { message: String },
}

// ---------------------------------------------------------------------------
// PodmanRuntime
// ---------------------------------------------------------------------------

pub(crate) struct PodmanRuntime {
    client: PodmanRestClient,
}

impl PodmanRuntime {
    pub(crate) async fn new() -> Result<Self, PodmanError> {
        let client = PodmanRestClient::new(Config {
            uri: "unix:///run/podman/podman.sock".to_string(),
            identity_file: None,
        })
        .await
        .map_err(|e| PodmanError::Api {
            source: Box::new(e),
        })?;

        Ok(Self { client })
    }

    async fn inspect_impl(&self, name: &str) -> Result<Option<ContainerState>, PodmanError> {
        let data = match self
            .client
            .v5()
            .containers()
            .container_inspect_libpod(name, None)
            .await
        {
            Ok(d) => d,
            Err(ref e) if is_not_found(e) => return Ok(None),
            Err(e) => return Err(map_api_err(e)),
        };

        let state = data.state.as_ref();

        let status = state
            .and_then(|s| s.status.as_deref())
            .map(parse_container_status)
            .unwrap_or(ContainerStatus::Unknown);

        let health = state
            .and_then(|s| s.health.as_ref())
            .and_then(|h| h.status.as_deref())
            .map(parse_health)
            .unwrap_or(ContainerHealth::None);

        let pid = state
            .and_then(|s| s.pid)
            .filter(|&p| p > 0)
            .map(|p| p as u32);

        let exit_code = state.and_then(|s| s.exit_code);

        let started_at = state
            .and_then(|s| s.started_at.as_deref())
            .and_then(parse_rfc3339);

        let finished_at = state
            .and_then(|s| s.finished_at.as_deref())
            .and_then(parse_rfc3339);

        let pod_addr = data
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.as_ref())
            .and_then(|nets| {
                nets.values().find_map(|n| {
                    n.global_i_pv6_address
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(|s| s.parse::<Ipv6Addr>().ok())
                })
            });

        let pod_addr_v4 = data
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.as_ref())
            .and_then(|nets| {
                nets.values().find_map(|n| {
                    n.ip_address
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok())
                })
            });

        Ok(Some(ContainerState {
            status,
            health,
            pid,
            exit_code,
            started_at,
            finished_at,
            pod_addr,
            pod_addr_v4,
            image_id: data.image.clone(),
        }))
    }

    async fn list_impl(
        &self,
        filter: ContainerFilter<'_>,
    ) -> Result<Vec<ContainerSummary>, PodmanError> {
        let filters_json = build_filters(&filter);
        let params = ContainerListLibpod {
            all: Some(true),
            filters: filters_json.as_deref(),
            ..Default::default()
        };

        let containers = self
            .client
            .v5()
            .containers()
            .container_list_libpod(Some(params))
            .await
            .map_err(map_api_err)?;

        let summaries = containers
            .into_iter()
            .map(|c| {
                let name = c
                    .names
                    .and_then(|ns| ns.into_iter().next())
                    .unwrap_or_default();
                let status = c
                    .state
                    .as_deref()
                    .map(parse_container_status)
                    .unwrap_or(ContainerStatus::Unknown);
                let labels = c.labels.unwrap_or_default();
                ContainerSummary {
                    name,
                    status,
                    labels,
                }
            })
            .collect();

        Ok(summaries)
    }

    async fn image_exists_impl(&self, reference: &str) -> Result<bool, PodmanError> {
        match self
            .client
            .v5()
            .images()
            .image_exists_libpod(reference)
            .await
        {
            Ok(()) => Ok(true),
            Err(ref e) if is_not_found(e) => Ok(false),
            Err(e) => Err(map_api_err(e)),
        }
    }

    #[tracing::instrument(skip_all, fields(%reference))]
    async fn pull_image_impl(&self, reference: &str) -> Result<(), PodmanError> {
        let params = ImagePullLibpod {
            reference: Some(reference),
            ..Default::default()
        };
        let report = self
            .client
            .v5()
            .images()
            .image_pull_libpod(Some(params))
            .await
            .map_err(map_api_err)?;
        if let Some(err) = report.error {
            return Err(PodmanError::Pull { message: err });
        }
        Ok(())
    }

    async fn network_exists_impl(&self, name: &str) -> Result<bool, PodmanError> {
        match self
            .client
            .v5()
            .networks()
            .network_exists_libpod(name)
            .await
        {
            Ok(()) => Ok(true),
            Err(ref e) if is_not_found(e) => Ok(false),
            Err(e) => Err(map_api_err(e)),
        }
    }

    // r[impl infra.pod.network]
    async fn create_network_impl(
        &self,
        name: &str,
        prefix: Ipv6Net,
        ipv4: Option<ipnet::Ipv4Net>,
    ) -> Result<String, PodmanError> {
        let net_addr = prefix.network();
        let mut gw_bytes = net_addr.octets();
        gw_bytes[15] = 1;
        let gateway = Ipv6Addr::from(gw_bytes).to_string();
        let subnet = prefix.to_string();

        // dual_stack must be checked before ipv4 is moved into the if-let below.
        let dual_stack = ipv4.is_some();

        let mut subnets = vec![Subnet {
            gateway: Some(gateway),
            subnet: Some(subnet),
            ..Default::default()
        }];

        if let Some(v4) = ipv4 {
            let mut gw4 = v4.network().octets();
            gw4[3] = 1;
            let gateway4 = std::net::Ipv4Addr::from(gw4).to_string();
            subnets.push(Subnet {
                gateway: Some(gateway4),
                subnet: Some(v4.to_string()),
                ..Default::default()
            });
        }

        // Setting ipv6_enabled is equivalent to `podman network create --ipv6`, which
        // enables dual-stack and causes Podman to auto-allocate an IPv4 subnet even when
        // only an IPv6 subnet is specified. For IPv6-only pod networks, omit the flag and
        // let the explicit IPv6 subnet alone define the network family.
        let body = NetworkCreateLibpod {
            name: Some(name.to_string()),
            driver: Some("bridge".to_string()),
            ipv6_enabled: if dual_stack { Some(true) } else { None },
            subnets: Some(subnets),
            ..Default::default()
        };

        // The creation response includes network_interface: the bridge name
        // assigned by netavark (e.g. "podman1" or "br-<id>").
        let created = self
            .client
            .v5()
            .networks()
            .network_create_libpod(body)
            .await
            .map_err(map_api_err)?;

        let bridge_name = created
            .network_interface
            .ok_or_else(|| PodmanError::Protocol {
                message: format!("network {name} was created but has no network_interface field"),
            })?;

        Ok(bridge_name)
    }

    async fn list_networks_impl(&self, prefix: &str) -> Result<Vec<NetworkSummary>, PodmanError> {
        let networks = self
            .client
            .v5()
            .networks()
            .network_list_libpod(None)
            .await
            .map_err(map_api_err)?;

        Ok(networks
            .into_iter()
            .filter_map(|n| {
                let name = n.name?;
                if !name.starts_with(prefix) {
                    return None;
                }
                let bridge_name = n.network_interface?;
                Some(NetworkSummary { name, bridge_name })
            })
            .collect())
    }

    async fn remove_network_impl(&self, name: &str) -> Result<(), PodmanError> {
        let params = NetworkDeleteLibpod { force: Some(false) };
        self.client
            .v5()
            .networks()
            .network_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn volume_exists_impl(&self, name: &str) -> Result<bool, PodmanError> {
        match self.client.v5().volumes().volume_exists_libpod(name).await {
            Ok(()) => Ok(true),
            Err(ref e) if is_not_found(e) => Ok(false),
            Err(e) => Err(map_api_err(e)),
        }
    }

    async fn create_volume_impl(&self, name: &str) -> Result<(), PodmanError> {
        let body = VolumeCreateOptions {
            name: Some(name.to_string()),
            ..Default::default()
        };
        self.client
            .v5()
            .volumes()
            .volume_create_libpod(body)
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn remove_volume_impl(&self, name: &str) -> Result<(), PodmanError> {
        let params = VolumeDeleteLibpod { force: Some(false) };
        self.client
            .v5()
            .volumes()
            .volume_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn volume_mountpoint_impl(&self, name: &str) -> Result<std::path::PathBuf, PodmanError> {
        let info = self
            .client
            .v5()
            .volumes()
            .volume_inspect_libpod(name)
            .await
            .map_err(map_api_err)?;
        let mountpoint = info.mountpoint.ok_or_else(|| PodmanError::Protocol {
            message: format!("volume '{name}' has no mountpoint"),
        })?;
        Ok(std::path::PathBuf::from(mountpoint))
    }

    async fn remove_container_impl(&self, name: &str, force: bool) -> Result<(), PodmanError> {
        let params = ContainerDeleteLibpod {
            force: Some(force),
            ..Default::default()
        };
        self.client
            .v5()
            .containers()
            .container_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn local_image_digest_impl(
        &self,
        reference: &str,
    ) -> Result<Option<String>, PodmanError> {
        match self
            .client
            .v5()
            .images()
            .image_inspect_libpod(reference)
            .await
        {
            Ok(data) => Ok(data.id),
            Err(ref e) if is_not_found(e) => Ok(None),
            Err(e) => Err(map_api_err(e)),
        }
    }

    async fn exec_impl(&self, spec: ContainerSpec) -> Result<ExecHandle, PodmanError> {
        let pty = nix::pty::openpty(None, None).map_err(|e| PodmanError::Protocol {
            message: e.to_string(),
        })?;

        let master_raw = pty.master.as_raw_fd();
        let pty_master_fd = master_raw;

        // Set O_NONBLOCK on the PTY master so that AsyncFd can drive it via
        // epoll rather than blocking threads.  A PTY fd supports epoll unlike
        // regular files, so this is the correct approach.
        unsafe {
            let flags = libc::fcntl(master_raw, libc::F_GETFL, 0);
            libc::fcntl(master_raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        // Dup the master so stdin and stdout each own an independent fd.
        // Both fds share the same open file description (including O_NONBLOCK).
        let master_read_fd = pty.master.into_raw_fd();
        let master_write_fd = unsafe { libc::dup(master_read_fd) };
        if master_write_fd < 0 {
            return Err(PodmanError::Protocol {
                message: io::Error::last_os_error().to_string(),
            });
        }

        let stdout =
            AsyncPtyHalf::new(unsafe { OwnedFd::from_raw_fd(master_read_fd) }).map_err(|e| {
                PodmanError::Protocol {
                    message: e.to_string(),
                }
            })?;
        let stdin =
            AsyncPtyHalf::new(unsafe { OwnedFd::from_raw_fd(master_write_fd) }).map_err(|e| {
                PodmanError::Protocol {
                    message: e.to_string(),
                }
            })?;

        let slave_raw = pty.slave.into_raw_fd();
        let slave_out = unsafe { libc::dup(slave_raw) };
        let slave_err = unsafe { libc::dup(slave_raw) };

        // Build argv from the standard translation pipeline, then insert -i -t
        // for interactive PTY use right after "podman run --rm".
        let mut argv = podman_args(&spec);
        // argv = ["podman", "run", "--rm", "--name", …]
        argv.insert(3, "-t".to_string());
        argv.insert(3, "-i".to_string());
        // argv = ["podman", "run", "--rm", "-i", "-t", "--name", …]

        let program = argv.remove(0);
        let mut cmd = Command::new(program);
        cmd.args(&argv);

        cmd.stdin(unsafe { std::process::Stdio::from_raw_fd(slave_raw) });
        cmd.stdout(unsafe { std::process::Stdio::from_raw_fd(slave_out) });
        cmd.stderr(unsafe { std::process::Stdio::from_raw_fd(slave_err) });

        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0i32);
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| PodmanError::Protocol {
            message: e.to_string(),
        })?;

        Ok(ExecHandle {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            pty_master_fd,
            child,
        })
    }
}

// ---------------------------------------------------------------------------
// AsyncPtyHalf — epoll-backed async read/write for a PTY master fd
// ---------------------------------------------------------------------------

/// Wraps a PTY master (or dup thereof) as a proper tokio async I/O type.
///
/// `tokio::fs::File` uses `spawn_blocking` for all I/O, assuming regular
/// files that don't support epoll.  PTY master fds *do* support epoll, so
/// `AsyncFd` is correct here: reads return `EAGAIN` when no data is available
/// and writes return `EAGAIN` when the kernel buffer is full, both of which
/// the epoll waker handles correctly.
struct AsyncPtyHalf(AsyncFd<OwnedFd>);

impl AsyncPtyHalf {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        Ok(Self(AsyncFd::new(fd)?))
    }
}

impl AsyncRead for AsyncPtyHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.0.poll_read_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            let result = guard.try_io(|inner| {
                let raw = inner.as_raw_fd();
                let dst = buf.initialize_unfilled();
                let n = unsafe { libc::read(raw, dst.as_mut_ptr().cast(), dst.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    buf.advance(n as usize);
                    Ok(())
                }
            });
            match result {
                Ok(r) => return Poll::Ready(r),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for AsyncPtyHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = match self.0.poll_write_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            let result = guard.try_io(|inner| {
                let raw = inner.as_raw_fd();
                let n = unsafe { libc::write(raw, buf.as_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match result {
                Ok(r) => return Poll::Ready(r),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_api_err(e: podman_rest_client::Error) -> PodmanError {
    PodmanError::Api {
        source: Box::new(e),
    }
}

fn is_not_found(e: &podman_rest_client::Error) -> bool {
    matches!(
        e,
        podman_rest_client::Error::Api { code, .. } if code.as_u16() == 404
    )
}

fn parse_container_status(s: &str) -> ContainerStatus {
    match s {
        "created" | "configured" => ContainerStatus::Created,
        "running" => ContainerStatus::Running,
        "paused" => ContainerStatus::Paused,
        "exited" | "stopped" | "dead" => ContainerStatus::Exited,
        _ => ContainerStatus::Unknown,
    }
}

fn parse_health(s: &str) -> ContainerHealth {
    match s {
        "starting" => ContainerHealth::Starting,
        "healthy" => ContainerHealth::Healthy,
        "unhealthy" => ContainerHealth::Unhealthy,
        _ => ContainerHealth::None,
    }
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    if s.is_empty() || s.starts_with("0001-01-01") {
        return None;
    }
    let dt = DateTime::parse_from_rfc3339(s).ok()?;
    let secs = dt.timestamp();
    if secs < 0 {
        return None;
    }
    SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::new(
        secs as u64,
        dt.timestamp_subsec_nanos(),
    ))
}

fn build_filters(filter: &ContainerFilter<'_>) -> Option<String> {
    let mut map: HashMap<&str, Vec<String>> = HashMap::new();
    if let Some((key, val)) = filter.label {
        map.insert("label", vec![format!("{}={}", key, val)]);
    }
    if let Some(prefix) = filter.name_prefix {
        map.insert("name", vec![format!("^{}", prefix)]);
    }
    if map.is_empty() {
        None
    } else {
        serde_json::to_string(&map).ok()
    }
}

// ---------------------------------------------------------------------------
// ContainerRuntime impl
// ---------------------------------------------------------------------------

impl ContainerRuntime for PodmanRuntime {
    fn inspect<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerState>, BoxError>> {
        Box::pin(async move { self.inspect_impl(name).await.map_err(Into::into) })
    }

    fn list<'a>(
        &'a self,
        filter: ContainerFilter<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContainerSummary>, BoxError>> {
        Box::pin(async move { self.list_impl(filter).await.map_err(Into::into) })
    }

    fn image_exists<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.image_exists_impl(reference).await.map_err(Into::into) })
    }

    fn pull_image<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.pull_image_impl(reference).await.map_err(Into::into) })
    }

    fn local_image_id<'a>(
        &'a self,
        reference: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
        Box::pin(async move {
            self.local_image_digest_impl(reference)
                .await
                .map_err(Into::into)
        })
    }

    fn network_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.network_exists_impl(name).await.map_err(Into::into) })
    }

    fn create_network<'a>(
        &'a self,
        name: &'a str,
        prefix: Ipv6Net,
        ipv4: Option<ipnet::Ipv4Net>,
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        Box::pin(async move {
            self.create_network_impl(name, prefix, ipv4)
                .await
                .map_err(Into::into)
        })
    }

    fn list_networks<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<NetworkSummary>, BoxError>> {
        Box::pin(async move { self.list_networks_impl(prefix).await.map_err(Into::into) })
    }

    fn remove_network<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.remove_network_impl(name).await.map_err(Into::into) })
    }

    fn volume_exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.volume_exists_impl(name).await.map_err(Into::into) })
    }

    fn create_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.create_volume_impl(name).await.map_err(Into::into) })
    }

    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.remove_volume_impl(name).await.map_err(Into::into) })
    }

    fn volume_mountpoint<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, BoxError>> {
        Box::pin(async move { self.volume_mountpoint_impl(name).await.map_err(Into::into) })
    }

    fn remove_container<'a>(
        &'a self,
        name: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            self.remove_container_impl(name, force)
                .await
                .map_err(Into::into)
        })
    }

    fn exec<'a>(&'a self, spec: ContainerSpec) -> BoxFuture<'a, Result<ExecHandle, BoxError>> {
        Box::pin(async move { self.exec_impl(spec).await.map_err(Into::into) })
    }
}
