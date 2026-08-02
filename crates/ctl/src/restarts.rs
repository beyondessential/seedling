//! Container restart history and the crash-loop rate that is derived from it.
//!
//! The rate — not systemd's own start limit — is what files a `crash_loop`
//! fault, so the threshold and window are operator business and live here
//! rather than being an implementation constant.

use clap::Subcommand;
use seedling_protocol::client::OiClient;
use serde_json::json;

use super::print_result;

#[derive(Subcommand)]
pub(super) enum RestartsCommand {
    /// List recorded container restarts, most recent first
    List {
        /// Only restarts for this app
        #[arg(long)]
        app: Option<String>,
        /// Only restarts for this instance id
        #[arg(long)]
        instance: Option<String>,
        /// Maximum records to return (default 100, max 1000)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show the crash-loop rate threshold and window
    Settings,
    /// Change the crash-loop rate threshold and/or window.
    ///
    /// A `crash_loop` fault is filed once an instance records this many
    /// recovery restarts inside the window. Restarts seedling performs
    /// deliberately (rolling updates, replacements) do not count.
    SetSettings {
        /// Restarts within the window that file the fault (minimum 2)
        #[arg(long)]
        threshold: Option<i64>,
        /// Width of the window in seconds (minimum 60)
        #[arg(long)]
        window_secs: Option<i64>,
    },
}

pub(super) async fn dispatch(client: &OiClient, cmd: RestartsCommand) {
    match cmd {
        RestartsCommand::List {
            app,
            instance,
            limit,
        } => {
            print_result(
                client
                    .request(
                        "/restarts/list",
                        json!({ "app": app, "instance": instance, "limit": limit }),
                    )
                    .await,
            );
        }
        RestartsCommand::Settings => {
            print_result(client.request("/restarts/settings/get", json!({})).await);
        }
        RestartsCommand::SetSettings {
            threshold,
            window_secs,
        } => {
            if threshold.is_none() && window_secs.is_none() {
                eprintln!("error: pass at least one of --threshold or --window-secs");
                std::process::exit(1);
            }
            print_result(
                client
                    .request(
                        "/restarts/settings/set",
                        json!({ "threshold": threshold, "window_secs": window_secs }),
                    )
                    .await,
            );
        }
    }
}
