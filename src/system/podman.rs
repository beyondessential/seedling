use std::{collections::HashMap, net::Ipv6Addr, time::SystemTime};

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

use crate::system::{
    ContainerRuntime,
    types::{
        ContainerFilter, ContainerHealth, ContainerState, ContainerStatus, ContainerSummary,
        ExecHandle, ExecSpec,
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
    type Error = PodmanError;

    async fn inspect(&self, name: &str) -> Result<Option<ContainerState>, Self::Error> {
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
        }))
    }

    async fn list(
        &self,
        filter: ContainerFilter<'_>,
    ) -> Result<Vec<ContainerSummary>, Self::Error> {
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

    async fn image_exists(&self, reference: &str) -> Result<bool, Self::Error> {
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

    async fn pull_image(&self, reference: &str) -> Result<(), Self::Error> {
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

    async fn network_exists(&self, name: &str) -> Result<bool, Self::Error> {
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

    async fn create_network(&self, name: &str, prefix: Ipv6Net) -> Result<(), Self::Error> {
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

        self.client
            .v5()
            .networks()
            .network_create_libpod(body)
            .await
            .map_err(map_api_err)?;

        todo!("add ::2 mount endpoint to pod bridge")
    }

    async fn remove_network(&self, name: &str) -> Result<(), Self::Error> {
        let params = NetworkDeleteLibpod { force: Some(false) };
        self.client
            .v5()
            .networks()
            .network_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn volume_exists(&self, name: &str) -> Result<bool, Self::Error> {
        match self.client.v5().volumes().volume_exists_libpod(name).await {
            Ok(()) => Ok(true),
            Err(ref e) if is_not_found(e) => Ok(false),
            Err(e) => Err(map_api_err(e)),
        }
    }

    async fn create_volume(&self, name: &str) -> Result<(), Self::Error> {
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

    async fn remove_volume(&self, name: &str) -> Result<(), Self::Error> {
        let params = VolumeDeleteLibpod { force: Some(false) };
        self.client
            .v5()
            .volumes()
            .volume_delete_libpod(name, Some(params))
            .await
            .map_err(map_api_err)?;
        Ok(())
    }

    async fn remove_container(&self, name: &str, force: bool) -> Result<(), Self::Error> {
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

    async fn exec(&self, _name: &str, _spec: ExecSpec) -> Result<ExecHandle, Self::Error> {
        todo!("PodmanRuntime::exec")
    }
}
