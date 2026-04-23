use std::path::PathBuf;

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use seedling_protocol::names::{AppName, TemplateName};

use super::print_result;

#[derive(Subcommand)]
pub(super) enum TemplatesCommand {
    /// List uploaded templates
    List,
    /// Show a template's body and metadata
    Show {
        /// Template name
        name: TemplateName,
    },
    /// Upload a new template from a script file
    Create {
        /// Template name
        name: TemplateName,
        /// Path to the BSL script file to upload
        script_file: PathBuf,
        /// Optional human-readable description
        #[arg(long)]
        description: Option<String>,
    },
    /// Update an existing template's body and/or description
    Update {
        /// Template name
        name: TemplateName,
        /// Path to a new BSL script file; omit to leave the body unchanged
        #[arg(long)]
        script_file: Option<PathBuf>,
        /// Replacement description; use --clear-description to remove instead
        #[arg(long, conflicts_with = "clear_description")]
        description: Option<String>,
        /// Clear the stored description
        #[arg(long)]
        clear_description: bool,
    },
    /// Remove an uploaded template
    Remove {
        /// Template name
        name: TemplateName,
        /// Confirm removal without prompting
        #[arg(long)]
        confirm: bool,
    },
    /// Preview a template's declared resources, params, and actions
    Preview {
        /// Stored template name (omit when using --file)
        name: Option<TemplateName>,
        /// Preview a local script file instead of a stored template
        #[arg(long, conflicts_with = "name")]
        file: Option<PathBuf>,
    },
    /// Create a new app from a template (copies the script wholesale)
    Instantiate {
        /// Template name
        template: TemplateName,
        /// Name for the new app
        app: AppName,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: TemplatesCommand) {
    match cmd {
        TemplatesCommand::List => {
            print_result(
                client
                    .request("/templates/list", serde_json::json!({}))
                    .await,
            );
        }
        TemplatesCommand::Show { name } => {
            print_result(
                client
                    .request("/templates/show", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        TemplatesCommand::Create {
            name,
            script_file,
            description,
        } => {
            let body = read_script_file(&script_file);
            print_result(
                client
                    .request(
                        "/templates/create",
                        serde_json::json!({
                            "name": name,
                            "body": body,
                            "description": description,
                        }),
                    )
                    .await,
            );
        }
        TemplatesCommand::Update {
            name,
            script_file,
            description,
            clear_description,
        } => {
            let mut req = serde_json::Map::new();
            req.insert("name".to_owned(), serde_json::to_value(&name).unwrap());
            if let Some(path) = script_file {
                req.insert(
                    "body".to_owned(),
                    serde_json::Value::String(read_script_file(&path)),
                );
            }
            if clear_description {
                req.insert("description".to_owned(), serde_json::Value::Null);
            } else if let Some(desc) = description {
                req.insert("description".to_owned(), serde_json::Value::String(desc));
            }
            print_result(
                client
                    .request("/templates/update", serde_json::Value::Object(req))
                    .await,
            );
        }
        TemplatesCommand::Remove { name, confirm } => {
            let confirmed = confirm || {
                eprint!(
                    "Remove template {name}? Apps instantiated from it are unaffected. [yes/N] "
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
                    .request("/templates/remove", serde_json::json!({ "name": name }))
                    .await,
            );
        }
        TemplatesCommand::Preview { name, file } => {
            let params = match (name, file) {
                (Some(n), None) => serde_json::json!({ "name": n }),
                (None, Some(path)) => {
                    let body = read_script_file(&path);
                    serde_json::json!({ "body": body })
                }
                (None, None) => {
                    eprintln!("error: supply either <name> or --file <path>");
                    std::process::exit(1);
                }
                (Some(_), Some(_)) => unreachable!("clap conflicts_with prevents this"),
            };
            print_result(client.request("/templates/preview", params).await);
        }
        TemplatesCommand::Instantiate { template, app } => {
            print_result(
                client
                    .request(
                        "/templates/instantiate",
                        serde_json::json!({ "template": template, "app": app }),
                    )
                    .await,
            );
        }
    }
}

fn read_script_file(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!("cannot read {}: {e}", path.display());
        std::process::exit(1);
    })
}
