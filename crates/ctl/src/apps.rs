use std::{collections::HashMap, path::PathBuf};

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use seedling_protocol::names::{ActionName, AppName};

use super::print_result;

// i[ctl.action.params]
// i[ctl.shell.params]
fn parse_positional_params(args: &[String]) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            map.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
        } else {
            map.insert(arg.clone(), serde_json::Value::Bool(true));
        }
    }
    map
}

#[derive(Subcommand)]
pub(super) enum AppsCommand {
    /// Manage volumes for an app
    Volumes {
        #[command(subcommand)]
        command: VolumesCommand,
    },
    /// List registered apps
    List,
    /// Describe an app
    Show { app: AppName },
    /// Register an app from a script file
    Create { app: AppName, script_file: PathBuf },
    /// Deregister an app
    Remove { app: AppName },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    Uninstall { app: AppName },
    /// Update an app's script
    Update { app: AppName, script_file: PathBuf },
    /// Manage app parameters
    Param {
        #[command(subcommand)]
        command: ParamCommand,
    },
    /// Invoke a lifecycle action
    Action {
        app: AppName,
        name: ActionName,
        /// Params as key[=value] (bare key maps to true)
        #[arg(trailing_var_arg = true)]
        params: Vec<String>,
    },
    /// Cancel the in-flight lifecycle operation for an app
    // i[impl ctl.action.cancel]
    CancelAction { app: AppName },
    /// Invoke the install action
    Install {
        app: AppName,
        /// Params as key=value (trailing positional args)
        #[arg(trailing_var_arg = true)]
        params: Vec<String>,
    },
    /// Open an interactive shell session
    Shell {
        app: AppName,
        name: String,
        /// Params as key[=value] (bare key maps to true)
        #[arg(trailing_var_arg = true)]
        params: Vec<String>,
    },
    /// Stream container logs
    Logs {
        /// App name
        app: AppName,
        /// Resource name (optional filter)
        resource: Option<String>,
        /// Instance display-name suffix (requires resource)
        #[arg(short, long)]
        instance: Option<String>,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Number of historical lines
        #[arg(short = 'n', long = "lines", default_value = "10")]
        tail: u64,
        /// Print raw JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Adjust deployment scale
    Scale {
        app: AppName,
        deployment: String,
        #[command(subcommand)]
        direction: ScaleDirection,
    },
    /// Restart a deployment (follows its update strategy without changing config)
    Restart { app: AppName, deployment: String },
    /// Stop a resource (scale deployments to zero, unschedule jobs/ingresses)
    StopResource {
        app: AppName,
        /// Resource kind: deployment, job, or ingress
        kind: String,
        /// Resource name
        name: String,
    },
    /// Unstop a previously stopped resource
    UnstopResource {
        app: AppName,
        kind: String,
        name: String,
    },
    /// Unstop all stopped resources for an app
    Unstop { app: AppName },
    /// Forward a local port to a service
    Forward {
        app: AppName,
        service: String,
        port: u16,
        #[arg(long, default_value = "tcp")]
        proto: String,
        #[arg(long)]
        local_port: Option<u16>,
    },
    /// Get the script for an app (current generation by default)
    Script {
        app: AppName,
        /// Specific generation to fetch
        #[arg(long)]
        generation: Option<u64>,
    },
    /// List the generation history for an app
    Generations {
        app: AppName,
        /// Maximum number of entries to return (1-200, default 50)
        #[arg(long)]
        limit: Option<usize>,
        /// Only show entries with generation strictly less than this value
        #[arg(long)]
        before: Option<u64>,
    },
    /// Dry-run a hypothetical change against the current generation
    Plan {
        app: AppName,
        /// Path to a proposed script file
        #[arg(long = "script")]
        proposed_script_file: Option<PathBuf>,
        /// Proposed param change as `name=value` (repeatable). Use `name=` to
        /// model unsetting.
        #[arg(long = "param")]
        proposed_params: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(super) enum VolumesCommand {
    /// List exported volumes and external volume mappings for an app
    List { app: AppName },
    /// Attach an external volume to a target (_site/name or app/name)
    Attach {
        /// App declaring the external volume
        app: AppName,
        /// External volume name (as declared in BSL with app.external_volume())
        external_volume: String,
        /// Target volume ID: _site/<name> or <app>/<volume>
        vol_id: String,
        /// Mount as read-only
        #[arg(long)]
        read_only: bool,
        /// Remap if already attached
        #[arg(long)]
        force: bool,
    },
    /// Detach an external volume mapping
    Detach {
        /// App declaring the external volume
        app: AppName,
        /// External volume name
        external_volume: String,
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
        app: AppName,
        name: String,
        value: String,
    },
    /// Unset a param value
    Unset { app: AppName, name: String },
}

fn parse_vol_id(vol_id: &str) -> Result<(&str, &str), String> {
    let (prefix, vol) = vol_id.split_once('/').ok_or_else(|| {
        format!("invalid volume ID {vol_id:?}: expected _site/<name> or <app>/<volume>")
    })?;
    if prefix.is_empty() || vol.is_empty() {
        return Err(format!(
            "invalid volume ID {vol_id:?}: neither part may be empty"
        ));
    }
    Ok((prefix, vol))
}

pub(super) async fn dispatch(client: &OiClient, cmd: AppsCommand) {
    match cmd {
        AppsCommand::Volumes { command } => match command {
            VolumesCommand::List { app } => {
                print_result(
                    client
                        .request("/apps/show", serde_json::json!({ "app": app }))
                        .await
                        .map(|v| {
                            let resources = v["resources"].as_array().cloned().unwrap_or_default();
                            let vols: Vec<_> = resources
                                .iter()
                                .filter(|r| {
                                    r["type"] == "externalvolume"
                                        || (r["type"] == "volume" && r.get("export").is_some())
                                })
                                .cloned()
                                .collect();
                            serde_json::Value::Array(vols)
                        }),
                );
            }
            VolumesCommand::Attach {
                app,
                external_volume,
                vol_id,
                read_only,
                force,
            } => {
                let (prefix, vol) = match parse_vol_id(&vol_id) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                };
                let target = if prefix == "_site" {
                    serde_json::json!({ "kind": "site", "name": vol })
                } else {
                    serde_json::json!({ "kind": "app", "app": prefix, "volume": vol })
                };
                let params = serde_json::json!({
                    "app": app,
                    "external_name": external_volume,
                    "target": target,
                    "read_only": read_only,
                });
                let route = if force {
                    "/volumes/external/remap"
                } else {
                    "/volumes/external/map"
                };
                print_result(client.request(route, params).await);
            }
            VolumesCommand::Detach {
                app,
                external_volume,
            } => {
                print_result(
                    client
                        .request(
                            "/volumes/external/unmap",
                            serde_json::json!({
                                "app": app,
                                "external_name": external_volume,
                            }),
                        )
                        .await,
                );
            }
        },
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
            // i[impl ctl.backup.app.hint]
            let looks_like_backup_app = seedling_protocol::backup_actions::REQUIRED_ACTIONS
                .iter()
                .all(|a| script.contains(a));
            print_result(
                client
                    .request(
                        "/apps/create",
                        serde_json::json!({ "app": app, "script": script }),
                    )
                    .await,
            );
            if looks_like_backup_app {
                tracing::info!(
                    "this app looks like a backup app; \
                     register it with: ctl backups apps register --name <name> --app {app}"
                );
            }
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
        AppsCommand::Action { app, name, params } => {
            let action_params = parse_positional_params(&params);
            let mut req = serde_json::json!({ "app": app, "name": name });
            if !action_params.is_empty() {
                req["params"] = serde_json::Value::Object(action_params);
            }
            print_result(client.request("/apps/action/invoke", req).await);
        }
        // i[impl ctl.action.cancel]
        AppsCommand::CancelAction { app } => {
            print_result(
                client
                    .request("/apps/action/cancel", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        AppsCommand::Install { app, params } => {
            let submitted: HashMap<String, String> = params
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
                        serde_json::json!({ "app": app, "params": submitted }),
                    )
                    .await,
            );
        }
        AppsCommand::Shell { app, name, params } => {
            let shell_params = parse_positional_params(&params);
            let code = super::shell::open_shell(client, app, name, shell_params).await;
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
        AppsCommand::Restart { app, deployment } => {
            print_result(
                client
                    .request(
                        "/apps/restart",
                        serde_json::json!({ "app": app, "deployment": deployment }),
                    )
                    .await,
            );
        }
        AppsCommand::StopResource { app, kind, name } => {
            print_result(
                client
                    .request(
                        "/apps/resource/stop",
                        serde_json::json!({ "app": app, "kind": kind, "name": name }),
                    )
                    .await,
            );
        }
        AppsCommand::UnstopResource { app, kind, name } => {
            print_result(
                client
                    .request(
                        "/apps/resource/unstop",
                        serde_json::json!({ "app": app, "kind": kind, "name": name }),
                    )
                    .await,
            );
        }
        AppsCommand::Unstop { app } => {
            print_result(
                client
                    .request("/apps/unstop", serde_json::json!({ "app": app }))
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
        AppsCommand::Script { app, generation } => {
            let mut params = serde_json::json!({ "app": app });
            if let Some(g) = generation {
                params["generation"] = serde_json::json!(g);
            }
            print_result(client.request("/apps/script", params).await);
        }
        AppsCommand::Generations { app, limit, before } => {
            let mut params = serde_json::json!({ "app": app });
            if let Some(l) = limit {
                params["limit"] = serde_json::json!(l);
            }
            if let Some(b) = before {
                params["before"] = serde_json::json!(b);
            }
            print_result(client.request("/apps/generations", params).await);
        }
        AppsCommand::Plan {
            app,
            proposed_script_file,
            proposed_params,
        } => {
            let mut params = serde_json::json!({ "app": app });
            if let Some(path) = proposed_script_file {
                let script = read_script_file(&path);
                params["proposed_script"] = serde_json::json!(script);
            }
            if !proposed_params.is_empty() {
                let parsed: Vec<serde_json::Value> = proposed_params
                    .iter()
                    .map(|spec| match spec.split_once('=') {
                        Some((name, "")) => {
                            serde_json::json!({ "name": name, "value": serde_json::Value::Null })
                        }
                        Some((name, value)) => serde_json::json!({ "name": name, "value": value }),
                        None => {
                            serde_json::json!({ "name": spec, "value": serde_json::Value::Null })
                        }
                    })
                    .collect();
                params["proposed_params"] = serde_json::Value::Array(parsed);
            }
            print_result(client.request("/apps/plan", params).await);
        }
    }
}

fn read_script_file(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!("cannot read {}: {e}", path.display());
        std::process::exit(1);
    })
}
