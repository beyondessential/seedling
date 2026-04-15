use std::{collections::HashMap, path::PathBuf};

use clap::Subcommand;
use seedling::oi::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum AppsCommand {
    /// List registered apps
    List,
    /// Describe an app
    Show { app: String },
    /// Register an app from a script file
    Create { app: String, script_file: PathBuf },
    /// Deregister an app
    Remove { app: String },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    Uninstall { app: String },
    /// Update an app's script
    Update { app: String, script_file: PathBuf },
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
    /// Stream container logs
    Logs {
        /// App name
        app: String,
        /// Resource name (optional filter)
        resource: Option<String>,
        /// Instance display-name suffix (requires resource)
        #[arg(long)]
        instance: Option<String>,
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
    /// Adjust deployment scale
    Scale {
        app: String,
        deployment: String,
        #[command(subcommand)]
        direction: ScaleDirection,
    },
    /// Forward a local port to a service
    Forward {
        app: String,
        service: String,
        port: u16,
        #[arg(long, default_value = "tcp")]
        proto: String,
        #[arg(long)]
        local_port: Option<u16>,
    },
}

#[derive(Subcommand)]
pub(super) enum ScaleDirection {
    /// Scale up by one instance
    Up,
    /// Scale down by one instance
    Down,
    /// Scale to the minimum (lower bound)
    ToMin,
    /// Scale to an exact instance count (clamped to bounds)
    To { count: u16 },
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
        AppsCommand::Show { app } => {
            print_result(
                client
                    .request("/apps/show", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        AppsCommand::Create { app, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "/apps/create",
                        serde_json::json!({ "app": app, "script": script }),
                    )
                    .await,
            );
        }
        AppsCommand::Remove { app } => {
            print_result(
                client
                    .request("/apps/remove", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        AppsCommand::Uninstall { app } => {
            print_result(
                client
                    .request("/apps/uninstall", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        AppsCommand::Update { app, script_file } => {
            let script = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "/apps/update",
                        serde_json::json!({ "app": app, "script": script }),
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
        AppsCommand::Logs {
            app,
            resource,
            instance,
            follow,
            tail,
            json,
        } => {
            let mut params = serde_json::json!({
                "app": app,
                "follow": follow,
                "tail": tail,
            });
            if let Some(r) = resource {
                params["resource"] = serde_json::Value::String(r);
            }
            if let Some(i) = instance {
                params["instance"] = serde_json::Value::String(i);
            }
            super::logs::stream_logs(client, params, json, follow).await;
        }
        AppsCommand::Scale {
            app,
            deployment,
            direction,
        } => {
            let scale = match direction {
                ScaleDirection::To { count } => count,
                relative => {
                    // Fetch current scale info from /apps/show.
                    let show = client
                        .request("/apps/show", serde_json::json!({ "app": app }))
                        .await;
                    let info = match show {
                        Ok(v) => v,
                        Err(e) => {
                            print_result(Err(e));
                            return;
                        }
                    };
                    let resource = info["resources"]
                        .as_array()
                        .and_then(|rs| rs.iter().find(|r| r["name"].as_str() == Some(&deployment)));
                    let scale_obj = match resource.and_then(|r| r.get("scale")) {
                        Some(s) => s,
                        None => {
                            eprintln!(
                                "error: deployment {deployment:?} not found or has no scale info"
                            );
                            std::process::exit(1);
                        }
                    };
                    let current = scale_obj["current"].as_u64().unwrap_or(0) as u16;
                    let low = scale_obj["low"].as_u64().unwrap_or(0) as u16;
                    let high = scale_obj["high"].as_u64().unwrap_or(0) as u16;
                    match relative {
                        ScaleDirection::Up => current.saturating_add(1).min(high),
                        ScaleDirection::Down => current.saturating_sub(1).max(low),
                        ScaleDirection::ToMin => low,
                        ScaleDirection::To { .. } => unreachable!(),
                    }
                }
            };
            print_result(
                client
                    .request(
                        "/apps/scale",
                        serde_json::json!({ "app": app, "deployment": deployment, "scale": scale }),
                    )
                    .await,
            );
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
