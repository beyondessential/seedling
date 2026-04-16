use clap::Subcommand;
use seedling::oi::client::OiClient;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum VolumesCommand {
    /// Held volume management
    Held {
        #[command(subcommand)]
        command: HeldCommand,
    },
    /// Site volume management
    Site {
        #[command(subcommand)]
        command: SiteCommand,
    },
}

#[derive(Subcommand)]
pub(super) enum HeldCommand {
    /// List held volumes awaiting operator confirmation
    List,
    /// Confirm deletion of a held volume
    Delete {
        /// Held volume ID (from `volumes held list`)
        id: String,
        /// Confirm deletion without prompting
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand)]
pub(super) enum SiteCommand {
    /// Create a managed site volume (BTRFS subvolume or directory)
    CreateManaged {
        /// Volume name
        name: String,
    },
    /// Create a bind site volume pointing at an existing host path
    CreateBind {
        /// Volume name
        name: String,
        /// Absolute host path to bind-mount
        #[arg(long)]
        host_path: String,
    },
    /// List site volumes
    List,
    /// Delete a site volume
    Delete {
        /// Volume name
        name: String,
    },
    /// Create a read-only BTRFS snapshot of a named volume
    Snapshot {
        /// Name for the new snapshot site volume
        name: String,
        /// Source volume: _site/<name> or <app>/<volume>
        source: String,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: VolumesCommand) {
    match cmd {
        VolumesCommand::Held { command } => match command {
            HeldCommand::List => {
                print_result(
                    client
                        .request("/volumes/held/list", serde_json::json!({}))
                        .await,
                );
            }
            HeldCommand::Delete { id, confirm } => {
                let confirmed = confirm || {
                    eprint!(
                        "Delete held volume {id}? This is permanent and cannot be undone. [yes/N] "
                    );
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).ok();
                    line.trim() == "yes"
                };
                if !confirmed {
                    eprintln!("Aborted.");
                    std::process::exit(1);
                }
                print_result(
                    client
                        .request("/volumes/held/delete", serde_json::json!({ "id": id }))
                        .await,
                );
            }
        },
        VolumesCommand::Site { command } => match command {
            SiteCommand::CreateManaged { name } => {
                print_result(
                    client
                        .request(
                            "/volumes/site/create",
                            serde_json::json!({
                                "name": name,
                                "kind": "managed",
                            }),
                        )
                        .await,
                );
            }
            SiteCommand::CreateBind { name, host_path } => {
                print_result(
                    client
                        .request(
                            "/volumes/site/create",
                            serde_json::json!({
                                "name": name,
                                "kind": "bind",
                                "host_path": host_path,
                            }),
                        )
                        .await,
                );
            }
            SiteCommand::List => {
                print_result(
                    client
                        .request("/volumes/site/list", serde_json::json!({}))
                        .await,
                );
            }
            SiteCommand::Delete { name } => {
                print_result(
                    client
                        .request("/volumes/site/delete", serde_json::json!({ "name": name }))
                        .await,
                );
            }
            SiteCommand::Snapshot { name, source } => {
                print_result(
                    client
                        .request(
                            "/volumes/site/snapshot",
                            serde_json::json!({
                                "name": name,
                                "source": source,
                            }),
                        )
                        .await,
                );
            }
        },
    }
}
