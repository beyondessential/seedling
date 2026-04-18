use std::path::Path;

use clap::Subcommand;
use seedling_protocol::keys::ClientIdentity;

#[derive(Subcommand)]
pub(super) enum ClientCommand {
    /// Print this client's key fingerprint (no server connection needed)
    Fingerprint,
}

pub(super) fn dispatch(cmd: &ClientCommand, identity: &ClientIdentity, key_path: &Path) {
    match cmd {
        ClientCommand::Fingerprint => {
            println!("{}", identity.fingerprint);
            eprintln!("Client key: {}", key_path.display());
            eprintln!(
                "\nTo bootstrap a new server, add this line to $data_dir/authorized_keys:\n  {} my-label",
                identity.fingerprint
            );
        }
    }
}
