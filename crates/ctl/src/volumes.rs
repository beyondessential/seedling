use clap::Subcommand;
use seedling_protocol::client::OiClient;

use super::print_result;
use crate::shell::open_volume_shell;

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
    /// Open an interactive shell with one or more volumes mounted side-by-side.
    ///
    /// Each VOLUME argument is parsed as one of:
    ///
    ///   _site/NAME    a site volume
    ///
    ///   APP/VOLUME    a volume owned by APP
    ///
    ///   held:ID       a held volume (id from `volumes held list`)
    Shell {
        /// Mount every volume read-only regardless of its underlying kind.
        #[arg(short = 'r', long)]
        read_only: bool,
        /// Volumes to mount. At least one is required.
        #[arg(required = true)]
        volumes: Vec<String>,
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
    /// Restore a held volume's data into a fresh managed site volume
    Restore {
        /// Held volume ID (from `volumes held list`)
        id: String,
        /// Override the new site volume's name; defaults to the held
        /// volume's recorded name.
        #[arg(long)]
        name: Option<String>,
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
    /// Promote a snapshot site volume to a fresh read-write managed site volume
    Promote {
        /// Source snapshot site volume name
        source: String,
        /// Name for the new managed site volume
        name: String,
    },
}

/// Parse one of the volume spec strings accepted by `volumes shell` into the
/// JSON object the OI endpoint expects.
// i[impl volumes.shell]
fn parse_volume_spec(spec: &str) -> Result<serde_json::Value, String> {
    if let Some(id) = spec.strip_prefix("held:") {
        if id.is_empty() {
            return Err(format!("missing held id after 'held:': {spec:?}"));
        }
        return Ok(serde_json::json!({ "kind": "held", "id": id }));
    }
    let Some((left, right)) = spec.split_once('/') else {
        return Err(format!(
            "invalid volume spec {spec:?}: expected _site/NAME, APP/VOLUME, or held:ID"
        ));
    };
    if left == "_site" {
        if right.is_empty() {
            return Err(format!("missing site volume name after '_site/': {spec:?}"));
        }
        return Ok(serde_json::json!({ "kind": "site", "name": right }));
    }
    if left.is_empty() || right.is_empty() {
        return Err(format!(
            "invalid volume spec {spec:?}: expected _site/NAME, APP/VOLUME, or held:ID"
        ));
    }
    Ok(serde_json::json!({
        "kind": "app",
        "app": left,
        "volume": right,
    }))
}

pub(super) async fn dispatch(client: &OiClient, cmd: VolumesCommand) {
    match cmd {
        VolumesCommand::Shell { read_only, volumes } => {
            let parsed: Result<Vec<_>, _> = volumes.iter().map(|s| parse_volume_spec(s)).collect();
            let refs = match parsed {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            };
            let exit = open_volume_shell(client, refs, read_only).await;
            std::process::exit(exit);
        }
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
            HeldCommand::Restore { id, name } => {
                let mut params = serde_json::json!({ "id": id });
                if let Some(name) = name {
                    params["target_name"] = serde_json::json!(name);
                }
                print_result(client.request("/volumes/held/restore", params).await);
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
            SiteCommand::Promote { source, name } => {
                print_result(
                    client
                        .request(
                            "/volumes/site/promote",
                            serde_json::json!({
                                "source": source,
                                "name": name,
                            }),
                        )
                        .await,
                );
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::parse_volume_spec;
    use serde_json::json;

    #[test]
    fn parses_site_volume() {
        assert_eq!(
            parse_volume_spec("_site/data").unwrap(),
            json!({ "kind": "site", "name": "data" })
        );
    }

    #[test]
    fn parses_app_volume() {
        assert_eq!(
            parse_volume_spec("postgres/pg18").unwrap(),
            json!({ "kind": "app", "app": "postgres", "volume": "pg18" })
        );
    }

    #[test]
    fn parses_held_volume() {
        assert_eq!(
            parse_volume_spec("held:abc-123").unwrap(),
            json!({ "kind": "held", "id": "abc-123" })
        );
    }

    #[test]
    fn rejects_bare_word() {
        assert!(parse_volume_spec("just-a-name").is_err());
    }

    #[test]
    fn rejects_empty_held_id() {
        assert!(parse_volume_spec("held:").is_err());
    }

    #[test]
    fn rejects_empty_site_name() {
        assert!(parse_volume_spec("_site/").is_err());
    }
}
