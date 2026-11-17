//! The pod network each pod instance is connected to, allocated and recorded.
//!
//! A prefix derived from an instance's identity guarantees uniqueness only to
//! a probability, so the slot an instance's /64 takes is allocated here, with
//! a unique index to stand behind it, and held until its network is removed.

use ipnet::Ipv6Net;
use rusqlite::{OptionalExtension, params};

use crate::{
    defs::resource::ResourceKind, runtime::db::Db, system::translate::proxy::pod_network_prefix,
};

/// How many pod networks one resource kind can hold at once.
pub const SLOTS_PER_KIND: usize = 256;

#[derive(Debug)]
pub enum AllocationError {
    Db(rusqlite::Error),
    /// Every slot of the kind is held by a live network.
    Exhausted {
        kind: u8,
    },
}

impl std::fmt::Display for AllocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "pod network allocation: {e}"),
            Self::Exhausted { kind } => write!(
                f,
                "every pod network prefix for resource kind {kind} is in use"
            ),
        }
    }
}

impl std::error::Error for AllocationError {}

impl From<rusqlite::Error> for AllocationError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

/// The slot recorded for `network_name`, if one has been allocated.
// spec: INFRA#pod-networks
pub fn recorded(db: &Db, network_name: &str) -> rusqlite::Result<Option<u8>> {
    db.conn
        .query_row(
            "SELECT slot FROM pod_networks WHERE network_name = ?1",
            params![network_name],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|slot| slot.map(|s| s as u8))
}

/// Allocate `network_name` a slot not held by another network of its kind, or
/// return the one it already holds.
// spec: INFRA#pod-networks
pub fn allocate(db: &Db, network_name: &str, kind: u8) -> Result<u8, AllocationError> {
    if let Some(slot) = recorded(db, network_name)? {
        return Ok(slot);
    }
    let mut stmt = db
        .conn
        .prepare("SELECT slot FROM pod_networks WHERE kind = ?1")?;
    let mut held = [false; SLOTS_PER_KIND];
    for slot in stmt.query_map(params![kind as i64], |row| row.get::<_, i64>(0))? {
        held[slot? as usize] = true;
    }
    let slot = held
        .iter()
        .position(|taken| !taken)
        .ok_or(AllocationError::Exhausted { kind })? as u8;
    db.conn.execute(
        "INSERT INTO pod_networks (network_name, kind, slot) VALUES (?1, ?2, ?3)",
        params![network_name, kind as i64, slot as i64],
    )?;
    Ok(slot)
}

/// The /64 of the pod network `network_name`, allocating it if it has none.
// spec: INFRA#pod-networks
pub fn allocated_prefix(
    db: &Db,
    node_prefix: &Ipv6Net,
    network_name: &str,
    kind: ResourceKind,
) -> Result<Ipv6Net, AllocationError> {
    let slot = allocate(db, network_name, kind as u8)?;
    Ok(pod_network_prefix(node_prefix, kind, slot))
}

/// The /64 recorded for the pod network `network_name`, if it has one.
// spec: INFRA#pod-networks
pub fn recorded_prefix(
    db: &Db,
    node_prefix: &Ipv6Net,
    network_name: &str,
    kind: ResourceKind,
) -> rusqlite::Result<Option<Ipv6Net>> {
    Ok(recorded(db, network_name)?.map(|slot| pod_network_prefix(node_prefix, kind, slot)))
}

/// Free the slot `network_name` holds, once its network is gone.
pub fn release(db: &Db, network_name: &str) -> rusqlite::Result<()> {
    db.conn.execute(
        "DELETE FROM pod_networks WHERE network_name = ?1",
        params![network_name],
    )?;
    Ok(())
}

/// Free every slot whose network is not in `existing`: a network removed
/// while the daemon was not running leaves its slot held otherwise.
pub fn retain_only(db: &Db, existing: &[String]) -> rusqlite::Result<()> {
    let held: Vec<String> = {
        let mut stmt = db.conn.prepare("SELECT network_name FROM pod_networks")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for name in held.iter().filter(|n| !existing.contains(n)) {
        release(db, name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: INFRA#pod-networks
    #[test]
    fn two_networks_never_share_a_slot_and_a_released_slot_is_reused() {
        let db = Db::open_in_memory().unwrap();
        let a = allocate(&db, "seedling-a", 4).unwrap();
        let b = allocate(&db, "seedling-b", 4).unwrap();
        assert_ne!(a, b);
        assert_eq!(allocate(&db, "seedling-a", 4).unwrap(), a, "idempotent");

        // The same slot under another kind is another prefix.
        assert_eq!(allocate(&db, "seedling-c", 5).unwrap(), a);

        release(&db, "seedling-a").unwrap();
        assert_eq!(allocate(&db, "seedling-d", 4).unwrap(), a);
    }

    // spec: INFRA#pod-networks
    #[test]
    fn a_kind_with_every_slot_held_cannot_allocate_another() {
        let db = Db::open_in_memory().unwrap();
        for n in 0..SLOTS_PER_KIND {
            allocate(&db, &format!("seedling-{n}"), 4).unwrap();
        }
        assert!(matches!(
            allocate(&db, "seedling-one-more", 4),
            Err(AllocationError::Exhausted { kind: 4 })
        ));
    }

    #[test]
    fn retain_only_frees_the_slots_of_networks_that_are_gone() {
        let db = Db::open_in_memory().unwrap();
        allocate(&db, "seedling-keep", 4).unwrap();
        allocate(&db, "seedling-gone", 4).unwrap();
        retain_only(&db, &["seedling-keep".to_owned()]).unwrap();
        assert!(recorded(&db, "seedling-keep").unwrap().is_some());
        assert!(recorded(&db, "seedling-gone").unwrap().is_none());
    }
}
