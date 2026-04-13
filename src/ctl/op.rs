use clap::Subcommand;
use seedling::oi::client::OiClient;

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
    /// /events/subscribe to event feed (streams JSON to stdout)
    Events,
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
pub(super) enum UserCommand {
    /// List authorized client keys
    List,
    /// Authorize a client key
    Add {
        /// Fingerprint to authorize
        fingerprint: String,
        /// Human-readable label for this key
        #[arg(long)]
        label: String,
    },
    /// Revoke an authorized client key
    Remove {
        /// Fingerprint to revoke
        fingerprint: String,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: OpCommand) {
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
        OpCommand::Events => {
            super::subscribe::subscribe(client).await;
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
