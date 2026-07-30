//! The operator-facing side of the Canopy relay.
//!
//! Registering and withdrawing offers is deliberately absent: those are made by
//! the client that carries the requests, presenting the connection it will carry
//! them over, which an operator has nothing to present.
//!
//! So is issuing a request. The relay carries what the runtime itself needs; a
//! command for relaying an arbitrary one would hand every authorised operator
//! the full authority of the carrying client's Canopy identity, and the path is
//! exercised end to end by the status reports the runtime already sends.

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::json;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum CanopyCommand {
    /// Show whether Canopy access is on, which client is carrying requests, and
    /// how the last status report went
    Status,
    /// Turn Canopy access on
    Enable,
    /// Turn Canopy access off.
    ///
    /// Refuses new offers and revokes any live one immediately, so it takes
    /// effect without waiting for the carrying client to reconnect.
    Disable,
}

// i[impl ctl.canopy]
pub(super) async fn dispatch(client: &OiClient, cmd: CanopyCommand) {
    match cmd {
        CanopyCommand::Status => {
            print_result(client.request("/canopy/status", json!({})).await);
        }
        CanopyCommand::Enable => {
            print_result(
                client
                    .request("/canopy/settings/set", json!({ "enabled": true }))
                    .await,
            );
        }
        CanopyCommand::Disable => {
            print_result(
                client
                    .request("/canopy/settings/set", json!({ "enabled": false }))
                    .await,
            );
        }
    }
}
