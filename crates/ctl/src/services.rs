use std::net::SocketAddr;

use clap::Subcommand;
use seedling_protocol::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum ServicesCommand {
    /// List services apps have marked `service.exported()`
    Exported {
        #[command(subcommand)]
        command: ExportedCommand,
    },
    /// List every named app service on the site
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    /// Site service management
    Site {
        #[command(subcommand)]
        command: SiteCommand,
    },
    /// External-service mapping (operator wires up `app.external_service(...)` slots)
    External {
        #[command(subcommand)]
        command: ExternalCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum ExportedCommand {
    /// List exported services
    List,
}

#[derive(Subcommand)]
pub(super) enum AppCommand {
    /// List every named app service, with an `exported` flag
    List,
}

#[derive(Subcommand)]
pub(super) enum SiteCommand {
    /// Create a site service (no endpoints yet; use `add-port` after)
    Create {
        /// Site service name
        name: String,
        /// Operator-facing description
        #[arg(long)]
        description: Option<String>,
    },
    /// List site services with their endpoints
    List,
    /// Delete a site service (refused while any external-service slot still maps to it)
    Delete {
        /// Site service name
        name: String,
    },
    /// Add an endpoint to a site service
    ///
    /// The remote address is given as `[ipv6]:port`, `<ipv4>:port`, or
    /// `<host>:port`. DNS names are resolved at runtime by the daemon;
    /// IPv4 and A-only DNS endpoints route via NAT64 when active.
    AddPort {
        /// Site service name
        name: String,
        /// Service-side port the site service exposes
        service_port: u16,
        /// Protocol: tcp, udp, or http
        protocol: String,
        /// Remote backend, e.g. `[2001:db8::1]:8080`, `10.0.0.1:5432`,
        /// or `db.example.com:5432`
        remote: String,
    },
    /// Remove an endpoint from a site service
    RemovePort {
        /// Site service name
        name: String,
        /// Service-side port
        service_port: u16,
        /// Protocol: tcp, udp, or http
        protocol: String,
        /// Remote backend, same shapes as `add-port`
        remote: String,
    },
    /// Show the daemon's site-service DNS resolver cache
    Resolver,
}

#[derive(Subcommand)]
pub(super) enum ExternalCommand {
    /// Map an app's external-service slot to a concrete target
    ///
    /// Targets use the `_site/<name>` / `<app>/<service>` shorthand, e.g.
    /// `_site/postgres-prod` or `api-app/api`.
    Map {
        /// App declaring the slot
        app: String,
        /// Slot name (from `app.external_service(...)`)
        slot: String,
        /// Target reference
        target: String,
    },
    /// Remove an external-service mapping
    Unmap {
        /// App declaring the slot
        app: String,
        /// Slot name
        slot: String,
    },
    /// Retarget an existing external-service mapping
    Remap {
        /// App declaring the slot
        app: String,
        /// Slot name
        slot: String,
        /// New target reference
        target: String,
    },
    /// List external-service mappings
    List {
        /// Filter to a single app
        #[arg(long)]
        app: Option<String>,
    },
    /// List every external-service slot declared across apps
    Declared,
}

pub(super) async fn dispatch(client: &OiClient, cmd: ServicesCommand) {
    match cmd {
        ServicesCommand::Exported { command } => match command {
            ExportedCommand::List => {
                print_result(
                    client
                        .request("/services/exported/list", serde_json::json!({}))
                        .await,
                );
            }
        },
        ServicesCommand::App { command } => match command {
            AppCommand::List => {
                print_result(
                    client
                        .request("/services/app/list", serde_json::json!({}))
                        .await,
                );
            }
        },
        ServicesCommand::Site { command } => dispatch_site(client, command).await,
        ServicesCommand::External { command } => dispatch_external(client, command).await,
    }
}

async fn dispatch_site(client: &OiClient, cmd: SiteCommand) {
    match cmd {
        SiteCommand::Create { name, description } => {
            let mut body = serde_json::json!({ "name": name });
            if let Some(desc) = description {
                body["description"] = serde_json::Value::String(desc);
            }
            print_result(client.request("/services/site/create", body).await);
        }
        SiteCommand::List => {
            print_result(
                client
                    .request("/services/site/list", serde_json::json!({}))
                    .await,
            );
        }
        SiteCommand::Delete { name } => {
            print_result(
                client
                    .request("/services/site/delete", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        SiteCommand::AddPort {
            name,
            service_port,
            protocol,
            remote,
        } => {
            let (remote_host, remote_port) = match parse_remote(&remote) {
                Ok(pair) => pair,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/services/site/endpoint/add",
                        serde_json::json!({
                            "name": name,
                            "service_port": service_port,
                            "protocol": protocol,
                            "remote_host": remote_host,
                            "remote_port": remote_port,
                        }),
                    )
                    .await,
            );
        }
        SiteCommand::RemovePort {
            name,
            service_port,
            protocol,
            remote,
        } => {
            let (remote_host, remote_port) = match parse_remote(&remote) {
                Ok(pair) => pair,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/services/site/endpoint/remove",
                        serde_json::json!({
                            "name": name,
                            "service_port": service_port,
                            "protocol": protocol,
                            "remote_host": remote_host,
                            "remote_port": remote_port,
                        }),
                    )
                    .await,
            );
        }
        SiteCommand::Resolver => {
            print_result(
                client
                    .request("/services/site/resolver-status", serde_json::json!({}))
                    .await,
            );
        }
    }
}

async fn dispatch_external(client: &OiClient, cmd: ExternalCommand) {
    match cmd {
        ExternalCommand::Map { app, slot, target } => {
            let target_json = match parse_target(&target) {
                Ok(t) => t,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/services/external/map",
                        serde_json::json!({
                            "app": app,
                            "external_name": slot,
                            "target": target_json,
                        }),
                    )
                    .await,
            );
        }
        ExternalCommand::Unmap { app, slot } => {
            print_result(
                client
                    .request(
                        "/services/external/unmap",
                        serde_json::json!({
                            "app": app,
                            "external_name": slot,
                        }),
                    )
                    .await,
            );
        }
        ExternalCommand::Remap { app, slot, target } => {
            let target_json = match parse_target(&target) {
                Ok(t) => t,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/services/external/remap",
                        serde_json::json!({
                            "app": app,
                            "external_name": slot,
                            "target": target_json,
                        }),
                    )
                    .await,
            );
        }
        ExternalCommand::List { app } => {
            let body = match app {
                Some(a) => serde_json::json!({ "app": a }),
                None => serde_json::json!({}),
            };
            print_result(client.request("/services/external/list", body).await);
        }
        ExternalCommand::Declared => {
            print_result(
                client
                    .request("/services/external/declared", serde_json::json!({}))
                    .await,
            );
        }
    }
}

/// Parse a site-service remote address into `(host, port)`. Accepted shapes:
///
/// - `[ipv6]:port` — IPv6 literal (brackets required).
/// - `<ipv4>:port` — IPv4 literal.
/// - `<dns-name>:port` — bare DNS name.
///
/// IPv6 literals must be bracketed even when no port-disambiguating colon
/// appears in the address, mirroring URL syntax. The daemon validates the
/// host string further at the OI layer (rejecting `localhost`, underscore
/// labels, etc).
fn parse_remote(s: &str) -> Result<(String, u16), String> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return match addr {
            SocketAddr::V6(v6) => Ok((v6.ip().to_string(), v6.port())),
            SocketAddr::V4(v4) => Ok((v4.ip().to_string(), v4.port())),
        };
    }
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid remote {s:?}: expected <host>:<port>"))?;
    if host.is_empty() {
        return Err(format!("invalid remote {s:?}: host must not be empty"));
    }
    let port: u16 = port
        .parse()
        .map_err(|e| format!("invalid remote {s:?}: bad port: {e}"))?;
    // Strip surrounding brackets if the operator wrote `[host]:port` for a
    // bare DNS name (URL-style); the host string we send to the OI doesn't
    // carry brackets.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    Ok((host.to_owned(), port))
}

/// Parse an external-service target in the `_site/<name>` or `<app>/<service>`
/// form into the JSON shape the OI expects for `ServiceRef`.
fn parse_target(s: &str) -> Result<serde_json::Value, String> {
    let (prefix, tail) = s
        .split_once('/')
        .ok_or_else(|| format!("invalid target {s:?}: expected _site/<name> or <app>/<service>"))?;
    if prefix.is_empty() || tail.is_empty() {
        return Err(format!("invalid target {s:?}: neither part may be empty"));
    }
    if prefix == "_site" {
        Ok(serde_json::json!({ "kind": "site", "name": tail }))
    } else {
        Ok(serde_json::json!({ "kind": "app", "app": prefix, "service": tail }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_accepts_bracketed_ipv6() {
        let (host, port) = parse_remote("[2001:db8::1]:3000").unwrap();
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, 3000);
    }

    #[test]
    fn parse_remote_accepts_ipv4() {
        let (host, port) = parse_remote("10.0.0.1:80").unwrap();
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 80);
    }

    #[test]
    fn parse_remote_accepts_dns_name() {
        let (host, port) = parse_remote("db.example.com:5432").unwrap();
        assert_eq!(host, "db.example.com");
        assert_eq!(port, 5432);
    }

    #[test]
    fn parse_remote_strips_optional_brackets_around_dns_name() {
        // URL-style brackets are tolerated for ergonomic consistency with
        // IPv6 input, but the OI sees the unbracketed host.
        let (host, port) = parse_remote("[host.example]:80").unwrap();
        assert_eq!(host, "host.example");
        assert_eq!(port, 80);
    }

    #[test]
    fn parse_remote_rejects_missing_port() {
        assert!(parse_remote("example.com").is_err());
    }

    #[test]
    fn parse_remote_rejects_empty_host() {
        assert!(parse_remote(":80").is_err());
    }

    #[test]
    fn parse_target_site_shorthand() {
        let v = parse_target("_site/postgres-prod").unwrap();
        assert_eq!(v["kind"], "site");
        assert_eq!(v["name"], "postgres-prod");
    }

    #[test]
    fn parse_target_app_shorthand() {
        let v = parse_target("api-app/api").unwrap();
        assert_eq!(v["kind"], "app");
        assert_eq!(v["app"], "api-app");
        assert_eq!(v["service"], "api");
    }

    #[test]
    fn parse_target_rejects_missing_slash() {
        assert!(parse_target("justaname").is_err());
    }
}
