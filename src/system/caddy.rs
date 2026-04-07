use snafu::Snafu;

use crate::system::{NetworkProxy, types::ProxyConfig};

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum CaddyError {
    #[snafu(display("Caddy admin API request failed: {message}"))]
    Api { message: String },
    #[snafu(display("Caddy is unreachable at {addr}: {source}"))]
    Unreachable {
        addr: std::net::SocketAddr,
        source: std::io::Error,
    },
    #[snafu(display("failed to serialize Caddy config: {source}"))]
    Serialize { source: serde_json::Error },
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
///
/// All methods are currently stubs; implement `is_healthy` and `apply_config`
/// first.
pub(crate) struct CaddyProxy {
    // TODO: add the following fields once `reqwest` (or `hyper`) is added as
    // a dependency for HTTP client use:
    //
    //   admin_addr: Arc<tokio::sync::RwLock<std::net::SocketAddr>>,
    //   client: reqwest::Client,
    _private: (),
}

impl CaddyProxy {
    /// Create a `CaddyProxy` pointed at the given Caddy admin API address.
    /// The address can be updated later (e.g. after a blue/green upgrade) via
    /// the `Arc<RwLock<SocketAddr>>` handle.
    pub(crate) fn new(_admin_addr: std::net::SocketAddr) -> Self {
        Self { _private: () }
    }
}

impl NetworkProxy for CaddyProxy {
    type Error = CaddyError;

    async fn is_healthy(&self) -> Result<bool, Self::Error> {
        todo!("CaddyProxy::is_healthy")
    }

    async fn apply_config(&self, _config: &ProxyConfig) -> Result<(), Self::Error> {
        todo!("CaddyProxy::apply_config")
    }
}
