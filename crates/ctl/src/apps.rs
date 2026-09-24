use std::{collections::HashMap, path::PathBuf};

use clap::{Subcommand, ValueEnum};
use seedling_protocol::client::OiClient;
use seedling_protocol::names::{ActionName, AppName};

use super::{
    definition::{self, DefinitionArgs, resolve_or_exit},
    print_result,
};

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
    /// Register an app from a script file, a definition folder, a GitHub
    /// folder URL, or an OCI reference
    Create {
        app: AppName,
        #[command(flatten)]
        definition: DefinitionArgs,
    },
    /// Deregister an app
    Remove { app: AppName },
    /// Uninstall an app (stop all resources). The app can be deregistered once done.
    Uninstall { app: AppName },
    /// Replace an app's definition, optionally changing one parameter in
    /// the same step
    Update {
        app: AppName,
        #[command(flatten)]
        definition: DefinitionArgs,
        /// Set a parameter alongside the definition, as name=value
        #[arg(long, value_name = "NAME=VALUE", conflicts_with = "unset")]
        set: Option<String>,
        /// Unset a parameter alongside the definition
        #[arg(long, value_name = "NAME")]
        unset: Option<String>,
    },
    /// Write an app's definition bundle into a new folder
    Export {
        app: AppName,
        folder: PathBuf,
        /// Generation to export (the current one by default)
        #[arg(long)]
        generation: Option<u64>,
    },
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
    /// Set an app's priority
    Priority {
        app: AppName,
        /// Standing against other apps when the host is under pressure
        priority: PriorityLevel,
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
        /// A proposed script file, definition folder, or GitHub folder URL
        #[arg(long = "script", conflicts_with = "proposed_reference")]
        proposed_script_file: Option<String>,
        /// A proposed definition, fetched from this OCI reference
        #[arg(long = "ref")]
        proposed_reference: Option<String>,
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

/// The levels `/apps/priority` accepts. Spelled out here so a typo is refused
/// by the parser with the alternatives listed, rather than making a round trip
/// to the daemon to come back as `requirements_invalid`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum PriorityLevel {
    High,
    Normal,
    Low,
}

impl PriorityLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
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

/// Parse `key=value` install params.
///
/// Unlike action and shell params, these carry values typed by the app's
/// parameter schema rather than free-form JSON, so a bare key has no sensible
/// reading — `true` is the wrong type and `""` is a guess at intent. It used
/// to be dropped by a `filter_map` with no warning, so an operator who wrote
/// `db-pass` instead of `db-pass=secret` got an install missing a parameter
/// and no indication why.
// i[impl ctl.install.params]
fn parse_install_params(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for arg in args {
        let Some((key, value)) = arg.split_once('=') else {
            return Err(format!("invalid install param {arg:?}: expected key=value"));
        };
        map.insert(key.to_owned(), value.to_owned());
    }
    Ok(map)
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
        AppsCommand::Create { app, definition } => {
            let definition = require_definition(&definition).await;
            // i[impl ctl.backup.app.hint]
            let looks_like_backup_app = definition.script_text().is_some_and(|script| {
                seedling_protocol::backup_actions::REQUIRED_ACTIONS
                    .iter()
                    .all(|a| script.contains(a))
            });
            let mut params = definition.fields("script");
            params.insert("app".to_owned(), serde_json::json!(app));
            print_result(
                client
                    .request("/apps/create", serde_json::Value::Object(params))
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
        AppsCommand::Update {
            app,
            definition,
            set,
            unset,
        } => {
            // i[impl ctl.definition.param]
            let param =
                definition::param_change(set.as_deref(), unset.as_deref()).unwrap_or_else(|e| {
                    tracing::error!("{e}");
                    std::process::exit(1);
                });
            let definition = require_definition(&definition).await;
            let mut params = definition.fields("script");
            params.insert("app".to_owned(), serde_json::json!(app));
            if let Some(param) = param {
                params.insert("param".to_owned(), param);
            }
            print_result(
                client
                    .request("/apps/update", serde_json::Value::Object(params))
                    .await,
            );
        }
        // i[impl ctl.definition.export]
        AppsCommand::Export {
            app,
            folder,
            generation,
        } => {
            let mut params = serde_json::json!({ "app": app });
            if let Some(g) = generation {
                params["generation"] = serde_json::json!(g);
            }
            match client.request("/apps/bundle", params).await {
                Ok(v) => {
                    let bundle = v["bundle"].as_object().cloned().unwrap_or_default();
                    if let Err(e) = definition::export(&bundle, &folder) {
                        tracing::error!("{e}");
                        std::process::exit(1);
                    }
                }
                Err(e) => print_result(Err(e)),
            }
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
            let submitted = match parse_install_params(&params) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
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
        AppsCommand::Priority { app, priority } => {
            print_result(
                client
                    .request(
                        "/apps/priority",
                        serde_json::json!({ "app": app, "priority": priority.as_str() }),
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
            proposed_reference,
            proposed_params,
        } => {
            let mut params = serde_json::json!({ "app": app });
            let proposed = resolve_or_exit(&DefinitionArgs {
                source: proposed_script_file,
                reference: proposed_reference,
            })
            .await;
            if let Some(proposed) = proposed {
                for (key, value) in proposed.fields("script") {
                    let key = match key.as_str() {
                        "origin" => continue,
                        other => format!("proposed_{other}"),
                    };
                    params[key] = value;
                }
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

/// Resolve a definition that must be given, or exit.
async fn require_definition(args: &DefinitionArgs) -> definition::Definition {
    match resolve_or_exit(args).await {
        Some(d) => d,
        None => {
            tracing::error!("give a script file, a folder, a GitHub folder URL, or --ref");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: AppsCommand,
    }

    // i[verify ctl.definition.source]
    // i[verify ctl.definition.param]
    #[test]
    fn update_takes_a_reference_and_one_param_change() {
        let cli = TestCli::try_parse_from([
            "t",
            "update",
            "web",
            "--ref",
            "ghcr.io/org/def:2.12",
            "--set",
            "version=v2.12",
        ])
        .expect("parses");
        let AppsCommand::Update {
            definition, set, ..
        } = cli.cmd
        else {
            panic!("expected Update");
        };
        assert_eq!(
            definition.reference.as_deref(),
            Some("ghcr.io/org/def:2.12")
        );
        assert_eq!(definition.source, None);
        assert_eq!(set.as_deref(), Some("version=v2.12"));

        assert!(
            TestCli::try_parse_from(["t", "update", "web", "./def", "--ref", "x/y:1"]).is_err(),
            "a source and a reference are exclusive"
        );
        assert!(
            TestCli::try_parse_from([
                "t", "update", "web", "./def", "--set", "a=b", "--unset", "c"
            ])
            .is_err(),
            "one parameter change at most"
        );
    }

    // i[verify ctl.install.params]
    // A bare key used to be dropped by a `filter_map`, so an operator who
    // wrote `db-pass` instead of `db-pass=secret` got an install missing a
    // parameter and nothing said so.
    #[test]
    fn install_params_reject_a_bare_key() {
        let err = parse_install_params(&["db-pass".to_owned()])
            .expect_err("a bare key has no typed value");
        assert!(err.contains("db-pass"), "should name the argument: {err}");
    }

    // i[verify ctl.install.params]
    #[test]
    fn install_params_take_pairs_including_empty_values() {
        let map = parse_install_params(&[
            "key=value".to_owned(),
            "empty=".to_owned(),
            "url=https://x/?a=b".to_owned(),
        ])
        .expect("pairs parse");
        assert_eq!(map["key"], "value");
        assert_eq!(map["empty"], "");
        assert_eq!(map["url"], "https://x/?a=b", "only the first = splits");
        assert!(parse_install_params(&[]).unwrap().is_empty());
    }

    // i[verify ctl.action.params]
    // i[verify ctl.shell.params]
    #[test]
    fn positional_params_map_pairs_and_bare_keys() {
        let map = parse_positional_params(&[
            "key=value".to_owned(),
            "verbose".to_owned(),
            "empty=".to_owned(),
        ]);
        assert_eq!(map["key"], serde_json::json!("value"));
        assert_eq!(map["verbose"], serde_json::json!(true));
        assert_eq!(map["empty"], serde_json::json!(""));
        assert!(parse_positional_params(&[]).is_empty());
    }

    // i[verify ctl.action.params]
    #[test]
    fn action_args_after_name_are_collected_as_params() {
        let cli =
            TestCli::try_parse_from(["test", "action", "myapp", "backup", "key=value", "verbose"])
                .unwrap();
        let AppsCommand::Action { app, name, params } = cli.cmd else {
            panic!("expected Action");
        };
        assert_eq!(app.to_string(), "myapp");
        assert_eq!(name.to_string(), "backup");
        assert_eq!(params, ["key=value", "verbose"]);
    }

    #[test]
    fn vol_id_parses_site_and_app_forms() {
        assert_eq!(parse_vol_id("_site/data").unwrap(), ("_site", "data"));
        assert_eq!(parse_vol_id("myapp/pgdata").unwrap(), ("myapp", "pgdata"));
    }

    #[test]
    fn vol_id_rejects_malformed_input() {
        assert!(parse_vol_id("noslash").is_err());
        assert!(parse_vol_id("/data").is_err());
        assert!(parse_vol_id("myapp/").is_err());
    }
}
