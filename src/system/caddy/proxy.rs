use std::{net::SocketAddr, sync::Arc};

use reqwest::Client;
use serde_json::Value;
use snafu::Snafu;
use tokio::sync::RwLock;

use crate::system::{BoxError, BoxFuture, NetworkProxy, types::ProxyConfig};

// ---------------------------------------------------------------------------
// CaddyAddrs — returned by ensure_caddy_running
// ---------------------------------------------------------------------------

/// Addresses at which Caddy's admin API is reachable.
pub(crate) struct CaddyAddrs {
    pub v6: SocketAddr,
    pub v4: Option<SocketAddr>,
}

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum CaddyError {
    #[snafu(display("Caddy admin API returned HTTP {status}: {body}"))]
    Api { status: u16, body: String },
    #[snafu(display("HTTP request to Caddy admin API failed: {source}"))]
    Http { source: reqwest::Error },
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
/// The admin API is accessed at `http://[<caddy-ip>]:2019` on the
/// `seedling-proxy` network. The current admin address is stored in an
/// `Arc<tokio::sync::RwLock<SocketAddr>>` so it can be updated atomically
/// during a blue/green Caddy upgrade without restarting `CaddyProxy`.
pub(crate) struct CaddyProxy {
    admin_addr: Arc<RwLock<SocketAddr>>,
    client: Client,
}

impl CaddyProxy {
    /// Create a `CaddyProxy` pointed at the given Caddy admin API address.
    pub(crate) fn new(admin_addr: SocketAddr) -> Self {
        Self {
            admin_addr: Arc::new(RwLock::new(admin_addr)),
            client: Client::new(),
        }
    }

    /// Returns a handle to the shared admin address, so the caller can swap
    /// it atomically during a blue/green Caddy upgrade.
    pub(crate) fn admin_addr_handle(&self) -> Arc<RwLock<SocketAddr>> {
        Arc::clone(&self.admin_addr)
    }

    async fn admin_url(&self, path: &str) -> String {
        let addr = *self.admin_addr.read().await;
        // SocketAddr formats IPv6 addresses with brackets: [fd5e:ed...]:2019
        format!("http://{}{}", addr, path)
    }
}

impl CaddyProxy {
    async fn is_healthy_impl(&self) -> Result<bool, CaddyError> {
        let url = self.admin_url("/config/").await;
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    async fn apply_config_impl(&self, config: &ProxyConfig) -> Result<(), CaddyError> {
        let caddy_json = super::config::build_caddy_config(config);
        let url = self.admin_url("/config/").await;

        let resp = self
            .client
            .post(&url)
            .json(&caddy_json)
            .send()
            .await
            .map_err(|e| CaddyError::Http { source: e })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(CaddyError::Api { status, body });
        }

        Ok(())
    }

    pub(crate) async fn apply_raw_json(&self, json: &Value) -> Result<(), CaddyError> {
        let url = self.admin_url("/config/").await;
        let resp = self
            .client
            .post(&url)
            .json(json)
            .send()
            .await
            .map_err(|e| CaddyError::Http { source: e })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(CaddyError::Api { status, body });
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
