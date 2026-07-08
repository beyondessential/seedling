//! Leader-side publish operations.
//!
//! All mutating leader actions (`grove init`, `grove invite`, `grove
//! revoke`, `grove param set`/`unset`) flow through one of two entry
//! points: [`GroveState::init`] (no current membership) or
//! [`GroveState::publish`] (current membership exists). Both serialise on
//! [`GroveState::publish_mutex`] so the seq is monotonic and the on-disk
//! membership row is always consistent with the latest signed payload.

use ed25519_dalek::SigningKey;
use jiff::Timestamp;
use uuid::Uuid;

use seedling_protocol::grove::{
    GroveError as ProtoError, Member, Param, Payload, Role, SignedPayload,
};
use seedling_protocol::keys;

use crate::grove::db::{self, Membership};
use crate::grove::state::GroveState;

/// Hard maximum on the canonical-JSON byte length of a published payload.
/// Receivers reject anything larger; the leader's pre-publish check
/// reserves headroom below this.
// g[impl versioning.size-cap]
pub const PAYLOAD_SIZE_CAP_BYTES: usize = 256 * 1024;

/// Reservation kept below [`PAYLOAD_SIZE_CAP_BYTES`]. Leader mutations
/// that would push the next payload past `cap - headroom` are rejected
/// at the point of mutation, before seq is bumped or the signature is
/// computed.
pub const PAYLOAD_SIZE_HEADROOM_BYTES: usize = 16 * 1024;

/// Effective leader cap after subtracting the headroom reservation.
pub const PAYLOAD_PUBLISH_CAP_BYTES: usize = PAYLOAD_SIZE_CAP_BYTES - PAYLOAD_SIZE_HEADROOM_BYTES;

#[derive(Debug)]
pub enum PublishError {
    /// This node is not a member of any grove. Use [`GroveState::init`] first.
    NotMember,
    /// This node is a member but is a follower; only the leader may publish.
    NotLeader,
    /// `init` was called on a node that is already a member.
    AlreadyMember,
    /// The supplied signing key's SPKI fingerprint does not match the grove's
    /// pinned `leader_fingerprint`. Almost always a configuration error.
    LeaderKeyMismatch {
        expected: String,
        actual: String,
    },
    /// The candidate payload's canonical-JSON encoding exceeds the leader
    /// publish cap. Operators must shrink the change set before retrying.
    // g[impl versioning.size-cap]
    PayloadTooLarge {
        current_bytes: usize,
        cap_bytes: usize,
    },
    /// In-version v0, secret grove parameters are rejected at the publish
    /// boundary. The signed-payload `secrets` field is reserved but
    /// always-empty in this version.
    // g[impl params.no-secrets]
    SecretsNotSupported,
    Sign(ProtoError),
    Db(db::LoadError),
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMember => write!(f, "not a member of any grove"),
            Self::NotLeader => write!(f, "publish requires the leader role"),
            Self::AlreadyMember => write!(f, "node is already a member of a grove"),
            Self::LeaderKeyMismatch { expected, actual } => write!(
                f,
                "leader key fingerprint {actual} does not match pinned leader {expected}"
            ),
            Self::PayloadTooLarge {
                current_bytes,
                cap_bytes,
            } => write!(
                f,
                "next payload would be {current_bytes} bytes, exceeding the {cap_bytes}-byte publish cap"
            ),
            Self::SecretsNotSupported => write!(
                f,
                "secret grove params are not supported in this protocol version"
            ),
            Self::Sign(e) => write!(f, "sign: {e}"),
            Self::Db(e) => write!(f, "db: {e}"),
        }
    }
}

impl std::error::Error for PublishError {}

/// View of the payload fields a leader operation may mutate. Other
/// fields (`grove_id`, `seq`, `created_at`, `leader_fp`, `secrets`) are
/// managed by [`GroveState::publish`] itself.
pub struct PendingPayload {
    pub members: Vec<Member>,
    pub params: Vec<Param>,
}

impl GroveState {
    /// Initialise a new grove with this node as leader. Generates a fresh
    /// `grove_id`, creates a seq=1 self-only signed payload, persists,
    /// and returns the signed payload.
    ///
    /// Fails with [`PublishError::AlreadyMember`] if this node is already
    /// in a grove.
    // g[impl bootstrap.init]
    // g[impl membership.bootstrap]
    pub fn init(
        &self,
        leader_key: &SigningKey,
        leader_label: String,
    ) -> Result<SignedPayload, PublishError> {
        let _guard = self.publish_mutex.lock();
        let existing: Option<Membership> = self
            .db
            .call(db::load_membership)
            .map_err(PublishError::Db)?;
        if existing.is_some() {
            return Err(PublishError::AlreadyMember);
        }

        let leader_fp = keys::fingerprint(&keys::spki_der(leader_key));
        let now = Timestamp::now();

        let payload = Payload {
            grove_id: Uuid::new_v4(),
            seq: 1,
            created_at: now,
            leader_fp: leader_fp.clone(),
            members: vec![Member {
                fp: leader_fp.clone(),
                label: leader_label,
            }],
            params: Vec::new(),
            secrets: Vec::new(),
        };

        self.finalise_publish(payload, leader_key, Role::Leader, leader_fp, now)
    }

    /// Run a leader-side publish operation. Acquires the publish mutex,
    /// loads the current membership from the DB inside the lock, applies
    /// `mutate` to a clone of the current members + params, bumps seq,
    /// canonicalises, size-checks, signs, and persists atomically.
    // g[impl versioning.seq]
    // g[impl membership.invite]
    // g[impl membership.revoke]
    // g[impl params.set]
    // g[impl surface.role-gate]
    pub fn publish<F>(
        &self,
        leader_key: &SigningKey,
        mutate: F,
    ) -> Result<SignedPayload, PublishError>
    where
        F: FnOnce(&mut PendingPayload),
    {
        let _guard = self.publish_mutex.lock();
        let m: Membership = self
            .db
            .call(db::load_membership)
            .map_err(PublishError::Db)?
            .ok_or(PublishError::NotMember)?;
        if m.role != Role::Leader {
            return Err(PublishError::NotLeader);
        }

        let leader_fp = keys::fingerprint(&keys::spki_der(leader_key));
        if leader_fp != m.leader_fingerprint {
            return Err(PublishError::LeaderKeyMismatch {
                expected: m.leader_fingerprint.clone(),
                actual: leader_fp,
            });
        }

        let mut pending = PendingPayload {
            members: m.current.payload.members.clone(),
            params: m.current.payload.params.clone(),
        };
        mutate(&mut pending);

        let now = Timestamp::now();
        let payload = Payload {
            grove_id: m.current.payload.grove_id,
            seq: m.current.payload.seq + 1,
            created_at: now,
            leader_fp: leader_fp.clone(),
            members: pending.members,
            params: pending.params,
            secrets: Vec::new(),
        };

        self.finalise_publish(payload, leader_key, Role::Leader, leader_fp, m.joined_at)
    }

    fn finalise_publish(
        &self,
        mut payload: Payload,
        leader_key: &SigningKey,
        role: Role,
        leader_fp: String,
        joined_at: Timestamp,
    ) -> Result<SignedPayload, PublishError> {
        // g[impl params.no-secrets]
        if !payload.secrets.is_empty() {
            return Err(PublishError::SecretsNotSupported);
        }
        payload.canonicalise();
        let size = payload.canonical_size().map_err(PublishError::Sign)?;
        if size > PAYLOAD_PUBLISH_CAP_BYTES {
            return Err(PublishError::PayloadTooLarge {
                current_bytes: size,
                cap_bytes: PAYLOAD_PUBLISH_CAP_BYTES,
            });
        }
        let signed = payload.sign(leader_key).map_err(PublishError::Sign)?;

        let m = Membership {
            grove_id: signed.payload.grove_id,
            role,
            leader_fingerprint: leader_fp,
            joined_at,
            current: signed.clone(),
        };
        let signed_for_history = signed.clone();
        let received_at = Timestamp::now();
        let params_list = signed.payload.params.clone();
        let new_seq = signed.payload.seq;

        self.db
            .call(move |db| -> Result<(), db::LoadError> {
                let tx = db.conn.unchecked_transaction()?;
                db::write_membership(db, &m)?;
                db::insert_version(db, &signed_for_history, received_at)?;
                db::replace_params(db, new_seq, &params_list)?;
                tx.commit()?;
                Ok(())
            })
            .map_err(PublishError::Db)?;

        // g[impl trust]
        *self.current.write() = Some(signed.clone());
        let mut trust = self.trust.write();
        trust.clear();
        for member in &signed.payload.members {
            trust.insert(member.fp.clone());
        }

        Ok(signed)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use ed25519_dalek::SigningKey;
    use rand_core::OsRng;

    use seedling_protocol::keys;

    use super::*;
    use crate::runtime::db::DbHandle;
    use crate::transport::TransportState;

    fn fresh_state() -> (Arc<GroveState>, SigningKey) {
        let db = DbHandle::open_in_memory().expect("db");
        let transport = TransportState::new(PathBuf::from("/tmp/grove-publish-test-key"));
        let state = GroveState::load(transport, db).expect("state");
        (state, SigningKey::generate(&mut OsRng))
    }

    // g[verify bootstrap.init]
    // g[verify membership.bootstrap]
    #[test]
    fn init_creates_seq_one_self_only_payload() {
        let (state, key) = fresh_state();
        let signed = state.init(&key, "leader".into()).expect("init");
        assert_eq!(signed.payload.seq, 1);
        assert_eq!(signed.payload.members.len(), 1);
        let leader_fp = keys::fingerprint(&keys::spki_der(&key));
        assert_eq!(signed.payload.members[0].fp, leader_fp);
        assert_eq!(signed.payload.leader_fp, leader_fp);
        assert!(state.is_member());
        assert!(state.trust.read().contains(&leader_fp));
    }

    #[test]
    fn init_fails_if_already_a_member() {
        let (state, key) = fresh_state();
        state.init(&key, "leader".into()).unwrap();
        let err = state
            .init(&key, "leader".into())
            .expect_err("second init must fail");
        assert!(matches!(err, PublishError::AlreadyMember));
    }

    // g[verify versioning.seq]
    // g[verify membership.invite]
    #[test]
    fn publish_after_init_invite_bumps_seq_and_extends_membership() {
        let (state, key) = fresh_state();
        state.init(&key, "leader".into()).unwrap();
        let signed = state
            .publish(&key, |p| {
                p.members.push(Member {
                    fp: "fp-new".into(),
                    label: "n".into(),
                });
            })
            .expect("invite");
        assert_eq!(signed.payload.seq, 2);
        assert_eq!(signed.payload.members.len(), 2);
        // canonicalise sorts alphabetically by fp; "fp-new" precedes the
        // leader fingerprint only if it sorts that way, so just check both
        // are present.
        let fps: Vec<&str> = signed
            .payload
            .members
            .iter()
            .map(|m| m.fp.as_str())
            .collect();
        assert!(fps.contains(&"fp-new"));
        let trust = state.trust.read();
        assert_eq!(trust.len(), 2);
        assert!(trust.contains("fp-new"));
    }

    // g[verify params.set]
    #[test]
    fn publish_param_set_appears_in_grove_params_table() {
        let (state, key) = fresh_state();
        state.init(&key, "leader".into()).unwrap();
        state
            .publish(&key, |p| {
                p.params.push(Param {
                    name: "greeting".into(),
                    kind: "text".into(),
                    value: "hello".into(),
                });
            })
            .unwrap();
        let listed = state.db.call(db::list_params).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0.name, "greeting");
        assert_eq!(listed[0].0.value, "hello");
        assert_eq!(listed[0].1, 2);
    }

    // g[verify surface.role-gate]
    #[test]
    fn publish_fails_on_non_member() {
        let (state, key) = fresh_state();
        let err = state.publish(&key, |_| {}).expect_err("must fail");
        assert!(matches!(err, PublishError::NotMember));
    }

    // g[verify versioning.size-cap]
    #[test]
    fn publish_rejects_oversize_payload() {
        let (state, key) = fresh_state();
        state.init(&key, "leader".into()).unwrap();
        let err = state
            .publish(&key, |p| {
                // Push enough members to blow past the publish cap.
                let big_label = "x".repeat(1024);
                for i in 0..1024 {
                    p.members.push(Member {
                        fp: format!("fp-padding-{i:08}"),
                        label: big_label.clone(),
                    });
                }
            })
            .expect_err("must reject");
        match err {
            PublishError::PayloadTooLarge {
                current_bytes,
                cap_bytes,
            } => {
                assert!(
                    current_bytes > cap_bytes,
                    "got {current_bytes} <= cap {cap_bytes}"
                );
            }
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
        // Seq did not advance.
        let m = state
            .db
            .call(db::load_membership)
            .unwrap()
            .expect("present");
        assert_eq!(m.current.payload.seq, 1);
    }

    #[test]
    fn publish_rejects_wrong_leader_key() {
        let (state, leader_key) = fresh_state();
        state.init(&leader_key, "leader".into()).unwrap();
        let other_key = SigningKey::generate(&mut OsRng);
        let err = state.publish(&other_key, |_| {}).expect_err("must reject");
        assert!(matches!(err, PublishError::LeaderKeyMismatch { .. }));
    }

    #[test]
    fn publish_signature_is_verifiable_against_leader_key() {
        let (state, key) = fresh_state();
        let signed = state.init(&key, "leader".into()).unwrap();
        signed
            .verify(&key.verifying_key())
            .expect("signature must verify against leader key");
    }
}
