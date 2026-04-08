use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    time::SystemTime,
};

use chrono::DateTime;
use futures_util::StreamExt;
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
use rtnetlink::{Handle, new_connection};
use snafu::Snafu;

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime,
    translate::proxy::pod_mount_addr,
    types::{
        ContainerFilter, ContainerHealth, ContainerState, ContainerStatus, ContainerSummary,
        ExecHandle, ExecSpec, NetworkSummary,
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
    #[snafu(display("rtnetlink error: {source}"))]
    Netlink {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("netlink connection error: {source}"))]
    Io { source: std::io::Error },
}

// ---------------------------------------------------------------------------
// PodmanRuntime
// ---------------------------------------------------------------------------

pub(crate) struct PodmanRuntime {
    client: PodmanRestClient,
    route_handle: Handle,
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

        let (connection, route_handle, _) =
            new_connection().map_err(|e| PodmanError::Io { source: e })?;
        tokio::spawn(connection);

        Ok(Self {
            client,
            route_handle,
        })
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

        Ok(Some(ContainerState {
            status,
            health,
            pid,
            exit_code,
            started_at,
            finished_at,
            pod_addr,
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
        self.client
            .v5()
            .images()
            .image_pull_libpod(Some(params))
            .await
            .map_err(map_api_err)?;
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

    async fn create_network_impl(
        &self,
        name: &str,
        prefix: Ipv6Net,
    ) -> Result<String, PodmanError> {
        let net_addr = prefix.network();
        let mut gw_bytes = net_addr.octets();
        gw_bytes[15] = 1;
        let gateway = Ipv6Addr::from(gw_bytes).to_string();
        let subnet = prefix.to_string();

        let body = NetworkCreateLibpod {
            name: Some(name.to_string()),
            driver: Some("bridge".to_string()),
            ipv6_enabled: Some(true),
            subnets: Some(vec![Subnet {
                gateway: Some(gateway),
                subnet: Some(subnet),
                ..Default::default()
            }]),
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

        // Resolve the bridge name to a kernel interface index.
        let mut links = self
            .route_handle
            .link()
            .get()
            .match_name(bridge_name.clone())
            .execute();
        let link = links
            .next()
            .await
            .ok_or_else(|| PodmanError::Protocol {
                message: format!("bridge interface {bridge_name} not found after network creation"),
            })?
            .map_err(|e| PodmanError::Netlink {
                source: Box::new(e),
            })?;
        let if_index = link.header.index;

        // Assign pod_mount_addr/64 to the bridge. This is the mount endpoint
        // address: pod containers resolve `localmount` to this address, and
        // the DataPlane's MountRule DNAT6 redirects that traffic to the target
        // service IP. The ::1 gateway is already assigned by netavark; Linux
        // supports multiple addresses per interface so there is no conflict.
        let endpoint = pod_mount_addr(&prefix);

        self.route_handle
            .address()
            .add(if_index, IpAddr::V6(endpoint), 64)
            .execute()
            .await
            .map_err(|e| PodmanError::Netlink {
                source: Box::new(e),
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

    async fn exec_impl(&self, _name: &str, _spec: ExecSpec) -> Result<ExecHandle, PodmanError> {
        todo!("PodmanRuntime::exec")
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
    ) -> BoxFuture<'a, Result<String, BoxError>> {
        Box::pin(async move {
            self.create_network_impl(name, prefix)
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

    fn exec<'a>(
        &'a self,
        name: &'a str,
        spec: ExecSpec,
    ) -> BoxFuture<'a, Result<ExecHandle, BoxError>> {
        Box::pin(async move { self.exec_impl(name, spec).await.map_err(Into::into) })
    }
}
