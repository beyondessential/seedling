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
    /// Container's IPv4 address on the proxy network.
    pub v4: Ipv4Addr,
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
    #[snafu(display(
        "address(es) {} are bound by more than one server; \
         Caddy cannot bind one address twice and would reject the whole config",
        binds.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    ))]
    ConflictingListeners {
        binds: Vec<super::config::Bind>,
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
    pub(crate) fn new(socket_path: &Path) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Arc::new(RwLock::new(build_client(socket_path)?)),
        })
    }

    /// Create a `CaddyProxy` with a placeholder client, used when the initial
    /// Caddy bring-up failed. Requests fail until the reconciler swaps in a
    /// real client (bound to the live admin socket) once `ensure_caddy_running`
    /// succeeds — see `Reconciler`'s `caddy_admin_client` handling.
    pub(crate) fn placeholder() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Arc::new(RwLock::new(reqwest::Client::builder().build()?)),
        })
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
pub(crate) fn build_client(socket_path: &Path) -> Result<reqwest::Client, reqwest::Error> {
    crate::runtime::local_socket::client_builder(socket_path).build()
}

impl CaddyProxy {
    async fn is_healthy_impl(&self) -> Result<bool, CaddyError> {
        let client = self.get_client().await;
        let resp = match client.get("http://localhost/config/").send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(false),
        };
        // A 200 with body `null` means the admin API is alive but Caddy has
        // no config loaded — it would route nothing. Treat as unhealthy so
        // ensure_caddy_running restarts it and the observer reports the
        // ingress as unreachable instead of falsely "ready". Caddy is always
        // started with the admin-binding config (`{"admin": {...}}`), so a
        // healthy fresh instance still returns a non-null body here.
        match resp.json::<Value>().await {
            Ok(Value::Null) => Ok(false),
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn apply_config_impl(&self, config: &ProxyConfig) -> Result<(), CaddyError> {
        // Refuse before sending. Caddy rejects a config where two servers
        // bind the same address, and `POST /config/` is all-or-nothing, so
        // pushing one would drop every route and cert policy on the node —
        // far past the ingress that caused it.
        // spec: PRX#shared-ports
        let rendered = super::config::render(config);
        let conflicts = rendered.conflicting_binds();
        if !conflicts.is_empty() {
            return ConflictingListenersSnafu { binds: conflicts }.fail();
        }

        let caddy_json = rendered.json;
        let client = self.get_client().await;

        // Dedup against Caddy's current config: when nothing has
        // changed (the steady state) the per-tick POST kicks Caddy's
        // reload machinery, churns its admin-API logs, and prints
        // "config applied" lines on our side too. Cheaper to GET once,
        // compare, and skip when identical. A failed GET (or a body
        // we can't parse) just falls through to POST so we never
        // skip a real apply by accident.
        if let Ok(resp) = client.get("http://localhost/config/").send().await
            && resp.status().is_success()
            && let Ok(current) = resp.json::<Value>().await
            && current == caddy_json
        {
            tracing::debug!("caddy config unchanged; skipping POST");
            return Ok(());
        }

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

    /// Read when the proxy last refused a session of each DTLS ingress.
    // spec: PRX#dtls-terminating-ingresses
    async fn dtls_refusals_impl(
        &self,
    ) -> Result<std::collections::BTreeMap<String, jiff::Timestamp>, CaddyError> {
        let client = self.get_client().await;
        let resp = client
            .get("http://localhost/seedling/dtls/refusals")
            .send()
            .await
            .context(HttpSnafu)?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return ApiSnafu { status, body }.fail();
        }
        let raw: std::collections::BTreeMap<String, String> =
            resp.json().await.context(HttpSnafu)?;
        parse_refusals(raw)
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

/// The refusals as the proxy reports them, each time an RFC 3339 timestamp. A
/// time that does not parse fails the read, as the whole answer is then not
/// known, rather than being dropped from it.
// spec: PRX#dtls-terminating-ingresses
fn parse_refusals(
    raw: std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, jiff::Timestamp>, CaddyError> {
    raw.into_iter()
        .map(|(ingress, at)| match at.parse::<jiff::Timestamp>() {
            Ok(at) => Ok((ingress, at)),
            Err(e) => ApiSnafu {
                status: 200u16,
                body: format!("refusal time {at:?} for {ingress}: {e}"),
            }
            .fail(),
        })
        .collect()
}

impl NetworkProxy for CaddyProxy {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.is_healthy_impl().await.map_err(Into::into) })
    }

    fn apply_config<'a>(&'a self, config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_config_impl(config).await.map_err(Into::into) })
    }

    fn dtls_refusals<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<std::collections::BTreeMap<String, jiff::Timestamp>, BoxError>> {
        Box::pin(async move { self.dtls_refusals_impl().await.map_err(Into::into) })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_refusals;

    // spec: PRX#dtls-terminating-ingresses
    #[test]
    fn refusal_times_are_read_as_reported() {
        let raw = [(
            "voip.example.com:5684".to_owned(),
            "2026-11-18T01:02:03.5Z".to_owned(),
        )]
        .into();
        let read = parse_refusals(raw).expect("parses");
        assert_eq!(
            read["voip.example.com:5684"].to_string(),
            "2026-11-18T01:02:03.5Z"
        );
    }

    // spec: PRX#dtls-terminating-ingresses
    #[test]
    fn an_unreadable_time_fails_the_whole_read() {
        let raw = [
            ("a:1".to_owned(), "2026-11-18T01:02:03Z".to_owned()),
            ("b:2".to_owned(), "yesterday".to_owned()),
        ]
        .into();
        assert!(parse_refusals(raw).is_err());
    }
}
