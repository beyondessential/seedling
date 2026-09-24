pub mod actor;
pub mod backup_actions;
pub mod canopy;
pub mod client;
pub mod env;
pub mod error;
pub mod events;
pub mod keys;
pub mod names;

/// Upper bound on an OI request, and on the response to one.
///
/// It governs the daemon's read, the client's read of the reply, and every
/// hop in between, so a request one end would accept is not cut off by
/// another: a pushed definition bundle is the large case.
pub const REQUEST_LIMIT: usize = 4 * 1024 * 1024;

// i[transport.alpn]
/// ALPN identifier negotiated for OI QUIC connections.
///
/// Bumping this string is the lever for incompatible protocol revisions —
/// the handshake will then fail against peers that only offer the old id.
pub const OI_ALPN: &[u8] = b"bes.seedling/1";
