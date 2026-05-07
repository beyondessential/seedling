//! Grove signed payload, wire messages, and signing primitives.
//!
//! Canonical encoding: [serde_jcs] (RFC 8785) over the serde representation
//! of [`Payload`], with a `bes.grove/sig/v1\0` domain-separator prefix on
//! the bytes that are actually signed. Signatures are Ed25519 over those
//! bytes, using the leader's transport identity key (see [`super::keys`]).
//!
//! No transport, DB, or filesystem dependency: this module is pure types
//! and pure functions, depended on by both the daemon (commit-4 onward)
//! and `seedling-ctl` (commit-6) so each side can independently sign and
//! verify without going through the core crate.

use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Domain separator prefix prepended to canonical-JSON payload bytes
/// before signing or verifying. Bumping the trailing version requires a
/// new ALPN.
pub const SIG_DOMAIN_V1: &[u8] = b"bes.grove/sig/v1\0";

/// ALPN identifier negotiated for grove gossip on the shared transport.
// g[impl wire.alpn]
pub const GROVE_ALPN: &[u8] = b"bes.grove/1";

/// In-payload wire-protocol version, independent of the ALPN. Bumped for
/// soft (backwards-compatible) feature flags; a hard break uses a new ALPN.
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub fp: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub kind: String,
    pub value: String,
}

/// Reserved for future per-member envelope-encrypted secret parameters.
/// Always empty on the wire in this protocol version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretEntry {
    pub name: String,
}

/// Canonical body of a published grove version.
///
/// Fields are serialised in declaration order; collections are sorted by
/// the relevant key inside [`Self::canonicalise`] so the JCS encoding is
/// deterministic regardless of insertion order.
// g[impl versioning.payload-fields]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Payload {
    pub grove_id: Uuid,
    pub seq: u64,
    pub created_at: Timestamp,
    pub leader_fp: String,
    pub members: Vec<Member>,
    pub params: Vec<Param>,
    /// Always empty on the wire in this protocol version.
    pub secrets: Vec<SecretEntry>,
}

impl Payload {
    /// Sort members by `fp` and params by `name` so that the canonical
    /// encoding is invariant under insertion order.
    pub fn canonicalise(&mut self) {
        self.members.sort_by(|a, b| a.fp.cmp(&b.fp));
        self.params.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Produce the canonical-JSON bytes (RFC 8785) of this payload.
    pub fn canonical_json(&self) -> Result<Vec<u8>, GroveError> {
        serde_jcs::to_vec(self).map_err(GroveError::Encode)
    }

    /// Bytes that are actually signed: domain separator || canonical JSON.
    // g[impl versioning.signature]
    pub fn signing_bytes(&self) -> Result<Vec<u8>, GroveError> {
        let mut buf = SIG_DOMAIN_V1.to_vec();
        buf.extend_from_slice(&self.canonical_json()?);
        Ok(buf)
    }

    /// Length in bytes of [`Self::canonical_json`]. Used by the leader
    /// publish path to enforce the wire-format size cap before bumping
    /// seq or computing the signature.
    pub fn canonical_size(&self) -> Result<usize, GroveError> {
        Ok(self.canonical_json()?.len())
    }

    /// Sign the payload with the leader's signing key.
    ///
    /// Canonicalises first, so callers do not need to remember to.
    // g[impl versioning.signature]
    pub fn sign(mut self, key: &SigningKey) -> Result<SignedPayload, GroveError> {
        self.canonicalise();
        let bytes = self.signing_bytes()?;
        let sig = key.sign(&bytes);
        Ok(SignedPayload {
            payload: self,
            signature: sig.to_bytes().to_vec(),
        })
    }
}

/// A grove payload paired with its Ed25519 signature.
///
/// On the wire, the signature is encoded as a lowercase hex string.
// g[impl wire.version]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedPayload {
    pub payload: Payload,
    #[serde(with = "hex::serde")]
    pub signature: Vec<u8>,
}

impl SignedPayload {
    /// Verify the signature against the leader's verifying key.
    // g[impl versioning.signature]
    pub fn verify(&self, leader: &VerifyingKey) -> Result<(), GroveError> {
        let sig = Signature::try_from(self.signature.as_slice())
            .map_err(|e| GroveError::Signature(format!("malformed signature: {e}")))?;
        let bytes = self.payload.signing_bytes()?;
        leader
            .verify(&bytes, &sig)
            .map_err(|e| GroveError::Signature(format!("verification failed: {e}")))
    }

    /// SHA-256 of the canonical signing bytes, hex-encoded. Used as the
    /// `our_payload_hash` field of a [`Hello`] so that two peers reporting
    /// the same `seq` can detect a leader-side fork.
    pub fn payload_hash(&self) -> Result<String, GroveError> {
        let bytes = self.payload.signing_bytes()?;
        let mut s = String::with_capacity(64);
        for b in Sha256::digest(&bytes) {
            use std::fmt::Write as _;
            write!(s, "{b:02x}").expect("hex write");
        }
        Ok(s)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Leader,
    Follower,
}

/// First message exchanged on a fresh grove connection by both sides.
// g[impl wire.hello]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub grove_id: Uuid,
    pub our_seq: u64,
    pub our_payload_hash: String,
    pub our_role: Role,
    pub our_fingerprint: String,
    pub protocol_version: u16,
    pub nonce: u64,
}

/// Address-hint gossip entry. `last_seen` is sender-supplied and treated
/// as a tie-breaking hint, not as authoritative liveness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerHint {
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub last_seen: Option<Timestamp>,
}

// g[impl wire.peers]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeersGossip {
    pub entries: Vec<PeerHint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    GroveMismatch,
    SignatureInvalid,
    SeqRegression,
    LeaderMismatch,
    VersionTooOld,
    PayloadTooLarge,
    /// Free-form code for documented reasons not yet enumerated; kept open
    /// so the wire format does not need a bump for new reason codes.
    Other(String),
}

// g[impl wire.abort]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Abort {
    pub reason: AbortReason,
}

/// Tagged union for grove gossip messages. The `type` discriminant uses
/// snake-case names: `hello`, `version`, `peers`, `abort`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Version(SignedPayload),
    Peers(PeersGossip),
    Abort(Abort),
}

#[derive(Debug)]
pub enum GroveError {
    Encode(serde_json::Error),
    Signature(String),
}

impl fmt::Display for GroveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "canonical encoding: {e}"),
            Self::Signature(e) => write!(f, "signature: {e}"),
        }
    }
}

impl std::error::Error for GroveError {}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use jiff::Timestamp;
    use rand_core::OsRng;
    use uuid::Uuid;

    use super::*;

    fn sample_payload(seq: u64) -> Payload {
        Payload {
            grove_id: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            seq,
            created_at: Timestamp::from_second(1_700_000_000).unwrap(),
            leader_fp: "fp-leader".into(),
            members: vec![
                Member {
                    fp: "fp-z".into(),
                    label: "z".into(),
                },
                Member {
                    fp: "fp-a".into(),
                    label: "a".into(),
                },
            ],
            params: vec![
                Param {
                    name: "z-param".into(),
                    kind: "text".into(),
                    value: "z-val".into(),
                },
                Param {
                    name: "a-param".into(),
                    kind: "text".into(),
                    value: "a-val".into(),
                },
            ],
            secrets: vec![],
        }
    }

    // g[verify versioning.signature]
    #[test]
    fn sign_and_verify_round_trip() {
        let key = SigningKey::generate(&mut OsRng);
        let signed = sample_payload(1).sign(&key).expect("sign");
        signed.verify(&key.verifying_key()).expect("verify");
    }

    // g[verify versioning.signature]
    #[test]
    fn verify_fails_under_wrong_key() {
        let leader = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let signed = sample_payload(1).sign(&leader).expect("sign");
        let err = signed
            .verify(&other.verifying_key())
            .expect_err("must fail");
        assert!(matches!(err, GroveError::Signature(_)), "got {err:?}");
    }

    // g[verify versioning.signature]
    #[test]
    fn verify_fails_after_payload_mutation() {
        let key = SigningKey::generate(&mut OsRng);
        let mut signed = sample_payload(1).sign(&key).expect("sign");
        signed.payload.seq = 2;
        let err = signed.verify(&key.verifying_key()).expect_err("must fail");
        assert!(matches!(err, GroveError::Signature(_)), "got {err:?}");
    }

    // g[verify versioning.signature]
    #[test]
    fn signing_bytes_starts_with_domain_separator() {
        let payload = sample_payload(1);
        let bytes = payload.signing_bytes().expect("bytes");
        assert!(
            bytes.starts_with(SIG_DOMAIN_V1),
            "signing bytes must start with the domain separator prefix"
        );
    }

    #[test]
    fn canonicalise_sorts_members_and_params() {
        let mut p = sample_payload(1);
        p.canonicalise();
        assert_eq!(
            p.members.iter().map(|m| &m.fp).collect::<Vec<_>>(),
            vec!["fp-a", "fp-z"]
        );
        assert_eq!(
            p.params.iter().map(|q| &q.name).collect::<Vec<_>>(),
            vec!["a-param", "z-param"]
        );
    }

    #[test]
    fn signing_bytes_invariant_under_insertion_order() {
        let key = SigningKey::generate(&mut OsRng);

        let p1 = sample_payload(1);
        let mut p2 = p1.clone();
        p2.members.reverse();
        p2.params.reverse();

        let s1 = p1.sign(&key).expect("sign 1");
        let s2 = p2.sign(&key).expect("sign 2");

        assert_eq!(
            s1.signature, s2.signature,
            "reordering input collections must not change the signed bytes"
        );
    }

    #[test]
    fn sign_already_canonicalises_so_caller_need_not() {
        let key = SigningKey::generate(&mut OsRng);
        let p = sample_payload(1);
        let signed = p.sign(&key).expect("sign");
        // The members in the signed payload are sorted ("fp-a" before "fp-z").
        assert_eq!(signed.payload.members[0].fp, "fp-a");
        assert_eq!(signed.payload.members[1].fp, "fp-z");
    }

    // g[verify wire.hello]
    // g[verify wire.version]
    // g[verify wire.peers]
    // g[verify wire.abort]
    #[test]
    fn message_round_trips_through_json() {
        let key = SigningKey::generate(&mut OsRng);
        let signed = sample_payload(7).sign(&key).expect("sign");

        let messages = [
            Message::Hello(Hello {
                grove_id: Uuid::from_u128(1),
                our_seq: 7,
                our_payload_hash: "deadbeef".into(),
                our_role: Role::Follower,
                our_fingerprint: "fp-follower".into(),
                protocol_version: PROTOCOL_VERSION,
                nonce: 42,
            }),
            Message::Version(signed),
            Message::Peers(PeersGossip {
                entries: vec![PeerHint {
                    fingerprint: "fp-x".into(),
                    addresses: vec!["[::1]:7891".into()],
                    last_seen: None,
                }],
            }),
            Message::Abort(Abort {
                reason: AbortReason::SeqRegression,
            }),
        ];

        for m in &messages {
            let json = serde_json::to_string(m).expect("encode");
            let back: Message = serde_json::from_str(&json).expect("decode");
            assert_eq!(*m, back, "round-trip");
        }
    }

    #[test]
    fn payload_hash_is_stable_under_field_reorder() {
        let key = SigningKey::generate(&mut OsRng);
        let p1 = sample_payload(3);
        let mut p2 = p1.clone();
        p2.members.reverse();

        let s1 = p1.sign(&key).expect("sign 1");
        let s2 = p2.sign(&key).expect("sign 2");
        assert_eq!(
            s1.payload_hash().unwrap(),
            s2.payload_hash().unwrap(),
            "payload_hash must depend only on canonical content"
        );
    }
}
