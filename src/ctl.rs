use std::{collections::HashMap, io::Write, net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use lloggs::LoggingArgs;
use seedling::oi::{
    client::{ClientAuth, ClientError, OiClient},
    keys::ClientIdentity,
};

#[path = "ctl/forward.rs"]
mod forward;
#[path = "ctl/known_hosts.rs"]
mod known_hosts;
#[path = "ctl/shell.rs"]
mod shell;
#[path = "ctl/subscribe.rs"]
mod subscribe;

#[derive(Parser)]
#[command(name = "seedling-ctl", about = "Seedling operator interface CLI")]
struct Cli {
    /// OI server address
    #[arg(long, default_value = "[::1]:7891")]
    endpoint: SocketAddr,

    /// SHA-256 SPKI fingerprint (hex) to pin
    #[arg(long, conflicts_with = "trust_any")]
    fingerprint: Option<String>,

    /// Skip server key verification (development only)
    #[arg(long, conflicts_with = "fingerprint")]
    trust_any: bool,

    #[command(flatten)]
    logging: LoggingArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show instance status
    Status,
    /// List registered apps
    ListApps,
    /// Describe an app
    DescribeApp { name: String },
    /// Register an app from a script file
    RegisterApp { name: String, script_file: PathBuf },
    /// Deregister an app
    DeregisterApp { name: String },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    UninstallApp { name: String },
    /// Update an app's script
    UpdateApp { name: String, script_file: PathBuf },
    /// Set a param value
    SetParam {
        app: String,
        name: String,
        value: String,
    },
    /// Unset a param value
    UnsetParam { app: String, name: String },
    /// Invoke a lifecycle action
    InvokeAction { app: String, name: String },
    /// Invoke the install action
    InvokeInstall {
        app: String,
        /// Requirements as key=value
        #[arg(long = "req")]
        requirements: Vec<String>,
    },
    /// List faults
    ListFaults {
        #[arg(long)]
        app: Option<String>,
    },
    /// List open shell sessions
    ListShells {
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop a shell session
    StopShell { session_id: String },
    /// List port forwards
    ListForwards {
        #[arg(long)]
        app: Option<String>,
    },
    /// Subscribe to event feed (streams JSON to stdout)
    Subscribe,
    /// Open an interactive shell session
    OpenShell { app: String, name: String },
    /// Forward a local port to a service
    ForwardPort {
        app: String,
        service: String,
        port: u16,
        #[arg(long)]
        proto: String,
        #[arg(long)]
        local_port: Option<u16>,
    },
    /// Print this client's key fingerprint (no server connection needed)
    PrintFingerprint,
    /// List authorized client keys on the server
    ListKeys,
    /// Authorize a client key on the server
    AuthorizeKey {
        /// Fingerprint to authorize
        fingerprint: String,
        /// Human-readable label for this key
        #[arg(long)]
        label: String,
    },
    /// Revoke an authorized client key on the server
    RevokeKey {
        /// Fingerprint to revoke
        fingerprint: String,
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

    // Load (or generate) the client identity early; PrintFingerprint needs it
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

    // Handle commands that don't need a server connection.
    if let Command::PrintFingerprint = &cli.command {
        println!("{}", identity.fingerprint);
        eprintln!("Client key: {}", key_path.display());
        eprintln!(
            "\nTo bootstrap a new server, add this line to $data_dir/authorized_keys:\n  {} my-label",
            identity.fingerprint
        );
        return;
    }

    let client;

    if cli.trust_any {
        client = OiClient::connect(cli.endpoint, ClientAuth::TrustAny, &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
    } else if let Some(fp) = cli.fingerprint {
        client = OiClient::connect(cli.endpoint, ClientAuth::Fingerprint(fp), &identity)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
    } else {
        let kh_path = known_hosts::KnownHosts::default_path();
        let mut kh = known_hosts::KnownHosts::load(&kh_path).unwrap_or_else(|e| {
            tracing::warn!("could not read {}: {e}", kh_path.display());
            known_hosts::KnownHosts::empty(kh_path.clone())
        });

        let (c, fp) = OiClient::connect_pinning(cli.endpoint, &identity)
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

        client = c;
    }

    dispatch(&client, cli.command).await;
}

async fn dispatch(client: &OiClient, cmd: Command) {
    match cmd {
        Command::PrintFingerprint => unreachable!("handled before connect"),
        Command::ListKeys => {
            print_result(client.request("ListKeys", serde_json::json!({})).await);
        }
        Command::AuthorizeKey { fingerprint, label } => {
            print_result(
                client
                    .request(
                        "AuthorizeKey",
                        serde_json::json!({ "fingerprint": fingerprint, "label": label }),
                    )
                    .await,
            );
        }
        Command::RevokeKey { fingerprint } => {
            print_result(
                client
                    .request(
                        "RevokeKey",
                        serde_json::json!({ "fingerprint": fingerprint }),
                    )
                    .await,
            );
        }
        Command::Status => {
            print_result(client.request("GetStatus", serde_json::json!({})).await);
        }
        Command::ListApps => {
            print_result(client.request("ListApps", serde_json::json!({})).await);
        }
        Command::DescribeApp { name } => {
            print_result(
                client
                    .request("DescribeApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::RegisterApp { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "RegisterApp",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        Command::DeregisterApp { name } => {
            print_result(
                client
                    .request("DeregisterApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::UninstallApp { name } => {
            print_result(
                client
                    .request("UninstallApp", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        Command::UpdateApp { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "UpdateApp",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        Command::SetParam { app, name, value } => {
            print_result(
                client
                    .request(
                        "SetParam",
                        serde_json::json!({ "app": app, "name": name, "value": value }),
                    )
                    .await,
            );
        }
        Command::UnsetParam { app, name } => {
            print_result(
                client
                    .request(
                        "UnsetParam",
                        serde_json::json!({ "app": app, "name": name }),
                    )
                    .await,
            );
        }
        Command::InvokeAction { app, name } => {
            print_result(
                client
                    .request(
                        "InvokeAction",
                        serde_json::json!({ "app": app, "name": name }),
                    )
                    .await,
            );
        }
        Command::InvokeInstall { app, requirements } => {
            let reqs: HashMap<String, String> = requirements
                .iter()
                .filter_map(|r| {
                    let mut parts = r.splitn(2, '=');
                    Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
                })
                .collect();
            print_result(
                client
                    .request(
                        "InvokeInstall",
                        serde_json::json!({ "app": app, "requirements": reqs }),
                    )
                    .await,
            );
        }
        Command::ListFaults { app } => {
            print_result(
                client
                    .request("ListFaults", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::ListShells { app } => {
            print_result(
                client
                    .request("ListShells", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::StopShell { session_id } => {
            print_result(
                client
                    .request("StopShell", serde_json::json!({ "session_id": session_id }))
                    .await,
            );
        }
        Command::ListForwards { app } => {
            print_result(
                client
                    .request("ListForwards", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        Command::Subscribe => {
            subscribe::subscribe(client).await;
        }
        Command::OpenShell { app, name } => {
            let code = shell::open_shell(client, app, name).await;
            std::process::exit(code);
        }
        Command::ForwardPort {
            app,
            service,
            port,
            proto,
            local_port,
        } => {
            forward::forward_port(client, app, service, port, proto, local_port).await;
        }
    }
}

fn print_result(result: Result<serde_json::Value, ClientError>) {
    match result {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    }
}

fn read_script_file(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!("cannot read {}: {e}", path.display());
        std::process::exit(1);
    })
}
