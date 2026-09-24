use clap::Subcommand;
use seedling_protocol::client::OiClient;
use seedling_protocol::names::{AppName, TemplateName};

use super::{
    definition::{DefinitionArgs, DefinitionKeys, resolve_or_exit},
    print_result,
};

#[derive(Subcommand)]
pub(super) enum TemplatesCommand {
    /// List uploaded templates
    List,
    /// Show a template's body and metadata
    Show {
        /// Template name
        name: TemplateName,
    },
    /// Store a new template from a script file, a definition folder, a
    /// GitHub folder URL, or an OCI reference
    Create {
        /// Template name
        name: TemplateName,
        #[command(flatten)]
        definition: DefinitionArgs,
        /// Optional human-readable description
        #[arg(long)]
        description: Option<String>,
    },
    /// Update an existing template's body and/or description
    Update {
        /// Template name
        name: TemplateName,
        /// A new script file, definition folder, or GitHub folder URL; omit
        /// to leave the definition unchanged
        #[arg(long, alias = "script-file", conflicts_with = "reference")]
        source: Option<String>,
        /// A new definition, fetched from this OCI reference
        #[arg(long = "ref", value_name = "REFERENCE")]
        reference: Option<String>,
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
        /// Stored template name (omit when using --file or --ref)
        name: Option<TemplateName>,
        /// Preview a script file, definition folder, or GitHub folder URL
        /// instead of a stored template
        #[arg(long, conflicts_with_all = ["name", "reference"])]
        file: Option<String>,
        /// Preview a definition fetched from this OCI reference
        #[arg(long = "ref", value_name = "REFERENCE", conflicts_with = "name")]
        reference: Option<String>,
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
            definition,
            description,
        } => {
            let Some(definition) = resolve_or_exit(&definition).await else {
                tracing::error!("give a script file, a folder, a GitHub folder URL, or --ref");
                std::process::exit(1);
            };
            let mut req = definition.fields(DefinitionKeys::TEMPLATE);
            req.insert("name".to_owned(), serde_json::to_value(&name).unwrap());
            req.insert("description".to_owned(), serde_json::json!(description));
            print_result(
                client
                    .request("/templates/create", serde_json::Value::Object(req))
                    .await,
            );
        }
        TemplatesCommand::Update {
            name,
            source,
            reference,
            description,
            clear_description,
        } => {
            let mut req = serde_json::Map::new();
            req.insert("name".to_owned(), serde_json::to_value(&name).unwrap());
            if let Some(definition) = resolve_or_exit(&DefinitionArgs { source, reference }).await {
                req.extend(definition.fields(DefinitionKeys::TEMPLATE));
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
        TemplatesCommand::Preview {
            name,
            file,
            reference,
        } => {
            let definition = resolve_or_exit(&DefinitionArgs {
                source: file,
                reference,
            })
            .await;
            let params = match (name, definition) {
                (Some(n), None) => serde_json::json!({ "name": n }),
                (None, Some(d)) => serde_json::Value::Object(d.fields(DefinitionKeys::TEMPLATE)),
                (None, None) => {
                    eprintln!("error: supply <name>, --file <path>, or --ref <reference>");
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TemplatesCommand,
    }

    #[test]
    fn preview_rejects_both_name_and_file() {
        assert!(
            TestCli::try_parse_from(["test", "preview", "mytpl", "--file", "script.bsl"]).is_err()
        );
        let cli = TestCli::try_parse_from(["test", "preview", "--file", "script.bsl"]).unwrap();
        let TemplatesCommand::Preview { name, file, .. } = cli.cmd else {
            panic!("expected Preview");
        };
        assert_eq!(name, None);
        assert_eq!(file.as_deref(), Some("script.bsl"));
    }

    #[test]
    fn update_rejects_description_together_with_clear() {
        assert!(
            TestCli::try_parse_from([
                "test",
                "update",
                "mytpl",
                "--description",
                "d",
                "--clear-description",
            ])
            .is_err()
        );
        let cli =
            TestCli::try_parse_from(["test", "update", "mytpl", "--clear-description"]).unwrap();
        let TemplatesCommand::Update {
            description,
            clear_description,
            ..
        } = cli.cmd
        else {
            panic!("expected Update");
        };
        assert_eq!(description, None);
        assert!(clear_description);
    }
}
