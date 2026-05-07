//! Read/write helpers over the v53 grove tables.
//!
//! All functions take a [`crate::runtime::db::Db`] reference and run inline
//! on the DB thread, mirroring the pattern in `oi/auth.rs`. Callers wrap
//! them in `state.db.call(...)` to dispatch onto the DB actor.

use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use seedling_protocol::grove::{Param, Payload, Role, SignedPayload};

use crate::runtime::db::Db;

/// One row of `grove_membership` (id is implicitly 1) decoded into a
/// typed [`SignedPayload`] plus the metadata captured when this node
/// joined the grove.
#[derive(Debug, Clone)]
pub struct Membership {
    pub grove_id: Uuid,
    pub role: Role,
    pub leader_fingerprint: String,
    pub joined_at: Timestamp,
    pub current: SignedPayload,
}

#[derive(Debug)]
pub enum LoadError {
    Sql(rusqlite::Error),
    UuidLength(usize),
    UnknownRole(String),
    PayloadDecode(serde_json::Error),
    TimestampDecode(jiff::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "sql: {e}"),
            Self::UuidLength(n) => write!(f, "grove_id is {n} bytes, expected 16"),
            Self::UnknownRole(s) => write!(f, "unknown role: {s:?}"),
            Self::PayloadDecode(e) => write!(f, "payload decode: {e}"),
            Self::TimestampDecode(e) => write!(f, "timestamp decode: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<rusqlite::Error> for LoadError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// Load this node's grove membership, or `None` if it is not in any grove.
// g[impl identity]
pub fn load_membership(db: &Db) -> Result<Option<Membership>, LoadError> {
    let row = db
        .conn
        .query_row(
            "SELECT grove_id, role, leader_fingerprint, current_payload, current_signature, joined_at \
             FROM grove_membership WHERE id = 1",
            [],
            |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((grove_id_bytes, role_s, leader_fp, payload_bytes, sig_bytes, joined_at_s)) = row
    else {
        return Ok(None);
    };

    let grove_id = match grove_id_bytes.len() {
        16 => Uuid::from_bytes(grove_id_bytes.as_slice().try_into().unwrap()),
        n => return Err(LoadError::UuidLength(n)),
    };
    let role = match role_s.as_str() {
        "leader" => Role::Leader,
        "follower" => Role::Follower,
        _ => return Err(LoadError::UnknownRole(role_s)),
    };
    let payload: Payload =
        serde_json::from_slice(&payload_bytes).map_err(LoadError::PayloadDecode)?;
    let joined_at = joined_at_s
        .parse::<Timestamp>()
        .map_err(LoadError::TimestampDecode)?;

    Ok(Some(Membership {
        grove_id,
        role,
        leader_fingerprint: leader_fp,
        joined_at,
        current: SignedPayload {
            payload,
            signature: sig_bytes,
        },
    }))
}

/// Persist this node's grove membership and the currently-applied signed
/// payload. Used by both `grove init` (initial membership row) and
/// payload-apply (subsequent updates to the same row). Always runs on
/// `id = 1`; the table's CHECK constraint enforces the single-row invariant.
// g[impl identity]
// g[impl membership.canonical]
pub fn write_membership(db: &Db, m: &Membership) -> Result<(), LoadError> {
    let payload_bytes = serde_json::to_vec(&m.current.payload).map_err(LoadError::PayloadDecode)?;
    db.conn.execute(
        "INSERT INTO grove_membership \
            (id, grove_id, role, leader_fingerprint, current_seq, current_payload, current_signature, joined_at) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(id) DO UPDATE SET \
             grove_id           = excluded.grove_id, \
             role               = excluded.role, \
             leader_fingerprint = excluded.leader_fingerprint, \
             current_seq        = excluded.current_seq, \
             current_payload    = excluded.current_payload, \
             current_signature  = excluded.current_signature",
        params![
            m.grove_id.as_bytes().to_vec(),
            match m.role {
                Role::Leader => "leader",
                Role::Follower => "follower",
            },
            m.leader_fingerprint,
            m.current.payload.seq as i64,
            payload_bytes,
            m.current.signature,
            m.joined_at.to_string(),
        ],
    )?;
    Ok(())
}

/// Insert a payload into the audit history, idempotent on `seq`. Returns
/// `true` if a row was inserted, `false` if the seq was already present.
pub fn insert_version(
    db: &Db,
    signed: &SignedPayload,
    received_at: Timestamp,
) -> Result<bool, LoadError> {
    let payload_bytes = serde_json::to_vec(&signed.payload).map_err(LoadError::PayloadDecode)?;
    let rows = db.conn.execute(
        "INSERT OR IGNORE INTO grove_versions (seq, payload, signature, received_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            signed.payload.seq as i64,
            payload_bytes,
            signed.signature,
            received_at.to_string(),
        ],
    )?;
    Ok(rows == 1)
}

/// Replace the denormalised `grove_params` projection to match the given
/// payload's parameter list. Atomic with respect to other DB writes by
/// virtue of the DB actor's serialisation; callers wanting transactional
/// atomicity with a `write_membership` in the same closure must wrap both
/// in a `Db::conn::unchecked_transaction`.
// g[impl params.set]
pub fn replace_params(db: &Db, version_seq: u64, params_list: &[Param]) -> Result<(), LoadError> {
    db.conn.execute("DELETE FROM grove_params", [])?;
    let mut stmt = db.conn.prepare(
        "INSERT INTO grove_params (name, kind, value, version_seq) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for p in params_list {
        stmt.execute(params![p.name, p.kind, p.value, version_seq as i64])?;
    }
    Ok(())
}

/// Read back the denormalised grove parameters in name order.
pub fn list_params(db: &Db) -> Result<Vec<(Param, u64)>, LoadError> {
    let mut stmt = db
        .conn
        .prepare("SELECT name, kind, value, version_seq FROM grove_params ORDER BY name ASC")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            Param {
                name: r.get(0)?,
                kind: r.get(1)?,
                value: r.get(2)?,
            },
            r.get::<_, i64>(3)? as u64,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use jiff::Timestamp;
    use rand_core::OsRng;
    use seedling_protocol::grove::{Member, Param, Payload};
    use uuid::Uuid;

    use super::*;

    fn fresh_db() -> Db {
        Db::open_in_memory().expect("in-memory db")
    }

    fn sample_signed(seq: u64) -> SignedPayload {
        let key = SigningKey::generate(&mut OsRng);
        Payload {
            grove_id: Uuid::from_u128(0xfeed_face_cafe_babe_dead_beef_0000_0001),
            seq,
            created_at: Timestamp::from_second(1_700_000_000).unwrap(),
            leader_fp: "fp-leader".into(),
            members: vec![Member {
                fp: "fp-leader".into(),
                label: "leader".into(),
            }],
            params: vec![Param {
                name: "greeting".into(),
                kind: "text".into(),
                value: "hello".into(),
            }],
            secrets: vec![],
        }
        .sign(&key)
        .expect("sign")
    }

    fn sample_membership(seq: u64) -> Membership {
        let signed = sample_signed(seq);
        Membership {
            grove_id: signed.payload.grove_id,
            role: Role::Leader,
            leader_fingerprint: signed.payload.leader_fp.clone(),
            joined_at: Timestamp::from_second(1_700_000_000).unwrap(),
            current: signed,
        }
    }

    // g[verify identity]
    #[test]
    fn load_membership_returns_none_on_fresh_db() {
        let db = fresh_db();
        assert!(load_membership(&db).unwrap().is_none());
    }

    // g[verify identity]
    // g[verify membership.canonical]
    #[test]
    fn write_then_load_membership_round_trips() {
        let db = fresh_db();
        let m = sample_membership(1);
        write_membership(&db, &m).expect("write");
        let loaded = load_membership(&db).expect("load").expect("present");
        assert_eq!(loaded.grove_id, m.grove_id);
        assert_eq!(loaded.role, m.role);
        assert_eq!(loaded.leader_fingerprint, m.leader_fingerprint);
        assert_eq!(loaded.joined_at, m.joined_at);
        assert_eq!(loaded.current.payload.seq, m.current.payload.seq);
        assert_eq!(loaded.current.signature, m.current.signature);
    }

    #[test]
    fn write_membership_replaces_existing_row() {
        let db = fresh_db();
        write_membership(&db, &sample_membership(1)).unwrap();
        write_membership(&db, &sample_membership(2)).unwrap();
        let loaded = load_membership(&db).unwrap().unwrap();
        assert_eq!(loaded.current.payload.seq, 2);
    }

    #[test]
    fn insert_version_is_idempotent_on_seq() {
        let db = fresh_db();
        let s1 = sample_signed(1);
        let inserted_first = insert_version(&db, &s1, Timestamp::now()).unwrap();
        let inserted_second = insert_version(&db, &s1, Timestamp::now()).unwrap();
        assert!(inserted_first);
        assert!(!inserted_second);
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM grove_versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    // g[verify params.set]
    #[test]
    fn replace_params_overwrites_previous_set() {
        let db = fresh_db();
        replace_params(
            &db,
            1,
            &[Param {
                name: "a".into(),
                kind: "text".into(),
                value: "first".into(),
            }],
        )
        .unwrap();
        replace_params(
            &db,
            2,
            &[
                Param {
                    name: "b".into(),
                    kind: "text".into(),
                    value: "second".into(),
                },
                Param {
                    name: "a".into(),
                    kind: "text".into(),
                    value: "first-updated".into(),
                },
            ],
        )
        .unwrap();
        let loaded = list_params(&db).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].0.name, "a");
        assert_eq!(loaded[0].0.value, "first-updated");
        assert_eq!(loaded[0].1, 2);
        assert_eq!(loaded[1].0.name, "b");
        assert_eq!(loaded[1].1, 2);
    }
}
