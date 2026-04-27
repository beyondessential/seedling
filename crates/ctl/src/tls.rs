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
    /// Per-hostname strategy policies. Hostname patterns may be exact,
    /// the catch-all `*`, or a shell-glob subdomain wildcard
    /// (`*.example.com` matches `foo.example.com` and `a.b.example.com`).
    Policies {
        #[command(subcommand)]
        command: PoliciesCommand,
    },
    /// Stored certificates
    Certs {
        #[command(subcommand)]
        command: CertsCommand,
    },
    /// Global TLS settings (operator contact email, …)
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Cert-issuance attempt log (success and failures)
    Attempts {
        /// Filter by hostname
        #[arg(long)]
        hostname: Option<String>,
        /// Maximum number of entries (newest first)
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    /// Retry blocks: per-hostname pauses on automatic ACME-DNS issuance
    RetryBlocks {
        #[command(subcommand)]
        command: RetryBlocksCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum RetryBlocksCommand {
    /// List active retry blocks
    List,
    /// Pause on-demand issuance for a hostname
    Set {
        hostname: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Resume on-demand issuance for a hostname
    Clear { hostname: String },
}

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    /// Show the current global TLS settings
    Get,
    /// Set the operator contact email used for ACME account registration
    SetContact { email: String },
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
    /// Bind a hostname (or wildcard pattern) to ACME-DNS issuance via a
    /// configured provider.
    ///
    /// `<hostname>` may be exact (`foo.example.com`), a shell-glob subdomain
    /// wildcard (`*.example.com`, which covers `foo.example.com`,
    /// `a.b.example.com`, and any deeper subdomain — *not* RFC 6125
    /// single-label semantics), or the catch-all `*`. When the global
    /// contact email is configured (`tls config set-contact`) and the
    /// hostname is exact, the daemon kicks off auto-issuance in the
    /// background; the cert lands in `tls certs list` within seconds.
    SetAcmeDns {
        hostname: String,
        /// Name of a configured DNS provider
        #[arg(long)]
        provider: String,
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
    /// Run ACME-DNS issuance now for a hostname covered by an acme_dns policy
    IssueAcmeDns {
        hostname: String,
        /// Operator contact email override (defaults to the global setting)
        #[arg(long)]
        contact: Option<String>,
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
        TlsCommand::Config { command } => dispatch_config(client, command).await,
        TlsCommand::Attempts { hostname, limit } => {
            let mut params = json!({ "limit": limit });
            if let Some(h) = hostname {
                params["hostname"] = json!(h);
            }
            print_result(
                client
                    .request("/tls/certificates/attempts/list", params)
                    .await,
            );
        }
        TlsCommand::RetryBlocks { command } => dispatch_retry_blocks(client, command).await,
    }
}

async fn dispatch_retry_blocks(client: &OiClient, cmd: RetryBlocksCommand) {
    match cmd {
        RetryBlocksCommand::List => {
            print_result(client.request("/tls/retry-blocks/list", json!({})).await);
        }
        RetryBlocksCommand::Set { hostname, reason } => {
            let mut params = json!({ "hostname": hostname });
            if let Some(r) = reason {
                params["reason"] = json!(r);
            }
            print_result(client.request("/tls/retry-blocks/set", params).await);
        }
        RetryBlocksCommand::Clear { hostname } => {
            print_result(
                client
                    .request("/tls/retry-blocks/clear", json!({ "hostname": hostname }))
                    .await,
            );
        }
    }
}

async fn dispatch_config(client: &OiClient, cmd: ConfigCommand) {
    match cmd {
        ConfigCommand::Get => {
            print_result(client.request("/tls/settings/get", json!({})).await);
        }
        ConfigCommand::SetContact { email } => {
            print_result(
                client
                    .request("/tls/settings/set", json!({ "contact_email": email }))
                    .await,
            );
        }
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
            directory,
        } => {
            let mut params = json!({ "hostname": hostname, "dns_provider": provider });
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
            let mut params = json!({ "hostname": hostname });
            if let Some(c) = contact {
                params["contact_email"] = json!(c);
            }
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
