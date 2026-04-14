use std::net::SocketAddr;

use clap::Subcommand;
use seedling::oi::{client::OiClient, keys::ClientIdentity};

use super::print_result;

#[derive(Subcommand)]
pub(super) enum OpCommand {
    /// Show instance status
    Status,
    /// List faults
    Faults {
        #[arg(long)]
        app: Option<String>,
    },
    /// Shell session management
    Shells {
        #[command(subcommand)]
        command: ShellsCommand,
    },
    /// Port forward management
    Forwards {
        #[command(subcommand)]
        command: ForwardsCommand,
    },
    /// Stream infrastructure logs
    Logs {
        /// Infrastructure component: "proxy" or "resolver"
        infra: String,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Number of historical lines
        #[arg(long, default_value = "100")]
        tail: u64,
        /// Print raw JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Subscribe to event feed (streams JSON to stdout)
    Events,
    /// Container registry allowlist
    Registries {
        #[command(subcommand)]
        command: RegistriesCommand,
    },
    /// User/key management
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum ShellsCommand {
    /// List open shell sessions
    List {
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop a shell session
    Stop { session_id: String },
}

#[derive(Subcommand)]
pub(super) enum ForwardsCommand {
    /// List port forwards
    List {
        #[arg(long)]
        app: Option<String>,
    },
    /// Stop a port forward from the server side
    Stop { forward_id: String },
}

#[derive(Subcommand)]
pub(super) enum RegistriesCommand {
    /// List allowed registries
    List,
    /// Add a registry to the allowlist
    Add { registry: String },
    /// Remove a registry from the allowlist
    Remove { registry: String },
}

#[derive(Subcommand)]
pub(super) enum UserCommand {
    /// List authorized client keys
    List,
    /// Authorize a client key
    Add { fingerprint: String, label: String },
    /// Revoke an authorized client key
    Remove {
        /// Fingerprint to revoke
        fingerprint: String,
    },
}

pub(super) async fn dispatch(
    client: &OiClient,
    cmd: OpCommand,
    endpoint: SocketAddr,
    fingerprint: &str,
    identity: &ClientIdentity,
) {
    match cmd {
        OpCommand::Status => {
            print_result(
                client
                    .request("/server/status", serde_json::json!({}))
                    .await,
            );
        }
        OpCommand::Faults { app } => {
            print_result(
                client
                    .request("/faults/list", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        OpCommand::Shells { command } => match command {
            ShellsCommand::List { app } => {
                print_result(
                    client
                        .request("/shells/list", serde_json::json!({ "app": app }))
                        .await,
                );
            }
            ShellsCommand::Stop { session_id } => {
                print_result(
                    client
                        .request(
                            "/shells/stop",
                            serde_json::json!({ "session_id": session_id }),
                        )
                        .await,
                );
            }
        },
        OpCommand::Forwards { command } => match command {
            ForwardsCommand::List { app } => {
                print_result(
                    client
                        .request("/forwards/list", serde_json::json!({ "app": app }))
                        .await,
                );
            }
            ForwardsCommand::Stop { forward_id } => {
                print_result(
                    client
                        .request(
                            "/forwards/stop",
                            serde_json::json!({ "forward_id": forward_id }),
                        )
                        .await,
                );
            }
        },
        OpCommand::Registries { command } => match command {
            RegistriesCommand::List => {
                print_result(
                    client
                        .request("/registries/list", serde_json::json!({}))
                        .await,
                );
            }
            RegistriesCommand::Add { registry } => {
                print_result(
                    client
                        .request(
                            "/registries/add",
                            serde_json::json!({ "registry": registry }),
                        )
                        .await,
                );
            }
            RegistriesCommand::Remove { registry } => {
                print_result(
                    client
                        .request(
                            "/registries/remove",
                            serde_json::json!({ "registry": registry }),
                        )
                        .await,
                );
            }
        },
        OpCommand::Logs {
            infra,
            follow,
            tail,
            json,
        } => {
            let params = serde_json::json!({
                "infra": infra,
                "follow": follow,
                "tail": tail,
            });
            super::logs::stream_logs(client, params, json, follow).await;
        }
        OpCommand::Events => {
            super::subscribe::subscribe(endpoint, fingerprint.to_owned(), identity).await;
        }
        OpCommand::User { command } => match command {
            UserCommand::List => {
                print_result(client.request("/keys/list", serde_json::json!({})).await);
            }
            UserCommand::Add { fingerprint, label } => {
                print_result(
                    client
                        .request(
                            "/keys/authorise",
                            serde_json::json!({ "fingerprint": fingerprint, "label": label }),
                        )
                        .await,
                );
            }
            UserCommand::Remove { fingerprint } => {
                print_result(
                    client
                        .request(
                            "/keys/revoke",
                            serde_json::json!({ "fingerprint": fingerprint }),
                        )
                        .await,
                );
            }
        },
    }
}
