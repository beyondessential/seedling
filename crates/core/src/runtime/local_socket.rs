//! HTTP clients for services the host serves on a Unix socket: the proxy's
//! admin API and the Tailscale daemon's local API.

use std::path::Path;

/// A client builder whose every connection goes through the Unix socket at
/// `socket`.
#[cfg(unix)]
pub(crate) fn client_builder(socket: &Path) -> reqwest::ClientBuilder {
    reqwest::Client::builder().unix_socket(socket)
}

#[cfg(not(unix))]
pub(crate) fn client_builder(_socket: &Path) -> reqwest::ClientBuilder {
    todo!("reaching a local service on a platform without Unix sockets")
}
