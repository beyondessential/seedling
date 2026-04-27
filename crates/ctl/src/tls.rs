use std::path::PathBuf;

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::{Value, json};

use super::print_result;

#[derive(Subcommand)]
pub(super) enum TlsCommand {
    /// DNS provider credentials for the ACME-DNS strategy
    DnsProviders {
        #[command(subcommand)]
        command: DnsProvidersCommand,
    },
    /// Per-hostname strategy policies
    Policies {
        #[command(subcommand)]
        command: PoliciesCommand,
    },
    /// Stored certificates
    Certs {
        #[command(subcommand)]
        command: CertsCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum DnsProvidersCommand {
    /// List configured DNS providers (credentials are not displayed)
    List,
    /// Add or replace a DNS provider entry
    ///
    /// `--config` accepts either an inline JSON blob or `@/path/to/file`
    /// containing JSON. For Route 53:
    ///     {"access_key_id": "AKIA...", "secret_access_key": "...", "region": "us-east-1"}
    Set {
        /// Operator-chosen name for this provider entry
        name: String,
        /// Provider kind. Currently only `route53` is supported.
        #[arg(long, default_value = "route53")]
        kind: String,
        /// Provider-specific JSON config; prefix with `@` to read from a file
        #[arg(long)]
        config: String,
    },
    /// Delete a DNS provider entry
    Delete { name: String },
}

#[derive(Subcommand)]
pub(super) enum PoliciesCommand {
    /// List per-hostname policies (hostnames absent from the list use the default ACME-HTTP-01)
    List,
    /// Bind a hostname to ACME-DNS issuance via a configured provider
    ///
    /// Supplying `--contact` triggers a one-shot background issuance for the
    /// hostname if it has no current active cert, so the operator does not
    /// have to run a separate `tls certs issue-acme-dns` afterwards. The
    /// daemon prints the cert id (or a failure) to its log; on success the
    /// hostname will appear under `tls certs list` within a few seconds.
    SetAcmeDns {
        hostname: String,
        /// Name of a configured DNS provider
        #[arg(long)]
        provider: String,
        /// Operator contact email — kicks off auto-issuance when supplied
        #[arg(long)]
        contact: Option<String>,
        /// ACME directory URL (defaults to Let's Encrypt production)
        #[arg(long)]
        directory: Option<String>,
    },
    /// Bind a hostname to a stored certificate (manual or CSR-derived)
    SetManual {
        hostname: String,
        #[arg(long)]
        cert_id: i64,
    },
    /// Remove the policy for a hostname, returning it to the default ACME-HTTP-01
    Clear { hostname: String },
}

#[derive(Subcommand)]
pub(super) enum CertsCommand {
    /// List all stored certificates
    List,
    /// Run ACME-DNS issuance now for a hostname already bound to an acme_dns policy
    IssueAcmeDns {
        hostname: String,
        /// Operator contact email (used for ACME account registration)
        #[arg(long)]
        contact: String,
        /// ACME directory URL (defaults to Let's Encrypt production)
        #[arg(long)]
        directory: Option<String>,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: TlsCommand) {
    match cmd {
        TlsCommand::DnsProviders { command } => dispatch_dns_providers(client, command).await,
        TlsCommand::Policies { command } => dispatch_policies(client, command).await,
        TlsCommand::Certs { command } => dispatch_certs(client, command).await,
    }
}

async fn dispatch_dns_providers(client: &OiClient, cmd: DnsProvidersCommand) {
    match cmd {
        DnsProvidersCommand::List => {
            print_result(client.request("/tls/dns-providers/list", json!({})).await);
        }
        DnsProvidersCommand::Set { name, kind, config } => {
            let config_value = match read_config_arg(&config) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error reading --config: {e}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/tls/dns-providers/upsert",
                        json!({ "name": name, "kind": kind, "config": config_value }),
                    )
                    .await,
            );
        }
        DnsProvidersCommand::Delete { name } => {
            print_result(
                client
                    .request("/tls/dns-providers/delete", json!({ "name": name }))
                    .await,
            );
        }
    }
}

async fn dispatch_policies(client: &OiClient, cmd: PoliciesCommand) {
    match cmd {
        PoliciesCommand::List => {
            print_result(client.request("/tls/policies/list", json!({})).await);
        }
        PoliciesCommand::SetAcmeDns {
            hostname,
            provider,
            contact,
            directory,
        } => {
            let mut params = json!({ "hostname": hostname, "dns_provider": provider });
            if let Some(c) = contact {
                params["contact_email"] = json!(c);
            }
            if let Some(d) = directory {
                params["directory_url"] = json!(d);
            }
            print_result(client.request("/tls/policies/set-acme-dns", params).await);
        }
        PoliciesCommand::SetManual { hostname, cert_id } => {
            print_result(
                client
                    .request(
                        "/tls/policies/set-manual",
                        json!({ "hostname": hostname, "cert_id": cert_id }),
                    )
                    .await,
            );
        }
        PoliciesCommand::Clear { hostname } => {
            print_result(
                client
                    .request("/tls/policies/clear", json!({ "hostname": hostname }))
                    .await,
            );
        }
    }
}

async fn dispatch_certs(client: &OiClient, cmd: CertsCommand) {
    match cmd {
        CertsCommand::List => {
            print_result(client.request("/tls/certificates/list", json!({})).await);
        }
        CertsCommand::IssueAcmeDns {
            hostname,
            contact,
            directory,
        } => {
            let mut params = json!({ "hostname": hostname, "contact_email": contact });
            if let Some(dir) = directory {
                params["directory_url"] = json!(dir);
            }
            print_result(
                client
                    .request("/tls/certificates/issue-acme-dns", params)
                    .await,
            );
        }
    }
}

/// Accept either an inline JSON blob or `@/path/to/file`.
fn read_config_arg(s: &str) -> Result<Value, String> {
    if let Some(path) = s.strip_prefix('@') {
        let path = PathBuf::from(path);
        let contents =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&contents)
            .map_err(|e| format!("parse JSON in {}: {e}", path.display()))
    } else {
        serde_json::from_str(s).map_err(|e| format!("parse inline JSON: {e}"))
    }
}
