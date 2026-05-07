//! Per-node grove state.
//!
//! Built on top of [`crate::transport::TransportState`]. One [`GroveState`]
//! per node; the publish mutex serialises leader-side mutation, and the
//! cached signed payload provides the in-memory view of "current grove"
//! that handlers and the dial loop read.

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use seedling_protocol::grove::SignedPayload;

use crate::grove::db::{self, Membership};
use crate::runtime::db::DbHandle;
use crate::transport::TransportState;
use crate::transport::auth::{TrustedKeys, new_trusted_keys};

pub struct GroveState {
    pub transport: Arc<TransportState>,
    pub db: DbHandle,
    /// Grove trust set: SPKI fingerprints of the current grove's members.
    /// Reconciled by the apply pipeline (lands in commit 5) on every
    /// payload-applied event. Registered against the shared trust registry
    /// in [`Self::register`].
    pub trust: TrustedKeys,
    /// Single-writer lock for the leader publish pathway. Serialises
    /// (load current → mutate → bump seq → sign → persist) so the
    /// monotonic-seq invariant holds under concurrent operator-driven
    /// mutations.
    // g[impl versioning.seq]
    pub publish_mutex: Mutex<()>,
    /// In-memory cache of the latest applied signed payload, or `None` if
    /// this node is not yet in any grove. Refreshed on construction and
    /// on every payload-applied.
    pub current: RwLock<Option<SignedPayload>>,
}

#[derive(Debug)]
pub enum LoadError {
    Db(db::LoadError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<db::LoadError> for LoadError {
    fn from(e: db::LoadError) -> Self {
        Self::Db(e)
    }
}

impl GroveState {
    /// Construct a [`GroveState`] and populate the in-memory cache from the
    /// DB. The grove trust set is also seeded from the latest signed
    /// payload's membership list, so a daemon restart preserves grove
    /// authorisation without waiting for a peer connection.
    pub fn load(transport: Arc<TransportState>, db: DbHandle) -> Result<Arc<Self>, LoadError> {
        let trust = new_trusted_keys();
        let membership: Option<Membership> = db.call(db::load_membership)?;

        if let Some(m) = &membership {
            let mut t = trust.write();
            for member in &m.current.payload.members {
                t.insert(member.fp.clone());
            }
        }

        Ok(Arc::new(Self {
            transport,
            db,
            trust,
            publish_mutex: Mutex::new(()),
            current: RwLock::new(membership.map(|m| m.current)),
        }))
    }

    /// Whether this node currently belongs to a grove.
    pub fn is_member(&self) -> bool {
        self.current.read().is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ed25519_dalek::SigningKey;
    use jiff::Timestamp;
    use rand_core::OsRng;
    use uuid::Uuid;

    use seedling_protocol::grove::{Member, Param, Payload, Role, SignedPayload};

    use super::*;

    fn fresh_transport() -> Arc<TransportState> {
        TransportState::new(PathBuf::from("/tmp/seedling-grove-test-key"))
    }

    fn signed_payload_with_members(members: Vec<&str>) -> SignedPayload {
        let key = SigningKey::generate(&mut OsRng);
        Payload {
            grove_id: Uuid::from_u128(1),
            seq: 1,
            created_at: Timestamp::from_second(1_700_000_000).unwrap(),
            leader_fp: members.first().copied().unwrap_or("fp-leader").into(),
            members: members
                .into_iter()
                .map(|fp| Member {
                    fp: fp.into(),
                    label: fp.into(),
                })
                .collect(),
            params: vec![Param {
                name: "p".into(),
                kind: "text".into(),
                value: "v".into(),
            }],
            secrets: vec![],
        }
        .sign(&key)
        .expect("sign")
    }

    #[test]
    fn load_returns_state_without_membership_on_fresh_db() {
        let db = DbHandle::open_in_memory().expect("db");
        let state = GroveState::load(fresh_transport(), db).expect("load");
        assert!(!state.is_member());
        assert!(state.trust.read().is_empty());
    }

    // g[verify trust]
    #[test]
    fn load_seeds_trust_set_from_persisted_membership() {
        let db = DbHandle::open_in_memory().expect("db");
        let signed = signed_payload_with_members(vec!["fp-a", "fp-b"]);
        let m = Membership {
            grove_id: signed.payload.grove_id,
            role: Role::Leader,
            leader_fingerprint: signed.payload.leader_fp.clone(),
            joined_at: Timestamp::from_second(1_700_000_000).unwrap(),
            current: signed,
        };
        db.call(move |db| db::write_membership(db, &m).expect("write"));

        let state = GroveState::load(fresh_transport(), db).expect("load");
        assert!(state.is_member());
        let trusted = state.trust.read();
        assert!(trusted.contains("fp-a"));
        assert!(trusted.contains("fp-b"));
        assert_eq!(trusted.len(), 2);
    }
}
