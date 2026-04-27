pub mod actor;
pub mod backup_actions;
pub mod client;
pub mod env;
pub mod error;
pub mod events;
pub mod keys;
pub mod names;

// i[transport.alpn]
/// ALPN identifier negotiated for OI QUIC connections.
///
/// Bumping this string is the lever for incompatible protocol revisions —
/// the handshake will then fail against peers that only offer the old id.
pub const OI_ALPN: &[u8] = b"oi1";
