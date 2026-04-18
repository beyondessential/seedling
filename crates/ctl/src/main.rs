use std::{io::Write, net::SocketAddr};

use clap::{CommandFactory, Parser, Subcommand};
use lloggs::LoggingArgs;
use seedling_protocol::{
    client::{ClientAuth, ClientError, OiClient},
    keys::ClientIdentity,
};

mod apps;
mod backups;
mod client;
mod forward;
mod known_hosts;
mod logs;
mod op;
mod shell;
mod subscribe;
mod volumes;

#[derive(Parser)]
#[command(name = "seedling-ctl", about = "Seedling operator interface CLI")]
struct Cli {
    /// OI server address
    #[arg(long, default_value = "[::1]:7891")]
    endpoint: SocketAddr,

    /// SHA-256 SPKI fingerprint (hex) to pin
    #[arg(long)]
    #[cfg_attr(debug_assertions, arg(conflicts_with = "trust_any"))]
    fingerprint: Option<String>,

    /// Skip server key verification (development only)
    #[cfg(debug_assertions)]
    #[arg(long, conflicts_with = "fingerprint")]
    trust_any: bool,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Manage apps
    Apps {
        #[command(subcommand)]
        command: apps::AppsCommand,
    },
    /// Volume management (held, site)
    Volumes {
        #[command(subcommand)]
        command: volumes::VolumesCommand,
    },
    /// Proxy management
    Proxy {
        #[command(subcommand)]
        command: op::ProxyCommand,
    },
    /// DNS resolver management
    Dns {
        #[command(subcommand)]
        command: op::DnsCommand,
    },
    /// Shell session management
    Shells {
        #[command(subcommand)]
        command: op::ShellsCommand,
    },
    /// Port forward management
    Forwards {
        #[command(subcommand)]
        command: op::ForwardsCommand,
    },
    /// Container registry allowlist
    Registries {
        #[command(subcommand)]
        command: op::RegistriesCommand,
    },
    /// Backup app management
    Backups {
        #[command(subcommand)]
        command: backups::BackupsCommand,
    },
    /// User/key management
    User {
        #[command(subcommand)]
        command: op::UserCommand,
    },
    /// Show server status
    Status,
    /// List faults
    Faults {
        #[arg(long)]
        app: Option<String>,
    },
    /// Subscribe to event feed (streams JSON to stdout)
    Events,
    /// Client info (fingerprint)
    Client {
        #[command(subcommand)]
        command: client::ClientCommand,
    },
}

#[tokio::main]
async fn main() {
    let mut _guard = lloggs::PreArgs::parse_with_env("SEEDLING_LOG")
        .setup()
        .unwrap_or_else(|e| {
            tracing::warn!("logging setup: {e}");
            None
        });

    let cli = Cli::parse();

    if _guard.is_none() {
        _guard = cli
            .logging
            .setup(|v| match v {
                0 => "seedling=info,seedling_ctl=info,warn",
                1 => "seedling=debug,seedling_ctl=debug,warn",
                2 => "info",
                3 => "seedling=debug,seedling_ctl=debug,info",
                4 => "debug",
                5 => "seedling=trace,seedling_ctl=trace,debug",
                _ => "trace",
            })
            .map(Some)
            .unwrap_or_else(|e| {
                tracing::warn!("logging setup: {e}");
                None
            });
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("ring crypto provider already installed");

    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        tracing::warn!("SSLKEYLOGFILE is set — TLS session keys are being logged to disk");
    }

    // Load (or generate) the client identity early; `client fingerprint` needs it
    // before any server connection is attempted.
    let key_path = ClientIdentity::default_path();
    let (identity, is_new) = ClientIdentity::load_or_generate(&key_path).unwrap_or_else(|e| {
        tracing::error!(
            "could not load/generate client key at {}: {e}",
            key_path.display()
        );
        std::process::exit(1);
    });
    if is_new {
        tracing::info!(
            path = %key_path.display(),
            fingerprint = %identity.fingerprint,
            "generated new client key"
        );
    }

    let top_cmd = match cli.command {
        Some(cmd) => cmd,
        None => {
            print_tree(&Cli::command());
            return;
        }
    };

    if let Command::Client { command } = &top_cmd {
        client::dispatch(command, &identity, &key_path);
        return;
    }

    let client;

    #[cfg(debug_assertions)]
    let trust_any = cli.trust_any;
    #[cfg(not(debug_assertions))]
    let trust_any = false;

    let resolved_fingerprint: String;

    if trust_any {
        resolved_fingerprint = String::new();
        client = OiClient::connect(cli.endpoint, ClientAuth::TrustAny, &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
    } else if let Some(fp) = cli.fingerprint {
        client = OiClient::connect(cli.endpoint, ClientAuth::Fingerprint(fp.clone()), &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
        resolved_fingerprint = fp;
    } else {
        let kh_path = known_hosts::KnownHosts::default_path();
        let mut kh = known_hosts::KnownHosts::load(&kh_path).unwrap_or_else(|e| {
            tracing::warn!("could not read {}: {e}", kh_path.display());
            known_hosts::KnownHosts::empty(kh_path.clone())
        });

        let fp = OiClient::probe_fingerprint(cli.endpoint)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });

        let ep = cli.endpoint.to_string();
        match kh.check(&ep, &fp) {
            known_hosts::Status::Match => {}
            known_hosts::Status::Unknown => {
                let mut stderr = std::io::stderr();
                writeln!(
                    stderr,
                    "The authenticity of host '{ep}' can't be established."
                )
                .ok();
                writeln!(stderr, "Fingerprint: {fp}").ok();
                write!(stderr, "Continue connecting? (yes/no) ").ok();
                stderr.flush().ok();

                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                if line.trim() != "yes" {
                    eprintln!("Aborted.");
                    std::process::exit(1);
                }

                kh.add(&ep, &fp);
                match kh.save() {
                    Ok(()) => eprintln!(
                        "Permanently added '{ep}' to known hosts ({}).",
                        kh_path.display()
                    ),
                    Err(e) => tracing::warn!("could not save known hosts: {e}"),
                }
            }
            known_hosts::Status::Mismatch { expected } => {
                let bar = "@".repeat(60);
                eprintln!("{bar}");
                eprintln!("@ WARNING: REMOTE HOST FINGERPRINT HAS CHANGED!            @");
                eprintln!("{bar}");
                eprintln!("Someone could be eavesdropping on you right now!");
                eprintln!("Expected fingerprint for '{ep}':");
                eprintln!("  {expected}");
                eprintln!("Received:");
                eprintln!("  {fp}");
                eprintln!(
                    "Remove the stale entry from {} to proceed.",
                    kh_path.display()
                );
                std::process::exit(1);
            }
        }

        client = OiClient::connect(cli.endpoint, ClientAuth::Fingerprint(fp.clone()), &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
        resolved_fingerprint = fp;
    }

    match top_cmd {
        Command::Apps { command } => apps::dispatch(&client, command).await,
        Command::Volumes { command } => volumes::dispatch(&client, command).await,
        Command::Proxy { command } => op::dispatch_proxy(&client, command).await,
        Command::Dns { command } => op::dispatch_dns(&client, command).await,
        Command::Shells { command } => op::dispatch_shells(&client, command).await,
        Command::Forwards { command } => op::dispatch_forwards(&client, command).await,
        Command::Registries { command } => op::dispatch_registries(&client, command).await,
        Command::Backups { command } => backups::dispatch(&client, command).await,
        Command::User { command } => op::dispatch_user(&client, command).await,
        Command::Status => {
            print_result(
                client
                    .request("/server/status", serde_json::json!({}))
                    .await,
            );
        }
        Command::Faults { app } => {
            print_result(
                client
                    .request("/faults/list", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::Events => {
            op::dispatch_events(cli.endpoint, resolved_fingerprint, &identity).await;
        }
        Command::Client { .. } => unreachable!("handled before connect"),
    }
}

pub(crate) fn print_result(result: Result<serde_json::Value, ClientError>) {
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    }
}

fn print_tree(root: &clap::Command) {
    let mut top: Vec<_> = root
        .get_subcommands()
        .filter(|s| !s.is_hide_set())
        .collect();
    top.sort_by_key(|s| s.get_name());
    for sub in top {
        print_subtree(sub, 0);
    }
}

fn print_subtree(cmd: &clap::Command, indent: usize) {
    let pad = "  ".repeat(indent);
    let mut subs: Vec<_> = cmd.get_subcommands().filter(|s| !s.is_hide_set()).collect();
    subs.sort_by_key(|s| s.get_name());

    if subs.is_empty() {
        let args = leaf_args(cmd);
        if args.is_empty() {
            println!("{}{}", pad, cmd.get_name());
        } else {
            println!("{}{} {}", pad, cmd.get_name(), args);
        }
    } else {
        println!("{}{}", pad, cmd.get_name());
        for sub in subs {
            print_subtree(sub, indent + 1);
        }
    }
}

fn leaf_args(cmd: &clap::Command) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Required named options, excluding clap builtins.
    for arg in cmd.get_opts() {
        let id = arg.get_id().as_str();
        if matches!(id, "help" | "version") {
            continue;
        }
        if arg.is_required_set() {
            let long = arg.get_long().unwrap_or(id);
            let val = arg
                .get_value_names()
                .and_then(|v| v.first())
                .map(|n| format!(" <{n}>"))
                .unwrap_or_default();
            parts.push(format!("--{long}{val}"));
        }
    }

    // Required positionals in declaration order.
    for arg in cmd.get_positionals() {
        if !arg.is_required_set() {
            continue;
        }
        let name = arg
            .get_value_names()
            .and_then(|v| v.first())
            .map(|n| format!("<{n}>"))
            .unwrap_or_else(|| format!("<{}>", arg.get_id().as_str().to_uppercase()));
        parts.push(name);
    }

    parts.join(" ")
}
