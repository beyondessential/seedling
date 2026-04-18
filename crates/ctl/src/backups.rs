use clap::Subcommand;
use seedling_protocol::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum BackupsCommand {
    /// Backup app management
    Apps {
        #[command(subcommand)]
        command: BackupAppsCommand,
    },
    /// Backup strategy management
    Strategies {
        #[command(subcommand)]
        command: BackupStrategiesCommand,
    },
    /// Trigger an immediate backup for a strategy
    Run {
        /// Strategy name
        #[arg(long)]
        strategy: String,
    },
    /// Snapshot management
    Snapshots {
        #[command(subcommand)]
        command: BackupSnapshotsCommand,
    },
    /// Restore a snapshot to a new site volume
    Restore {
        /// Strategy name
        #[arg(long)]
        strategy: String,
        /// Volume identifier (e.g. myapp/data or _site/vol)
        #[arg(long)]
        volume: String,
        /// Snapshot identifier
        #[arg(long)]
        snapshot: String,
    },
}

#[derive(Subcommand)]
pub(super) enum BackupAppsCommand {
    /// Register an app as a backup app
    Register {
        /// Backup app registration name
        #[arg(long)]
        name: String,
        /// App name
        #[arg(long)]
        app: String,
    },
    /// Deregister a backup app
    Deregister {
        /// Backup app registration name
        #[arg(long)]
        name: String,
    },
    /// List registered backup apps
    List,
}

#[derive(Subcommand)]
pub(super) enum BackupStrategiesCommand {
    /// Create a backup strategy
    Create {
        /// Strategy name
        #[arg(long)]
        name: String,
        /// Registered backup app name
        #[arg(long)]
        via: String,
        /// Schedule: "every hour", "twice a day", or "every day"
        #[arg(long)]
        schedule: String,
        /// Source volumes (e.g. myapp/data or _site/vol), repeatable
        #[arg(long = "volume")]
        volumes: Vec<String>,
        /// Allow volumes that cannot be resolved at creation time
        #[arg(long)]
        allow_missing: bool,
    },
    /// List backup strategies
    List,
    /// Show a backup strategy
    Show {
        /// Strategy name
        #[arg(long)]
        name: String,
    },
    /// Update a backup strategy
    Update {
        /// Strategy name
        #[arg(long)]
        name: String,
        /// Change the backup app
        #[arg(long)]
        via: Option<String>,
        /// Change the schedule
        #[arg(long)]
        schedule: Option<String>,
        /// Replace all volumes (repeatable)
        #[arg(long = "volume")]
        volumes: Option<Vec<String>>,
        /// Allow volumes that cannot be resolved
        #[arg(long)]
        allow_missing: bool,
    },
    /// Delete a backup strategy
    Delete {
        /// Strategy name
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub(super) enum BackupSnapshotsCommand {
    /// List available snapshots for a volume
    List {
        /// Strategy name
        #[arg(long)]
        strategy: String,
        /// Volume identifier (e.g. myapp/data or _site/vol)
        #[arg(long)]
        volume: String,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: BackupsCommand) {
    match cmd {
        BackupsCommand::Apps { command } => match command {
            // i[impl backup.app.register]
            BackupAppsCommand::Register { name, app } => {
                print_result(
                    client
                        .request(
                            "/backups/apps/register",
                            serde_json::json!({ "name": name, "app": app }),
                        )
                        .await,
                );
            }
            // i[impl backup.app.deregister]
            BackupAppsCommand::Deregister { name } => {
                print_result(
                    client
                        .request(
                            "/backups/apps/deregister",
                            serde_json::json!({ "name": name }),
                        )
                        .await,
                );
            }
            // i[impl backup.app.list]
            BackupAppsCommand::List => {
                print_result(
                    client
                        .request("/backups/apps/list", serde_json::json!({}))
                        .await,
                );
            }
        },
        BackupsCommand::Strategies { command } => match command {
            // i[impl backup.strategy.create]
            BackupStrategiesCommand::Create {
                name,
                via,
                schedule,
                volumes,
                allow_missing,
            } => {
                // i[impl ctl.backup.strategy.allow-missing]
                if !allow_missing
                    && let Some(missing) = check_missing_volumes(client, &volumes).await
                {
                    tracing::error!(
                        "volumes not found: {missing}; pass --allow-missing to proceed anyway"
                    );
                    std::process::exit(1);
                }
                print_result(
                    client
                        .request(
                            "/backups/strategies/create",
                            serde_json::json!({
                                "name": name,
                                "via": via,
                                "schedule": schedule,
                                "volumes": volumes,
                            }),
                        )
                        .await,
                );
            }
            // i[impl backup.strategy.list]
            BackupStrategiesCommand::List => {
                print_result(
                    client
                        .request("/backups/strategies/list", serde_json::json!({}))
                        .await,
                );
            }
            // i[impl backup.strategy.show]
            BackupStrategiesCommand::Show { name } => {
                print_result(
                    client
                        .request(
                            "/backups/strategies/show",
                            serde_json::json!({ "name": name }),
                        )
                        .await,
                );
            }
            // i[impl backup.strategy.update]
            BackupStrategiesCommand::Update {
                name,
                via,
                schedule,
                volumes,
                allow_missing,
            } => {
                // i[impl ctl.backup.strategy.allow-missing]
                if !allow_missing
                    && let Some(vols) = &volumes
                    && let Some(missing) = check_missing_volumes(client, vols).await
                {
                    tracing::error!(
                        "volumes not found: {missing}; pass --allow-missing to proceed anyway"
                    );
                    std::process::exit(1);
                }
                let mut body = serde_json::json!({ "name": name });
                if let Some(v) = via {
                    body["via"] = serde_json::json!(v);
                }
                if let Some(s) = schedule {
                    body["schedule"] = serde_json::json!(s);
                }
                if let Some(vols) = volumes {
                    body["volumes"] = serde_json::json!(vols);
                }
                print_result(client.request("/backups/strategies/update", body).await);
            }
            // i[impl backup.strategy.delete]
            BackupStrategiesCommand::Delete { name } => {
                print_result(
                    client
                        .request(
                            "/backups/strategies/delete",
                            serde_json::json!({ "name": name }),
                        )
                        .await,
                );
            }
        },
        // i[impl backup.run]
        BackupsCommand::Run { strategy } => {
            print_result(
                client
                    .request("/backups/run", serde_json::json!({ "strategy": strategy }))
                    .await,
            );
        }
        // i[impl backup.snapshots.list]
        BackupsCommand::Snapshots { command } => match command {
            BackupSnapshotsCommand::List { strategy, volume } => {
                print_result(
                    client
                        .request(
                            "/backups/snapshots/list",
                            serde_json::json!({ "strategy": strategy, "volume": volume }),
                        )
                        .await,
                );
            }
        },
        // i[impl backup.restore]
        BackupsCommand::Restore {
            strategy,
            volume,
            snapshot,
        } => {
            print_result(
                client
                    .request(
                        "/backups/restore",
                        serde_json::json!({
                            "strategy": strategy,
                            "volume": volume,
                            "snapshot": snapshot,
                        }),
                    )
                    .await,
            );
        }
    }
}

/// Check whether any of the given volume identifiers are missing.
/// Returns a comma-separated list of missing volumes, or `None` if all are found.
async fn check_missing_volumes(client: &OiClient, volumes: &[String]) -> Option<String> {
    let exported = client
        .request("/volumes/exported/list", serde_json::json!({}))
        .await
        .ok()?;
    let site = client
        .request("/volumes/site/list", serde_json::json!({}))
        .await
        .ok()?;

    let mut missing = Vec::new();
    for vol_id in volumes {
        if let Some((prefix, vol)) = vol_id.split_once('/') {
            let found = if prefix == "_site" {
                site.as_array()
                    .map(|arr| arr.iter().any(|v| v["name"] == vol))
                    .unwrap_or(false)
            } else {
                exported
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .any(|v| v["app"] == prefix && v["volume_name"] == vol)
                    })
                    .unwrap_or(false)
            };
            if !found {
                missing.push(vol_id.as_str());
            }
        } else {
            missing.push(vol_id.as_str());
        }
    }

    if missing.is_empty() {
        None
    } else {
        Some(missing.join(", "))
    }
}
