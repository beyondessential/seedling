use std::{
    collections::HashMap,
    io,
    net::Ipv6Addr,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};

use ipnet::Ipv6Net;
use podman_rest_client::{
    Config, PodmanRestClient,
    v5::{
        models::{NetworkCreateLibpod, Subnet, VolumeCreateOptions},
        params::{
            ContainerDeleteLibpod, ContainerListLibpod, ImageDeleteLibpod, ImagePullLibpod,
            NetworkDeleteLibpod, VolumeDeleteLibpod,
        },
    },
};

use snafu::{IntoError, Snafu};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd},
    process::Command,
};

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime,
    translate::container::podman_args,
    types::{
        ContainerFilter, ContainerHealth, ContainerSpec, ContainerState, ContainerStatus,
        ContainerSummary, ExecHandle, ImageSummary, NetworkSummary,
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
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("unexpected response from podman: {message}"))]
    Protocol {
        message: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("image pull failed: {message}"))]
    Pull {
        message: String,
        backtrace: snafu::Backtrace,
    },
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
            backtrace: std::backtrace::Backtrace::capture(),
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
            spec_hash: data
                .config
                .as_ref()
                .and_then(|c| c.labels.as_ref())
                .and_then(|l| l.get("seedling.spec-hash"))
                .cloned(),
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
            return PullSnafu { message: err }.fail();
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

        let bridge_name = created.network_interface.ok_or_else(|| {
            ProtocolSnafu {
                message: format!("network {name} was created but has no network_interface field"),
            }
            .build()
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
        let reports = self
            .client
            .v5()
            .networks()
            .network_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        for r in &reports {
            if let Some(err) = &r.err {
                return ProtocolSnafu {
                    message: format!("network delete '{name}': {err}"),
                }
                .fail();
            }
        }
        Ok(())
    }

    async fn volume_exists_impl(&self, name: &str) -> Result<bool, PodmanError> {
        match self.client.v5().volumes().volume_exists_libpod(name).await {
            Ok(()) => Ok(true),
            Err(ref e) if is_not_found(e) => Ok(false),
            Err(e) => Err(map_api_err(e)),
        }
    }

    async fn create_volume_impl(&self, name: &str, tmpfs: bool) -> Result<(), PodmanError> {
        let body = VolumeCreateOptions {
            name: Some(name.to_string()),
            driver: if tmpfs {
                Some("local".to_string())
            } else {
                None
            },
            options: if tmpfs {
                Some(std::collections::HashMap::from([(
                    "type".to_string(),
                    "tmpfs".to_string(),
                )]))
            } else {
                None
            },
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
        let mountpoint = info.mountpoint.ok_or_else(|| {
            ProtocolSnafu {
                message: format!("volume '{name}' has no mountpoint"),
            }
            .build()
        })?;
        Ok(std::path::PathBuf::from(mountpoint))
    }

    async fn list_volumes_by_prefix_impl(&self, prefix: &str) -> Result<Vec<String>, PodmanError> {
        let volumes = self
            .client
            .v5()
            .volumes()
            .volume_list_libpod(None)
            .await
            .map_err(map_api_err)?;
        Ok(volumes
            .into_iter()
            .filter_map(|v| {
                let name = v.name?;
                if name.starts_with(prefix) {
                    Some(name)
                } else {
                    None
                }
            })
            .collect())
    }

    async fn remove_container_impl(&self, name: &str, force: bool) -> Result<(), PodmanError> {
        let params = ContainerDeleteLibpod {
            force: Some(force),
            ..Default::default()
        };
        let reports = self
            .client
            .v5()
            .containers()
            .container_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        for r in &reports {
            if let Some(err) = &r.err {
                return ProtocolSnafu {
                    message: format!("container delete '{name}': {err}"),
                }
                .fail();
            }
        }
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

    async fn list_images_impl(&self) -> Result<Vec<ImageSummary>, PodmanError> {
        let summaries = self
            .client
            .v5()
            .images()
            .image_list_libpod(None)
            .await
            .map_err(map_api_err)?;

        let mut out = Vec::with_capacity(summaries.len());
        for s in summaries {
            let image_id = match s.id {
                Some(id) if !id.is_empty() => id,
                _ => continue,
            };
            let mut references: Vec<String> = Vec::new();
            if let Some(tags) = s.repo_tags {
                references.extend(tags.into_iter().filter(|t| !t.is_empty()));
            }
            if let Some(digests) = s.repo_digests {
                references.extend(digests.into_iter().filter(|d| !d.is_empty()));
            }
            out.push(ImageSummary {
                image_id,
                references,
                size_bytes: s.size.unwrap_or(0),
                created_at_secs: s.created.unwrap_or(0),
            });
        }
        Ok(out)
    }

    async fn remove_image_impl(&self, reference: &str, force: bool) -> Result<bool, PodmanError> {
        let params = ImageDeleteLibpod { force: Some(force) };
        let report = match self
            .client
            .v5()
            .images()
            .image_delete_libpod(reference, Some(params))
            .await
        {
            Ok(r) => r,
            Err(ref e) if is_not_found(e) => return Ok(false),
            Err(e) => return Err(map_api_err(e)),
        };

        if let Some(errors) = report.errors
            && let Some(first) = errors.into_iter().next()
        {
            return ProtocolSnafu {
                message: format!("image delete '{reference}': {first}"),
            }
            .fail();
        }

        let deleted_any = report.deleted.as_ref().is_some_and(|v| !v.is_empty())
            || report.untagged.as_ref().is_some_and(|v| !v.is_empty());
        Ok(deleted_any)
    }

    async fn exec_impl(&self, spec: ContainerSpec) -> Result<ExecHandle, PodmanError> {
        let pty = nix::pty::openpty(None, None).map_err(|e| {
            ProtocolSnafu {
                message: e.to_string(),
            }
            .build()
        })?;

        let pty_master_fd = pty.master.as_raw_fd();

        // Set O_NONBLOCK on the PTY master so that AsyncFd can drive it via
        // epoll rather than blocking threads. A PTY fd supports epoll unlike
        // regular files, so this is the correct approach.
        unsafe {
            let flags = libc::fcntl(pty_master_fd, libc::F_GETFL, 0);
            libc::fcntl(pty_master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        // Dup the master so stdin and stdout each own an independent fd.
        // Both fds share the same open file description (including O_NONBLOCK).
        // We dup *before* consuming pty.master so OwnedFd RAII guards prevent
        // leaks on every error path.
        let master_write_raw = unsafe { libc::dup(pty.master.as_raw_fd()) };
        if master_write_raw < 0 {
            return ProtocolSnafu {
                message: io::Error::last_os_error().to_string(),
            }
            .fail();
        }
        // SAFETY: dup succeeded; wrap immediately so the fd is closed on drop.
        let master_write = unsafe { OwnedFd::from_raw_fd(master_write_raw) };

        let stdout = AsyncPtyHalf::new(pty.master).map_err(|e| {
            ProtocolSnafu {
                message: e.to_string(),
            }
            .build()
        })?;
        let stdin = AsyncPtyHalf::new(master_write).map_err(|e| {
            ProtocolSnafu {
                message: e.to_string(),
            }
            .build()
        })?;

        // Dup the slave fd for stdout and stderr so each Stdio owns an
        // independent descriptor. Wrap in OwnedFd immediately after each dup.
        let slave_out_raw = unsafe { libc::dup(pty.slave.as_raw_fd()) };
        if slave_out_raw < 0 {
            return ProtocolSnafu {
                message: io::Error::last_os_error().to_string(),
            }
            .fail();
        }
        let slave_out = unsafe { OwnedFd::from_raw_fd(slave_out_raw) };

        let slave_err_raw = unsafe { libc::dup(pty.slave.as_raw_fd()) };
        if slave_err_raw < 0 {
            return ProtocolSnafu {
                message: io::Error::last_os_error().to_string(),
            }
            .fail();
        }
        let slave_err = unsafe { OwnedFd::from_raw_fd(slave_err_raw) };

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

        cmd.stdin(std::process::Stdio::from(pty.slave));
        cmd.stdout(std::process::Stdio::from(slave_out));
        cmd.stderr(std::process::Stdio::from(slave_err));

        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0i32);
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| {
            ProtocolSnafu {
                message: e.to_string(),
            }
            .build()
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
    ApiSnafu.into_error(Box::new(e))
}

fn is_not_found(e: &podman_rest_client::Error) -> bool {
    match e {
        podman_rest_client::Error::Api { code, body } => {
            // Normal not-found.
            if code.as_u16() == 404 {
                return true;
            }
            // Podman can return 500 when a container disappears between the
            // name-lookup and the ID-based inspect (TOCTOU race with --rm).
            // Treat "no such container" 500s as not-found so callers see None.
            if code.as_u16() == 500 && body.to_string().contains("no such container") {
                return true;
            }
            false
        }
        _ => false,
    }
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
    let ts: jiff::Timestamp = s.parse().ok()?;
    if ts.as_second() < 0 {
        return None;
    }
    Some(SystemTime::from(ts))
}

fn build_filters(filter: &ContainerFilter<'_>) -> Option<String> {
    let mut map: HashMap<&str, Vec<String>> = HashMap::new();
    if let Some((key, val)) = filter.label {
        map.insert("label", vec![format!("{}={}", key, val)]);
    } else if let Some(key) = filter.label_key {
        map.insert("label", vec![key.to_string()]);
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

    fn list_images<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ImageSummary>, BoxError>> {
        Box::pin(async move { self.list_images_impl().await.map_err(Into::into) })
    }

    fn remove_image<'a>(
        &'a self,
        reference: &'a str,
        force: bool,
    ) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move {
            self.remove_image_impl(reference, force)
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

    fn create_volume<'a>(
        &'a self,
        name: &'a str,
        tmpfs: bool,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            self.create_volume_impl(name, tmpfs)
                .await
                .map_err(Into::into)
        })
    }

    fn remove_volume<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.remove_volume_impl(name).await.map_err(Into::into) })
    }

    fn list_volumes_by_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, BoxError>> {
        Box::pin(async move {
            self.list_volumes_by_prefix_impl(prefix)
                .await
                .map_err(Into::into)
        })
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
