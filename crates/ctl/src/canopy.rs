use std::io::Read as _;

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::json;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum CanopyCommand {
    /// Enrol this instance with Canopy using an enrolment ticket
    // i[impl canopy.enrol]
    Enrol {
        /// Encrypted enrolment ticket from Canopy. Reads stdin when omitted.
        ticket: Option<String>,
        /// The ticket's passphrase, shared out of band. Prompted for on
        /// stdin when omitted and the ticket was passed as an argument.
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Show the Canopy link's registration and reporting state
    // i[impl canopy.status]
    Status,
    /// Remove the Canopy registration and stop reporting
    // i[impl canopy.deregister]
    Deregister,
}

pub(super) async fn dispatch(client: &OiClient, cmd: CanopyCommand) {
    match cmd {
        CanopyCommand::Enrol { ticket, passphrase } => {
            let ticket = match ticket {
                Some(t) => t,
                None => {
                    let mut buf = String::new();
                    if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
                        eprintln!("no ticket on stdin");
                        std::process::exit(1);
                    }
                    buf.trim().to_owned()
                }
            };
            let passphrase = match passphrase {
                Some(p) => p,
                None => {
                    eprintln!("passphrase:");
                    let mut buf = String::new();
                    if std::io::stdin().read_line(&mut buf).is_err() || buf.trim().is_empty() {
                        eprintln!("no passphrase given");
                        std::process::exit(1);
                    }
                    buf.trim().to_owned()
                }
            };
            print_result(
                client
                    .request(
                        "/canopy/enrol",
                        json!({ "ticket": ticket, "passphrase": passphrase }),
                    )
                    .await,
            );
        }
        CanopyCommand::Status => {
            print_result(client.request("/canopy/status", json!({})).await);
        }
        CanopyCommand::Deregister => {
            print_result(client.request("/canopy/deregister", json!({})).await);
        }
    }
}
