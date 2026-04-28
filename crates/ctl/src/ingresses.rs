use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::{Value, json};

use super::print_result;

#[derive(Subcommand)]
pub(super) enum IngressesCommand {
    /// Site ingress management
    Site {
        #[command(subcommand)]
        command: SiteCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum SiteCommand {
    /// List all site ingresses with their attachments
    List,
    /// Show a single site ingress with its attachments
    Show {
        /// Site ingress name
        name: String,
    },
    /// Create a manual site ingress
    Create {
        /// Site ingress name
        name: String,
        /// Hostname this ingress serves (e.g. `old.example.com`)
        #[arg(long)]
        hostname: String,
        /// Operator-facing description
        #[arg(long)]
        description: Option<String>,
        /// TLS provisioning mode: `acme` (default), `internal`, or `none`.
        /// `tailscale` is reserved for discovered Tailscale ingresses and
        /// is rejected here.
        #[arg(long, default_value = "acme")]
        tls: String,
    },
    /// Delete a manual site ingress (refused on discovered ingresses)
    Delete {
        /// Site ingress name
        name: String,
    },
    /// Update the description and/or TLS provider of a manual site ingress
    Update {
        /// Site ingress name
        name: String,
        /// New description; pass an empty string to clear, or omit to leave unchanged
        #[arg(long)]
        description: Option<String>,
        /// New TLS provider: `acme`, `internal`, or `none`
        #[arg(long)]
        tls: Option<String>,
        /// Clear the description (mutually exclusive with --description)
        #[arg(long, conflicts_with = "description")]
        clear_description: bool,
    },
    /// Attach a (port, protocol) on this site ingress to an app service
    Attach {
        /// Site ingress name
        name: String,
        /// Listening port
        #[arg(long)]
        port: u16,
        /// Protocol: `tcp`, `udp`, `http`, or `http2`
        #[arg(long)]
        protocol: String,
        /// Forward target in `<app>/<service>` form
        #[arg(long)]
        to: String,
    },
    /// Attach a redirect on this site ingress
    AttachRedirect {
        /// Site ingress name
        name: String,
        /// Listening port
        #[arg(long)]
        port: u16,
        /// Protocol: `http` or `http2`
        #[arg(long)]
        protocol: String,
        /// Redirect destination URL (must start with `http://` or `https://`)
        #[arg(long)]
        to: String,
        /// HTTP redirect status code (301, 302, 307, 308). Defaults to 307.
        #[arg(long, default_value_t = 307)]
        code: u16,
        /// Send the URL verbatim instead of preserving the request path
        #[arg(long)]
        no_preserve_path: bool,
    },
    /// Remove an attachment from a site ingress
    Detach {
        /// Site ingress name
        name: String,
        /// Listening port
        #[arg(long)]
        port: u16,
        /// Protocol
        #[arg(long)]
        protocol: String,
    },
    /// Inspect discovery providers (Tailscale, ...)
    Discovery {
        #[command(subcommand)]
        command: DiscoveryCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum DiscoveryCommand {
    /// Show what each discovery provider has populated
    Status,
}

pub(super) async fn dispatch(client: &OiClient, cmd: IngressesCommand) {
    match cmd {
        IngressesCommand::Site { command } => dispatch_site(client, command).await,
    }
}

async fn dispatch_site(client: &OiClient, cmd: SiteCommand) {
    match cmd {
        SiteCommand::List => {
            print_result(client.request("/ingresses/site/list", json!({})).await);
        }
        SiteCommand::Show { name } => {
            print_result(
                client
                    .request("/ingresses/site/show", json!({ "name": name }))
                    .await,
            );
        }
        SiteCommand::Create {
            name,
            hostname,
            description,
            tls,
        } => {
            let mut body = json!({
                "name": name,
                "hostname": hostname,
                "tls_provider": tls,
            });
            if let Some(desc) = description {
                body["description"] = Value::String(desc);
            }
            print_result(client.request("/ingresses/site/create", body).await);
        }
        SiteCommand::Delete { name } => {
            print_result(
                client
                    .request("/ingresses/site/delete", json!({ "name": name }))
                    .await,
            );
        }
        SiteCommand::Update {
            name,
            description,
            tls,
            clear_description,
        } => {
            let mut body = json!({ "name": name });
            // Outer Some => operator opted to set description; inner None
            // means clear it.
            if clear_description {
                body["description"] = Value::Null;
            } else if let Some(desc) = description {
                body["description"] = Value::String(desc);
            }
            if let Some(t) = tls {
                body["tls_provider"] = Value::String(t);
            }
            print_result(client.request("/ingresses/site/update", body).await);
        }
        SiteCommand::Attach {
            name,
            port,
            protocol,
            to,
        } => {
            let (app, service) = match parse_app_service(&to) {
                Ok(p) => p,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/ingresses/site/attach/forward",
                        json!({
                            "name": name,
                            "port": port,
                            "protocol": protocol,
                            "target_app": app,
                            "target_service": service,
                        }),
                    )
                    .await,
            );
        }
        SiteCommand::AttachRedirect {
            name,
            port,
            protocol,
            to,
            code,
            no_preserve_path,
        } => {
            print_result(
                client
                    .request(
                        "/ingresses/site/attach/redirect",
                        json!({
                            "name": name,
                            "port": port,
                            "protocol": protocol,
                            "redirect_url": to,
                            "redirect_code": code,
                            "preserve_path": !no_preserve_path,
                        }),
                    )
                    .await,
            );
        }
        SiteCommand::Detach {
            name,
            port,
            protocol,
        } => {
            print_result(
                client
                    .request(
                        "/ingresses/site/detach",
                        json!({
                            "name": name,
                            "port": port,
                            "protocol": protocol,
                        }),
                    )
                    .await,
            );
        }
        SiteCommand::Discovery { command } => match command {
            DiscoveryCommand::Status => {
                print_result(
                    client
                        .request("/ingresses/site/discovery/status", json!({}))
                        .await,
                );
            }
        },
    }
}

/// Parse an `<app>/<service>` shorthand (used by the attach forward command).
fn parse_app_service(s: &str) -> Result<(String, String), String> {
    let (app, service) = s
        .split_once('/')
        .ok_or_else(|| format!("invalid forward target {s:?}: expected <app>/<service>"))?;
    if app.is_empty() || service.is_empty() {
        return Err(format!(
            "invalid forward target {s:?}: neither part may be empty"
        ));
    }
    Ok((app.to_owned(), service.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_app_service_round_trip() {
        let (a, s) = parse_app_service("api-app/api").unwrap();
        assert_eq!(a, "api-app");
        assert_eq!(s, "api");
    }

    #[test]
    fn parse_app_service_rejects_missing_slash() {
        assert!(parse_app_service("solo").is_err());
    }

    #[test]
    fn parse_app_service_rejects_empty_parts() {
        assert!(parse_app_service("/api").is_err());
        assert!(parse_app_service("api/").is_err());
    }
}
