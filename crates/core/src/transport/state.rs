//! Node-wide transport state shared across all registered application
//! protocols.
//!
//! Built once at daemon startup. Each protocol's own state struct
//! (`OiState`, `GroveState`, …) is constructed with an `Arc<TransportState>`
//! and Arc-clones the fields it surfaces directly so its own handlers
//! continue to access them without indirection.

use std::{path::PathBuf, sync::Arc};

use parking_lot::RwLock;

use crate::transport::{auth::ProtocolTrustRegistry, endpoint::AlpnHandlers};

pub struct TransportState {
    /// Filesystem path of the server's transport identity key (PKCS#8
    /// Ed25519). The key is loaded by [`crate::transport::endpoint::run`]
    /// at startup and used as the server identity for every registered
    /// ALPN.
    pub key_path: PathBuf,
    /// Server SPKI SHA-256 fingerprint. Set once by
    /// [`crate::transport::endpoint::run`] after the key is loaded; visible
    /// thereafter to every state struct that holds an Arc clone.
    pub spki_fingerprint: Arc<std::sync::OnceLock<String>>,
    /// Per-ALPN trusted-keys registry. Each protocol calls
    /// [`ProtocolTrustRegistry::register`] with its own set during the
    /// protocol's `register` call.
    pub trust_registry: Arc<ProtocolTrustRegistry>,
    /// ALPN → handler dispatch table. Each protocol registers its
    /// connection handler here during its `register` call.
    pub handlers: Arc<AlpnHandlers>,
    /// Optional fingerprint → label resolver, used to decorate
    /// per-connection log spans with human-readable peer names. When
    /// multiple protocols are registered, each may install its own; for
    /// now there is at most one (set by OI).
    pub label_lookup: RwLock<Option<crate::transport::endpoint::LabelLookup>>,
}

impl TransportState {
    pub fn new(key_path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            key_path,
            spki_fingerprint: Arc::new(std::sync::OnceLock::new()),
            trust_registry: ProtocolTrustRegistry::new(),
            handlers: AlpnHandlers::new(),
            label_lookup: RwLock::new(None),
        })
    }
}
