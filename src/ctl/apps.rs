use std::{collections::HashMap, path::PathBuf};

use clap::Subcommand;
use seedling::oi::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum AppsCommand {
    /// List registered apps
    List,
    /// Describe an app
    Show { name: String },
    /// Register an app from a script file
    Create { name: String, script_file: PathBuf },
    /// Deregister an app
    Remove { name: String },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    Uninstall { name: String },
    /// Update an app's script
    Update { name: String, script_file: PathBuf },
    /// Manage app parameters
    Param {
        #[command(subcommand)]
        command: ParamCommand,
    },
    /// Invoke a lifecycle action
    Action { app: String, name: String },
    /// Invoke the install action
    Install {
        app: String,
        /// Requirements as key=value
        #[arg(long = "req")]
        requirements: Vec<String>,
    },
    /// Open an interactive shell session
    Shell { app: String, name: String },
    /// Forward a local port to a service
    Forward {
        app: String,
        service: String,
        port: u16,
        #[arg(long)]
        proto: String,
        #[arg(long)]
        local_port: Option<u16>,
    },
}

#[derive(Subcommand)]
pub(super) enum ParamCommand {
    /// Set a param value
    Set {
        app: String,
        name: String,
        value: String,
    },
    /// Unset a param value
    Unset { app: String, name: String },
}

pub(super) async fn dispatch(client: &OiClient, cmd: AppsCommand) {
    match cmd {
        AppsCommand::List => {
            print_result(client.request("/apps/list", serde_json::json!({})).await);
        }
        AppsCommand::Show { name } => {
            print_result(
                client
                    .request("/apps/show", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        AppsCommand::Create { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "/apps/create",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        AppsCommand::Remove { name } => {
            print_result(
                client
                    .request("/apps/remove", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        AppsCommand::Uninstall { name } => {
            print_result(
                client
                    .request("/apps/uninstall", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        AppsCommand::Update { name, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "/apps/update",
                        serde_json::json!({ "name": name, "script": script }),
                    )
                    .await,
            );
        }
        AppsCommand::Param { command } => match command {
            ParamCommand::Set { app, name, value } => {
                print_result(
                    client
                        .request(
                            "/apps/params/set",
                            serde_json::json!({ "app": app, "name": name, "value": value }),
                        )
                        .await,
                );
            }
            ParamCommand::Unset { app, name } => {
                print_result(
                    client
                        .request(
                            "/apps/params/unset",
                            serde_json::json!({ "app": app, "name": name }),
                        )
                        .await,
                );
            }
        },
        AppsCommand::Action { app, name } => {
            print_result(
                client
                    .request(
                        "/apps/action/invoke",
                        serde_json::json!({ "app": app, "name": name }),
                    )
                    .await,
            );
        }
        AppsCommand::Install { app, requirements } => {
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
                        "/apps/install/invoke",
                        serde_json::json!({ "app": app, "requirements": reqs }),
                    )
                    .await,
            );
        }
        AppsCommand::Shell { app, name } => {
            let code = super::shell::open_shell(client, app, name).await;
            std::process::exit(code);
        }
        AppsCommand::Forward {
            app,
            service,
            port,
            proto,
            local_port,
        } => {
            super::forward::forward_port(client, app, service, port, proto, local_port).await;
        }
    }
}

fn read_script_file(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!("cannot read {}: {e}", path.display());
        std::process::exit(1);
    })
}
