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
    /// Per-hostname rollup of TLS state for every TLS-terminating ingress.
    ///
    /// Combines policy, active certificate, latest attempt outcome, retry
    /// blocks, and expected next issuance into a single view. Optionally
    /// filtered to a single app's ingresses.
    Hostnames {
        /// Filter to a single app's ingress hostnames
        #[arg(long)]
        app: Option<String>,
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
pub(super) enum CsrCommand {
    /// Generate a new keypair + CSR for `hostname`. The private key
    /// stays on the server (encrypted at rest); only the CSR is
    /// returned for external signing.
    Begin {
        hostname: String,
        /// Key type. Currently only `ecdsa-p256` is supported.
        #[arg(long, default_value = "ecdsa-p256")]
        key_type: String,
    },
    /// Re-fetch the PEM CSR for a pending request.
    Get { id: i64 },
    /// Upload the externally-signed cert for a pending CSR. The
    /// runtime verifies the cert against the stored key and SAN
    /// coverage before transitioning the row to `active`.
    UploadCert {
        id: i64,
        /// PEM-encoded certificate chain, or `-` for stdin.
        #[arg(long)]
        cert: String,
    },
    /// Cancel a pending CSR; deletes the stored keypair.
    Cancel { id: i64 },
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
    /// Set the ACME profile name forwarded on every order (e.g.
    /// `shortlived` for Let's Encrypt's ~6-day certs). Use `--clear`
    /// to revert to the CA's default profile.
    SetProfile {
        /// The profile name (e.g. `shortlived`).
        #[arg(conflicts_with = "clear")]
        name: Option<String>,
        /// Clear the profile so the CA picks its default.
        #[arg(long, conflicts_with = "name")]
        clear: bool,
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
    /// Remove the policy for a hostname, returning it to the default ACME-HTTP-01
    Clear { hostname: String },
}

#[derive(Subcommand)]
pub(super) enum CertsCommand {
    /// List all stored certificates
    List,
    /// Upload an operator-supplied certificate and matching private key.
    ///
    /// Both `--cert` and `--key` accept a PEM file path (or `-` to read
    /// from stdin). On success the cert is stored and the new row id is
    /// printed; bind the cert to a hostname with `tls policies set-manual`.
    UploadManual {
        hostname: String,
        /// PEM-encoded certificate chain (leaf + optional intermediates),
        /// or `-` for stdin.
        #[arg(long)]
        cert: String,
        /// PEM-encoded PKCS#8 private key, or `-` for stdin.
        #[arg(long)]
        key: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete a stored certificate by id.
    Delete { id: i64 },
    /// Server-generated keypair + Certificate Signing Request flow.
    Csr {
        #[command(subcommand)]
        command: CsrCommand,
    },
    /// Run ACME-DNS issuance now for a hostname covered by an acme_dns policy.
    ///
    /// Blocks until the flow completes (typically tens of seconds). For a
    /// fire-and-forget retry that survives daemon restarts, use `retry`
    /// instead — it sets a persistent force-retry signal that the
    /// reconciler picks up on the next tick.
    IssueAcmeDns { hostname: String },
    /// Queue a retry for a hostname.
    ///
    /// Clears any operator pause, records a persistent force-retry signal,
    /// and nudges the issuance coordinator. Returns immediately; the cert
    /// appears in `tls certs list` once the reconciler runs the flow.
    Retry { hostname: String },
}

pub(super) async fn dispatch(client: &OiClient, cmd: TlsCommand) {
    match cmd {
        TlsCommand::DnsProviders { command } => dispatch_dns_providers(client, command).await,
        TlsCommand::Policies { command } => dispatch_policies(client, command).await,
        TlsCommand::Certs { command } => dispatch_certs(client, command).await,
        TlsCommand::Config { command } => dispatch_config(client, command).await,
        TlsCommand::Hostnames { app } => {
            let mut params = json!({});
            if let Some(a) = app {
                params["app"] = json!(a);
            }
            print_result(client.request("/tls/hostnames/list", params).await);
        }
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
        ConfigCommand::SetProfile { name, clear } => {
            // Send the cert_profile field as JSON null when clearing,
            // or as the supplied string. The OI handler distinguishes
            // "field absent" from "field present but null" so it only
            // touches the column the operator asked about.
            let value = if clear {
                serde_json::Value::Null
            } else {
                match name {
                    Some(n) => serde_json::Value::String(n),
                    None => {
                        eprintln!("error: pass either a profile name or --clear");
                        std::process::exit(1);
                    }
                }
            };
            print_result(
                client
                    .request("/tls/settings/set", json!({ "cert_profile": value }))
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
        CertsCommand::UploadManual {
            hostname,
            cert,
            key,
            note,
        } => {
            let cert_pem = match read_pem_arg(&cert) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error reading --cert: {e}");
                    std::process::exit(1);
                }
            };
            let key_pem = match read_pem_arg(&key) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error reading --key: {e}");
                    std::process::exit(1);
                }
            };
            let mut params = json!({
                "hostname": hostname,
                "cert_pem": cert_pem,
                "key_pem": key_pem,
            });
            if let Some(n) = note {
                params["note"] = json!(n);
            }
            print_result(
                client
                    .request("/tls/certificates/upload-manual", params)
                    .await,
            );
        }
        CertsCommand::Delete { id } => {
            print_result(
                client
                    .request("/tls/certificates/delete", json!({ "id": id }))
                    .await,
            );
        }
        CertsCommand::Csr { command } => dispatch_csr(client, command).await,
        CertsCommand::IssueAcmeDns { hostname } => {
            print_result(
                client
                    .request(
                        "/tls/certificates/issue-acme-dns",
                        json!({ "hostname": hostname }),
                    )
                    .await,
            );
        }
        CertsCommand::Retry { hostname } => {
            print_result(
                client
                    .request("/tls/certificates/retry", json!({ "hostname": hostname }))
                    .await,
            );
        }
    }
}

async fn dispatch_csr(client: &OiClient, cmd: CsrCommand) {
    match cmd {
        CsrCommand::Begin { hostname, key_type } => {
            let kt = key_type.replace('-', "_");
            print_result(
                client
                    .request(
                        "/tls/certificates/csr/begin",
                        json!({ "hostname": hostname, "key_type": kt }),
                    )
                    .await,
            );
        }
        CsrCommand::Get { id } => {
            print_result(
                client
                    .request("/tls/certificates/csr/get", json!({ "id": id }))
                    .await,
            );
        }
        CsrCommand::UploadCert { id, cert } => {
            let cert_pem = match read_pem_arg(&cert) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error reading --cert: {e}");
                    std::process::exit(1);
                }
            };
            print_result(
                client
                    .request(
                        "/tls/certificates/csr/upload-cert",
                        json!({ "id": id, "cert_pem": cert_pem }),
                    )
                    .await,
            );
        }
        CsrCommand::Cancel { id } => {
            print_result(
                client
                    .request("/tls/certificates/csr/cancel", json!({ "id": id }))
                    .await,
            );
        }
    }
}

/// Read a PEM blob from a path, or from stdin when `s` is `-`.
fn read_pem_arg(s: &str) -> Result<String, String> {
    if s == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("read stdin: {e}"))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(s).map_err(|e| format!("read {s}: {e}"))
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: ConfigCommand,
    }

    #[test]
    fn config_arg_accepts_inline_json() {
        let v = read_config_arg(r#"{"region": "us-east-1"}"#).unwrap();
        assert_eq!(v["region"], "us-east-1");
    }

    #[test]
    fn config_arg_rejects_invalid_inline_json() {
        let err = read_config_arg("{not json").unwrap_err();
        assert!(err.contains("parse inline JSON"), "got: {err}");
    }

    #[test]
    fn config_arg_reports_unreadable_file() {
        let err = read_config_arg("@/nonexistent/seedling-test.json").unwrap_err();
        assert!(err.starts_with("read "), "got: {err}");
    }

    #[test]
    fn set_profile_name_and_clear_are_mutually_exclusive() {
        assert!(TestCli::try_parse_from(["test", "set-profile", "shortlived", "--clear"]).is_err());
        let cli = TestCli::try_parse_from(["test", "set-profile", "--clear"]).unwrap();
        let ConfigCommand::SetProfile { name, clear } = cli.cmd else {
            panic!("expected SetProfile");
        };
        assert!(clear);
        assert_eq!(name, None);
    }
}
