use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use seedling_protocol::client::OiClient;
use seedling_protocol::names::{ActionName, AppName};

use super::{
    definition::{self, DefinitionArgs, DefinitionKeys, resolve_or_exit},
    print_result,
};

// spec: CLI#action-and-shell-parameters
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
    /// Replace an app's definition, optionally changing parameters in the
    /// same step
    Update {
        app: AppName,
        #[command(flatten)]
        definition: DefinitionArgs,
        /// Set a parameter alongside the definition, as name=value (repeatable)
        #[arg(long, value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Unset a parameter alongside the definition (repeatable)
        #[arg(long, value_name = "NAME")]
        unset: Vec<String>,
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
    Params {
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
    // spec: CLI#action-and-shell-parameters
    CancelAction { app: AppName },
    /// Install an app
    // spec: ACI#install
    Install { app: AppName },
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
        /// Proposed parameter to set, as `name=value` (repeatable)
        #[arg(long = "param", value_name = "NAME=VALUE")]
        proposed_params: Vec<String>,
        /// Proposed parameter to unset (repeatable)
        #[arg(long = "unset", value_name = "NAME")]
        proposed_unset: Vec<String>,
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

/// Resolve a relative scale request against the declared bounds and what the
/// deployment is running now.
///
/// `current` is `None` where the daemon could not read the operator's scaling
/// decision and reported it unavailable. Returns `None` for a direction that
/// needs a figure there is none of, so the caller says so rather than counting
/// from zero: zero would turn a scale up into a scale down to the floor, which
/// is the harm the daemon withheld the figure to avoid.
// spec: CTL#scaling
fn resolve_relative_scale(
    direction: &ScaleDirection,
    current: Option<u16>,
    low: u16,
    high: u16,
) -> Option<u16> {
    match direction {
        ScaleDirection::Up => Some(current?.saturating_add(1).min(high)),
        ScaleDirection::Down => Some(current?.saturating_sub(1).max(low)),
        ScaleDirection::ToMin => Some(low),
        ScaleDirection::To { .. } => unreachable!("an exact count is resolved by its caller"),
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
    /// Set and unset parameters, all in one change
    Set {
        app: AppName,
        /// Parameters to set, as name=value
        #[arg(value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Unset a parameter (repeatable)
        #[arg(long, value_name = "NAME")]
        unset: Vec<String>,
    },
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
                                    r["type"] == "external_volume"
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
            let mut params = definition.fields(DefinitionKeys::APP);
            params.insert("app".to_owned(), serde_json::json!(app));
            print_result(
                client
                    .request("/apps/create", serde_json::Value::Object(params))
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
        AppsCommand::Update {
            app,
            definition,
            set,
            unset,
        } => {
            // spec: CLI#parameter-changes
            let changes = definition::param_changes(&set, &unset).unwrap_or_else(|e| {
                tracing::error!("{e}");
                std::process::exit(1);
            });
            let definition = require_definition(&definition).await;
            let mut params = definition.fields(DefinitionKeys::APP);
            params.insert("app".to_owned(), serde_json::json!(app));
            if !changes.is_empty() {
                params.insert("params".to_owned(), serde_json::Value::Object(changes));
            }
            print_result(
                client
                    .request("/apps/update", serde_json::Value::Object(params))
                    .await,
            );
        }
        // spec: CLI#definition-export
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
        AppsCommand::Params { command } => match command {
            // spec: CLI#parameter-changes
            ParamCommand::Set { app, set, unset } => {
                let changes = definition::param_changes(&set, &unset).unwrap_or_else(|e| {
                    tracing::error!("{e}");
                    std::process::exit(1);
                });
                if changes.is_empty() {
                    tracing::error!("give at least one parameter to set or --unset");
                    std::process::exit(1);
                }
                print_result(
                    client
                        .request(
                            "/apps/params/set",
                            serde_json::json!({ "app": app, "params": changes }),
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
        // spec: CLI#action-and-shell-parameters
        AppsCommand::CancelAction { app } => {
            print_result(
                client
                    .request("/apps/action/cancel", serde_json::json!({ "app": app }))
                    .await,
            );
        }
        // spec: ACI#install
        AppsCommand::Install { app } => {
            print_result(
                client
                    .request("/apps/install", serde_json::json!({ "app": app }))
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
                    let current = scale_obj["current"].as_u64().map(|c| c as u16);
                    let low = scale_obj["low"].as_u64().unwrap_or(0) as u16;
                    let high = scale_obj["high"].as_u64().unwrap_or(0) as u16;
                    match resolve_relative_scale(&relative, current, low, high) {
                        Some(scale) => scale,
                        None => {
                            eprintln!(
                                "error: the current scale of {deployment:?} is unavailable, so it \
                                 cannot be adjusted by one instance; set an exact count instead"
                            );
                            std::process::exit(1);
                        }
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
            proposed_unset,
        } => {
            let mut params = serde_json::json!({ "app": app });
            let proposed = resolve_or_exit(&DefinitionArgs {
                source: proposed_script_file,
                reference: proposed_reference,
            })
            .await;
            if let Some(proposed) = proposed {
                for (key, value) in proposed.fields(DefinitionKeys::PROPOSED) {
                    params[key] = value;
                }
            }
            // spec: CLI#parameter-changes
            let changes = definition::param_changes(&proposed_params, &proposed_unset)
                .unwrap_or_else(|e| {
                    tracing::error!("{e}");
                    std::process::exit(1);
                });
            if !changes.is_empty() {
                params["proposed_params"] = serde_json::Value::Object(changes);
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

    // spec: CTL#scaling
    // `current` was read with `unwrap_or(0)`, so a deployment whose scaling
    // decision the daemon could not read answered `--up` with 0 + 1 = 1 and
    // scaled it down to its floor. That is the harm the null exists to
    // prevent, arriving by another route.
    #[test]
    fn a_scale_that_is_unavailable_cannot_be_adjusted_by_one() {
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Up, None, 1, 5),
            None,
            "nothing to add one to"
        );
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Down, None, 1, 5),
            None
        );
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::ToMin, None, 1, 5),
            Some(1),
            "the floor is declared, so it needs no current figure"
        );
    }

    // spec: CTL#scaling
    #[test]
    fn a_known_scale_is_adjusted_within_its_bounds() {
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Up, Some(3), 1, 5),
            Some(4)
        );
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Up, Some(5), 1, 5),
            Some(5),
            "held at the upper bound"
        );
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Down, Some(3), 1, 5),
            Some(2)
        );
        assert_eq!(
            resolve_relative_scale(&ScaleDirection::Down, Some(1), 1, 5),
            Some(1),
            "held at the lower bound"
        );
    }

    // spec: CLI#definition-sources
    // spec: CLI#parameter-changes
    #[test]
    fn update_takes_a_reference_and_several_param_changes() {
        let cli = TestCli::try_parse_from([
            "t",
            "update",
            "web",
            "--ref",
            "ghcr.io/org/def:2.12",
            "--set",
            "version=v2.12",
            "--set",
            "port=8080",
            "--unset",
            "old",
            "--unset",
            "older",
        ])
        .expect("parses");
        let AppsCommand::Update {
            definition,
            set,
            unset,
            ..
        } = cli.cmd
        else {
            panic!("expected Update");
        };
        assert_eq!(
            definition.reference.as_deref(),
            Some("ghcr.io/org/def:2.12")
        );
        assert_eq!(definition.source, None);
        assert_eq!(set, ["version=v2.12", "port=8080"]);
        assert_eq!(unset, ["old", "older"]);

        assert!(
            TestCli::try_parse_from(["t", "update", "web", "./def", "--ref", "x/y:1"]).is_err(),
            "a source and a reference are exclusive"
        );
        let cli = TestCli::try_parse_from([
            "t", "update", "web", "./def", "--set", "a=b", "--unset", "c",
        ])
        .expect("setting and unsetting different parameters together parses");
        assert!(matches!(cli.cmd, AppsCommand::Update { .. }));
    }

    // spec: CLI#parameter-changes
    #[test]
    fn params_set_takes_pairs_and_unsets() {
        let cli = TestCli::try_parse_from([
            "t", "params", "set", "web", "a=1", "b=two=2", "--unset", "c", "--unset", "d",
        ])
        .expect("parses");
        let AppsCommand::Params {
            command: ParamCommand::Set { app, set, unset },
        } = cli.cmd
        else {
            panic!("expected params set");
        };
        assert_eq!(app.as_str(), "web");
        assert_eq!(set, ["a=1", "b=two=2"]);
        assert_eq!(unset, ["c", "d"]);

        let cli = TestCli::try_parse_from(["t", "params", "set", "web", "--unset", "c", "x=1"])
            .expect("pairs may follow an unset");
        let AppsCommand::Params {
            command: ParamCommand::Set { set, unset, .. },
        } = cli.cmd
        else {
            panic!("expected params set");
        };
        assert_eq!(set, ["x=1"]);
        assert_eq!(unset, ["c"]);
    }

    // spec: CLI#parameter-changes
    #[test]
    fn plan_takes_proposed_params_and_unsets() {
        let cli = TestCli::try_parse_from([
            "t", "plan", "web", "--param", "a=1", "--param", "b=2", "--unset", "c",
        ])
        .expect("parses");
        let AppsCommand::Plan {
            proposed_params,
            proposed_unset,
            ..
        } = cli.cmd
        else {
            panic!("expected Plan");
        };
        assert_eq!(proposed_params, ["a=1", "b=2"]);
        assert_eq!(proposed_unset, ["c"]);
    }

    // spec: CLI#action-and-shell-parameters
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

    // spec: CLI#action-and-shell-parameters
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
