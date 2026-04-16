use std::net::SocketAddr;

use clap::Subcommand;
use seedling::oi::{client::OiClient, keys::ClientIdentity};

use super::print_result;

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
    Remove { fingerprint: String },
}

#[derive(Subcommand)]
pub(super) enum ProxyCommand {
    /// Stream proxy logs
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(long, default_value = "100")]
        tail: u64,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(super) enum DnsCommand {
    /// Stream DNS resolver logs
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(long, default_value = "100")]
        tail: u64,
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn dispatch_shells(client: &OiClient, cmd: ShellsCommand) {
    match cmd {
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
    }
}

pub(super) async fn dispatch_forwards(client: &OiClient, cmd: ForwardsCommand) {
    match cmd {
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
    }
}

pub(super) async fn dispatch_registries(client: &OiClient, cmd: RegistriesCommand) {
    match cmd {
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
    }
}

pub(super) async fn dispatch_user(client: &OiClient, cmd: UserCommand) {
    match cmd {
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
    }
}

pub(super) async fn dispatch_proxy(client: &OiClient, cmd: ProxyCommand) {
    match cmd {
        ProxyCommand::Logs { follow, tail, json } => {
            let params = serde_json::json!({
                "infra": "proxy",
                "follow": follow,
                "tail": tail,
            });
            super::logs::stream_logs(client, params, json, follow).await;
        }
    }
}

pub(super) async fn dispatch_dns(client: &OiClient, cmd: DnsCommand) {
    match cmd {
        DnsCommand::Logs { follow, tail, json } => {
            let params = serde_json::json!({
                "infra": "resolver",
                "follow": follow,
                "tail": tail,
            });
            super::logs::stream_logs(client, params, json, follow).await;
        }
    }
}

pub(super) async fn dispatch_events(
    endpoint: SocketAddr,
    fingerprint: String,
    identity: &ClientIdentity,
) {
    super::subscribe::subscribe(endpoint, fingerprint, identity).await;
}
