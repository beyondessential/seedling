use std::{
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_json::Value;
use snafu::{ResultExt, Snafu};
use tokio::sync::RwLock;

use crate::system::{BoxError, BoxFuture, NetworkProxy, types::ProxyConfig};

// ---------------------------------------------------------------------------
// CaddyAddrs — returned by ensure_caddy_running
// ---------------------------------------------------------------------------

/// Addresses and socket path describing a running Caddy instance.
pub(crate) struct CaddyAddrs {
    /// Container's IPv6 address on the proxy network (used for nftables rules).
    pub v6: Ipv6Addr,
    /// Container's IPv4 address on the proxy network, if present.
    pub v4: Option<Ipv4Addr>,
    /// Host-side path of the Unix socket for the Caddy admin API.
    pub admin_socket: PathBuf,
}

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum CaddyError {
    #[snafu(display("Caddy admin API returned HTTP {status}: {body}"))]
    Api {
        status: u16,
        body: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("HTTP request to Caddy admin API failed: {source}"))]
    Http {
        source: reqwest::Error,
        backtrace: snafu::Backtrace,
    },
}

// ---------------------------------------------------------------------------
// CaddyProxy
// ---------------------------------------------------------------------------

/// `NetworkProxy` implementation that drives Caddy via its JSON admin API
/// (`POST /config/`).
///
/// Caddy is managed out of band as infrastructure: it is not tracked in
/// `resource_instances` and does not go through the normal `Actuator`
/// start/stop path. Seedling starts it at startup and manages it directly.
///
/// The admin API is reached over a Unix domain socket bind-mounted into the
/// Caddy container. The active `reqwest::Client` is held behind an
/// `Arc<RwLock<…>>` so it can be swapped atomically during a blue/green
/// upgrade without restarting `CaddyProxy`.
pub(crate) struct CaddyProxy {
    client: Arc<RwLock<reqwest::Client>>,
}

impl CaddyProxy {
    /// Create a `CaddyProxy` that connects via the given Unix socket path.
    pub(crate) fn new(socket_path: &Path) -> Self {
        Self {
            client: Arc::new(RwLock::new(build_client(socket_path))),
        }
    }

    /// Returns a handle to the shared client, so the caller can swap it
    /// atomically during a blue/green Caddy upgrade.
    pub(crate) fn admin_client_handle(&self) -> Arc<RwLock<reqwest::Client>> {
        Arc::clone(&self.client)
    }

    /// Clone the current client out of the lock for use in a single request.
    ///
    /// `reqwest::Client` is cheaply clonable (Arc-backed), so this does not
    /// duplicate any connection pool state.
    async fn get_client(&self) -> reqwest::Client {
        self.client.read().await.clone()
    }
}

/// Build a `reqwest::Client` whose every connection goes through `socket_path`.
pub(crate) fn build_client(socket_path: &Path) -> reqwest::Client {
    reqwest::Client::builder()
        .unix_socket(socket_path)
        .build()
        .expect("build reqwest client for caddy admin socket")
}

impl CaddyProxy {
    async fn is_healthy_impl(&self) -> Result<bool, CaddyError> {
        let client = self.get_client().await;
        match client.get("http://localhost/config/").send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn apply_config_impl(&self, config: &ProxyConfig) -> Result<(), CaddyError> {
        let caddy_json = super::config::build_caddy_config(config);
        let client = self.get_client().await;

        let resp = client
            .post("http://localhost/config/")
            .json(&caddy_json)
            .send()
            .await
            .context(HttpSnafu)?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return ApiSnafu { status, body }.fail();
        }

        Ok(())
    }

    pub(crate) async fn apply_raw_json(&self, json: &Value) -> Result<(), CaddyError> {
        let client = self.get_client().await;
        let resp = client
            .post("http://localhost/config/")
            .json(json)
            .send()
            .await
            .context(HttpSnafu)?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return ApiSnafu { status, body }.fail();
        }
        Ok(())
    }
}

impl NetworkProxy for CaddyProxy {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.is_healthy_impl().await.map_err(Into::into) })
    }

    fn apply_config<'a>(&'a self, config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_config_impl(config).await.map_err(Into::into) })
    }
}
