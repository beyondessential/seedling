use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use lloggs::LoggingArgs;
use seedling::oi::client::{ClientAuth, ClientError, OiClient};

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
    /// Update an app's script
    UpdateApp { name: String, script_file: PathBuf },
    /// Set a param value
    SetParam {
        app: String,
        name: String,
        value: String,
    },
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

    let auth = if cli.trust_any {
        ClientAuth::TrustAny
    } else if let Some(fp) = cli.fingerprint {
        ClientAuth::Fingerprint(fp)
    } else {
        tracing::error!("--fingerprint <hex> or --trust-any is required");
        std::process::exit(1);
    };

    let client = OiClient::connect(cli.endpoint, auth)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("{e}");
            std::process::exit(1);
        });

    dispatch(&client, cli.command).await;
}

async fn dispatch(client: &OiClient, cmd: Command) {
    match cmd {
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
            todo!("Subscribe requires server-initiated unidirectional streams (Phase 8)")
        }
        Command::OpenShell { .. } => {
            todo!("OpenShell requires terminal raw mode and multiplexed streams (Phase 5)")
        }
        Command::ForwardPort { .. } => {
            todo!("ForwardPort requires local TCP/UDP listener and QUIC relay (Phase 6)")
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
