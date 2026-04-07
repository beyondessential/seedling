use std::{net::SocketAddr, sync::Arc};

use ipnet::Ipv6Net;
use reqwest::Client;
use serde_json::{Value, json};
use snafu::Snafu;
use tokio::sync::RwLock;

use crate::system::{
    BoxError, BoxFuture, ContainerRuntime, NetworkProxy, ProcessManager,
    types::{ProxyConfig, ProxyListenerProto, VirtualHost},
};

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
        let caddy_json = build_caddy_config(config);
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
}

impl NetworkProxy for CaddyProxy {
    fn is_healthy<'a>(&'a self) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(async move { self.is_healthy_impl().await.map_err(Into::into) })
    }

    fn apply_config<'a>(&'a self, config: &'a ProxyConfig) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_config_impl(config).await.map_err(Into::into) })
    }
}

// ---------------------------------------------------------------------------
// Startup constants
// ---------------------------------------------------------------------------

pub(crate) const CADDY_CONTAINER: &str = "seedling-caddy";
pub(crate) const CADDY_UNIT: &str = "seedling-caddy.service";
pub(crate) const CADDY_IMAGE: &str = "docker.io/library/caddy:2.11.2";
pub(crate) const CADDY_DATA_VOLUME: &str = "seedling-caddy-data";
pub(crate) const PROXY_NETWORK: &str = "seedling-proxy";
/// Minimal Caddy JSON config that binds the admin API on all interfaces.
const CADDY_ADMIN_JSON: &str = r#"{"admin":{"listen":":2019"}}"#;

// ---------------------------------------------------------------------------
// Startup helpers
// ---------------------------------------------------------------------------

/// Returns the /64 infrastructure prefix for the seedling-proxy network.
///
/// The network sits at `fd5e:edXX:XXXX:ff00::/64` within the node's /48,
/// using `0xff` as the subnet discriminant (above the resource-kind range 0–9).
// r[impl infra.proxy.startup]
pub(crate) fn proxy_network_prefix(node_prefix: &Ipv6Net) -> Ipv6Net {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xff;
    Ipv6Net::new(std::net::Ipv6Addr::from(addr), 64).expect("64 is a valid IPv6 prefix length")
}

#[derive(Debug, Snafu)]
pub(crate) enum CaddyStartupError {
    #[snafu(display("container runtime error: {source}"))]
    Container {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("process manager error: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("I/O error writing admin config: {source}"))]
    Io { source: std::io::Error },
    #[snafu(display("Caddy did not become healthy within the timeout"))]
    Timeout,
}

// r[impl infra.proxy.startup]
/// Ensure the Caddy proxy container is running and healthy.
///
/// 1. Creates the `seedling-proxy` network if absent.
/// 2. Writes a minimal admin-API config to `{data_dir}/caddy-admin.json`.
/// 3. Creates the `seedling-caddy-data` volume if absent.
/// 4. If Caddy is already running and healthy, returns its admin `SocketAddr`.
/// 5. Otherwise force-removes any existing (unhealthy) container, pulls the
///    image if necessary, starts a transient systemd unit, and polls until
///    healthy (up to 60 s).
pub(crate) async fn ensure_caddy_running(
    container: &dyn ContainerRuntime,
    process: &dyn ProcessManager,
    node_prefix: &Ipv6Net,
    data_dir: &std::path::Path,
) -> Result<std::net::SocketAddr, CaddyStartupError> {
    use crate::system::types::{ContainerStatus, TransientRestart, TransientUnitSpec};
    use std::net::{IpAddr, SocketAddr};
    use std::time::Duration;

    // Ensure the proxy network exists.
    let proxy_prefix = proxy_network_prefix(node_prefix);
    if !container
        .network_exists(PROXY_NETWORK)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        container
            .create_network(PROXY_NETWORK, proxy_prefix)
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // Write admin config so Caddy binds the admin API on all interfaces.
    let admin_config_path = data_dir.join("caddy-admin.json");
    std::fs::write(&admin_config_path, CADDY_ADMIN_JSON)
        .map_err(|e| CaddyStartupError::Io { source: e })?;

    // Ensure the data volume exists.
    if !container
        .volume_exists(CADDY_DATA_VOLUME)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        container
            .create_volume(CADDY_DATA_VOLUME)
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // Check if Caddy is already running and healthy.
    if let Some(state) = container
        .inspect(CADDY_CONTAINER)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        if matches!(state.status, ContainerStatus::Running)
            && let Some(ip) = state.pod_addr
        {
            let addr = SocketAddr::new(IpAddr::V6(ip), 2019);
            let probe = CaddyProxy::new(addr);
            if probe.is_healthy().await.unwrap_or(false) {
                return Ok(addr);
            }
        }
        // Not healthy — remove and restart.
        container
            .remove_container(CADDY_CONTAINER, true)
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // Ensure the image is present.
    if !container
        .image_exists(CADDY_IMAGE)
        .await
        .map_err(|e| CaddyStartupError::Container { source: e })?
    {
        container
            .pull_image(CADDY_IMAGE)
            .await
            .map_err(|e| CaddyStartupError::Container { source: e })?;
    }

    // Start via a transient systemd unit.
    let admin_config_str = admin_config_path.to_string_lossy().into_owned();
    process
        .start_transient(TransientUnitSpec {
            name: CADDY_UNIT.to_owned(),
            description: "seedling Caddy proxy".to_owned(),
            exec_start: vec![
                "podman".to_owned(),
                "run".to_owned(),
                "--rm".to_owned(),
                "--name".to_owned(),
                CADDY_CONTAINER.to_owned(),
                "--network".to_owned(),
                PROXY_NETWORK.to_owned(),
                "--volume".to_owned(),
                format!("{CADDY_DATA_VOLUME}:/data"),
                "--volume".to_owned(),
                format!("{admin_config_str}:/etc/caddy/admin.json:ro"),
                CADDY_IMAGE.to_owned(),
                "caddy".to_owned(),
                "run".to_owned(),
                "--config".to_owned(),
                "/etc/caddy/admin.json".to_owned(),
            ],
            restart: TransientRestart::Always,
        })
        .await
        .map_err(|e| CaddyStartupError::Process { source: e })?;

    // Poll until Caddy is running and healthy (60 s deadline).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(CaddyStartupError::Timeout);
        }

        if let Ok(Some(state)) = container.inspect(CADDY_CONTAINER).await
            && matches!(state.status, ContainerStatus::Running)
            && let Some(ip) = state.pod_addr
        {
            let addr = SocketAddr::new(IpAddr::V6(ip), 2019);
            let probe = CaddyProxy::new(addr);
            if probe.is_healthy().await.unwrap_or(false) {
                return Ok(addr);
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ---------------------------------------------------------------------------
// ProxyConfig → Caddy JSON
// ---------------------------------------------------------------------------

/// Converts a `ProxyConfig` into the Caddy admin API JSON format sent to
/// `POST /config/`. Caddy applies this atomically with no traffic drop.
///
/// Two HTTP servers are created when both HTTP and HTTPS listeners are
/// present, keeping redirect-only and proxy-only routes clearly separated:
///
/// - `seedling_https`: listens on all HTTPS/QUIC ports, serves proxy routes
///   for TLS-enabled virtual hosts.
/// - `seedling_http`: listens on all plain-HTTP ports, serves redirect routes
///   (for hosts with `tls_acme=true`) and proxy routes (for plain-HTTP hosts).
///
/// TLS certificates are obtained via ACME (Let's Encrypt) for any virtual
/// host with `tls_acme=true`.
fn build_caddy_config(config: &ProxyConfig) -> Value {
    let http_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Http)
        .map(|l| l.port)
        .collect();

    let https_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Https)
        .map(|l| l.port)
        .collect();

    let quic_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Quic)
        .map(|l| l.port)
        .collect();

    let mut servers = serde_json::Map::new();

    // --- HTTPS server ---
    let mut https_listens: Vec<String> = https_ports.iter().map(|p| format!(":{p}")).collect();
    for p in &quic_ports {
        https_listens.push(format!(":{p}/quic"));
    }
    https_listens.dedup();

    if !https_listens.is_empty() {
        let https_routes: Vec<Value> = config
            .virtual_hosts
            .iter()
            .filter(|vh| vh.tls_acme)
            .flat_map(proxy_routes_for_vhost)
            .collect();

        if !https_routes.is_empty() {
            servers.insert(
                "seedling_https".to_string(),
                json!({ "listen": https_listens, "routes": https_routes }),
            );
        }
    }

    // --- HTTP server ---
    let http_listens: Vec<String> = http_ports.iter().map(|p| format!(":{p}")).collect();
    if !http_listens.is_empty() {
        let mut http_routes: Vec<Value> = Vec::new();

        for vh in &config.virtual_hosts {
            if let Some(redirect) = &vh.redirect {
                // Redirect route: HTTP → HTTPS
                http_routes.push(redirect_route(&vh.hostname, redirect.code, &https_ports));
            } else if !vh.tls_acme {
                // Plain HTTP proxy route
                http_routes.extend(proxy_routes_for_vhost(vh));
            }
        }

        if !http_routes.is_empty() {
            servers.insert(
                "seedling_http".to_string(),
                json!({ "listen": http_listens, "routes": http_routes }),
            );
        }
    }

    // --- TLS automation ---
    let tls_subjects: Vec<&str> = config
        .virtual_hosts
        .iter()
        .filter(|vh| vh.tls_acme)
        .map(|vh| vh.hostname.as_str())
        .collect();

    let mut apps = json!({ "http": { "servers": servers } });

    if !tls_subjects.is_empty() {
        apps["tls"] = json!({
            "automation": {
                "policies": [{
                    "subjects": tls_subjects,
                    "issuers": [{ "module": "acme" }],
                }]
            }
        });
    }

    json!({ "apps": apps })
}

/// Builds one Caddy route object per `ProxyRoute` within a virtual host.
fn proxy_routes_for_vhost(vh: &VirtualHost) -> Vec<Value> {
    vh.routes
        .iter()
        .map(|route| {
            let match_expr = if route.prefix == "/" {
                json!({ "host": [&vh.hostname] })
            } else {
                json!({ "host": [&vh.hostname], "path": [format!("{}*", route.prefix)] })
            };

            let upstreams: Vec<Value> = route
                .upstreams
                .iter()
                .map(|u| {
                    // Upstream URLs are "http://[fd5e:...]:3000".
                    // Caddy's `dial` field expects "[fd5e:...]:3000" (no scheme).
                    let dial = u.strip_prefix("http://").unwrap_or(u).to_string();
                    json!({ "dial": dial })
                })
                .collect();

            json!({
                "match": [match_expr],
                "handle": [{
                    "handler": "reverse_proxy",
                    "upstreams": upstreams,
                }],
                "terminal": true,
            })
        })
        .collect()
}

/// Builds a Caddy route that issues an HTTP redirect to the HTTPS port.
fn redirect_route(hostname: &str, code: u16, https_ports: &[u16]) -> Value {
    let target_port = https_ports.first().copied().unwrap_or(443);
    let location = if target_port == 443 {
        "https://{http.request.host}{http.request.uri}".to_string()
    } else {
        format!("https://{{http.request.host}}:{target_port}{{http.request.uri}}")
    };

    json!({
        "match": [{ "host": [hostname] }],
        "handle": [{
            "handler": "static_response",
            "status_code": code,
            "headers": { "Location": [location] },
        }],
        "terminal": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::types::{HttpRedirect, ProxyListener, ProxyRoute, VirtualHost};

    fn http_vhost(hostname: &str, upstream: &str) -> VirtualHost {
        VirtualHost {
            hostname: hostname.to_string(),
            tls_acme: false,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                upstreams: vec![format!("http://{upstream}")],
            }],
        }
    }

    fn https_vhost(hostname: &str, upstream: &str) -> VirtualHost {
        VirtualHost {
            hostname: hostname.to_string(),
            tls_acme: true,
            redirect: Some(HttpRedirect {
                from_port: 80,
                code: 308,
            }),
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                upstreams: vec![format!("http://{upstream}")],
            }],
        }
    }

    #[test]
    fn empty_config_produces_empty_servers() {
        let config = ProxyConfig::default();
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(servers.as_object().is_none_or(|m| m.is_empty()));
    }

    #[test]
    fn http_only_vhost_goes_in_http_server() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 80,
                proto: ProxyListenerProto::Http,
            }],
            virtual_hosts: vec![http_vhost("example.com", "[fd5e::1]:3000")],
        };
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(servers["seedling_http"].is_object());
        assert!(servers["seedling_https"].is_null());
    }

    #[test]
    fn https_vhost_goes_in_https_server_redirect_in_http() {
        let config = ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 80,
                    proto: ProxyListenerProto::Http,
                },
            ],
            virtual_hosts: vec![https_vhost("example.com", "[fd5e::1]:3000")],
        };
        let json = build_caddy_config(&config);
        let servers = &json["apps"]["http"]["servers"];
        assert!(
            servers["seedling_https"].is_object(),
            "missing https server"
        );
        assert!(servers["seedling_http"].is_object(), "missing http server");

        // https server should have proxy routes
        let https_routes = &servers["seedling_https"]["routes"];
        assert!(https_routes.as_array().is_some_and(|r| !r.is_empty()));

        // http server should have redirect route
        let http_routes = &servers["seedling_http"]["routes"];
        let redirect = &http_routes[0];
        assert_eq!(redirect["handle"][0]["handler"], "static_response");
        assert_eq!(redirect["handle"][0]["status_code"], 308);
    }

    #[test]
    fn tls_acme_subjects_appear_in_automation() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            }],
            virtual_hosts: vec![VirtualHost {
                hostname: "secure.example.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                }],
            }],
        };
        let json = build_caddy_config(&config);
        let subjects = &json["apps"]["tls"]["automation"]["policies"][0]["subjects"];
        assert_eq!(subjects[0], "secure.example.com");
    }

    #[test]
    fn dial_strips_http_scheme() {
        let config = ProxyConfig {
            listeners: vec![ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            }],
            virtual_hosts: vec![VirtualHost {
                hostname: "x.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e:ed12:3456:0100::3]:3000".to_string()],
                }],
            }],
        };
        let json = build_caddy_config(&config);
        let dial = &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][0]["upstreams"]
            [0]["dial"];
        assert_eq!(dial, "[fd5e:ed12:3456:0100::3]:3000");
    }

    #[test]
    fn quic_listener_appended_to_https_server() {
        let config = ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Quic,
                },
            ],
            virtual_hosts: vec![VirtualHost {
                hostname: "quic.example.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![ProxyRoute {
                    prefix: "/".to_string(),
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                }],
            }],
        };
        let json = build_caddy_config(&config);
        let listen = &json["apps"]["http"]["servers"]["seedling_https"]["listen"];
        let listen_strs: Vec<&str> = listen
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(listen_strs.contains(&":443"));
        assert!(listen_strs.contains(&":443/quic"));
    }
}
