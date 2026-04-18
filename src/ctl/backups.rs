use clap::Subcommand;
use seedling::oi::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum BackupsCommand {
    /// Backup app management
    Apps {
        #[command(subcommand)]
        command: BackupAppsCommand,
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
    }
}
