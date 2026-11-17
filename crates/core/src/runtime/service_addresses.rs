//! The address each service is reached at on the host, allocated and recorded.
//!
//! An address derived from an instance's identity is unique only to a
//! probability, so the derived address is a service's first choice, and the
//! address it takes is recorded here, with a unique index to stand behind it.
//! A service whose first choice is taken takes the next free one.

use std::{collections::HashSet, net::Ipv6Addr};

use ipnet::Ipv6Net;
use rusqlite::{OptionalExtension, params};

use crate::{
    runtime::{db::Db, history, identity::ResourceInstance},
    system::translate::proxy::instance_ipv6,
};

/// How many addresses past the first choice are tried before giving up.
const MAX_PROBES: u32 = 4096;

/// The part of the address after the host prefix: the resource kind byte that
/// keeps services apart from pod networks and infrastructure, and the rest.
fn suffix_of(addr: Ipv6Addr) -> String {
    addr.octets()[6..]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn with_suffix(node_prefix: &Ipv6Net, suffix: &str) -> Option<Ipv6Addr> {
    if suffix.len() != 20 || !suffix.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&node_prefix.network().octets()[..6]);
    for (i, byte) in bytes[6..].iter_mut().enumerate() {
        *byte = u8::from_str_radix(&suffix[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(Ipv6Addr::from(bytes))
}

/// The address after `addr`, within the same resource kind's range: only the
/// last eight bytes move, so the kind byte, and with it the disjointness from
/// pod networks and infrastructure, holds.
fn next(addr: Ipv6Addr) -> Ipv6Addr {
    let mut bytes = addr.octets();
    let low = u64::from_be_bytes(bytes[8..].try_into().expect("eight bytes")).wrapping_add(1);
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    Ipv6Addr::from(bytes)
}

fn unreadable(suffix: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("recorded service address suffix {suffix:?} is not an address").into(),
    )
}

/// The address recorded for `instance`, allocating one not held by another
/// service if it has none.
// spec: INFRA#service-addresses
pub fn address_of(
    db: &Db,
    node_prefix: &Ipv6Net,
    instance: &ResourceInstance,
) -> rusqlite::Result<Ipv6Addr> {
    db.atomically(|db| {
        let id = instance.id.to_hex();
        let recorded: Option<String> = db
            .conn
            .query_row(
                "SELECT suffix FROM service_addresses WHERE instance_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(suffix) = recorded {
            return with_suffix(node_prefix, &suffix).ok_or_else(|| unreadable(&suffix));
        }

        history::insert_instance(db, instance)?;
        let mut candidate = instance_ipv6(node_prefix, instance);
        for _ in 0..MAX_PROBES {
            let taken = db.conn.execute(
                "INSERT INTO service_addresses (instance_id, suffix) VALUES (?1, ?2)
                 ON CONFLICT (suffix) DO NOTHING",
                params![id, suffix_of(candidate)],
            )?;
            if taken == 1 {
                return Ok(candidate);
            }
            candidate = next(candidate);
        }
        Err(rusqlite::Error::ToSqlConversionFailure(
            format!(
                "no free address for service instance {} within {MAX_PROBES} of its first choice",
                instance.display_name
            )
            .into(),
        ))
    })
}

/// The same allocation, held in memory, for a registry that records nothing.
#[derive(Default)]
pub struct InMemory {
    by_instance: std::collections::HashMap<crate::runtime::identity::InstanceId, Ipv6Addr>,
    taken: HashSet<Ipv6Addr>,
}

impl InMemory {
    // spec: INFRA#service-addresses
    pub fn address_of(&mut self, node_prefix: &Ipv6Net, instance: &ResourceInstance) -> Ipv6Addr {
        if let Some(addr) = self.by_instance.get(&instance.id) {
            return *addr;
        }
        let mut candidate = instance_ipv6(node_prefix, instance);
        while self.taken.contains(&candidate) {
            candidate = next(candidate);
        }
        self.taken.insert(candidate);
        self.by_instance.insert(instance.id, candidate);
        candidate
    }
}

#[cfg(test)]
mod tests {
    use seedling_protocol::names::AppName;

    use super::*;
    use crate::{
        defs::resource::ResourceKind,
        runtime::identity::{InstanceId, InstanceVariant},
        system::translate::proxy::{node_mount_addr, pod_network_prefix},
    };

    fn prefix() -> Ipv6Net {
        "fd5e:ed12:3456::/48".parse().unwrap()
    }

    fn service(name: &str, id: InstanceId) -> ResourceInstance {
        ResourceInstance {
            id,
            app: AppName::new_unchecked("demo"),
            kind: ResourceKind::Service,
            name: Some(name.to_owned()),
            variant: InstanceVariant::Singleton,
            display_name: format!("demo-svc-{name}"),
        }
    }

    /// Two ids that agree on every byte the derived address is taken from.
    fn colliding() -> (InstanceId, InstanceId) {
        let a = uuid::Uuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 1]);
        let b = uuid::Uuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 2]);
        (InstanceId(a), InstanceId(b))
    }

    // spec: INFRA#service-addresses
    #[test]
    fn two_services_whose_derived_addresses_collide_are_given_different_addresses() {
        let db = Db::open_in_memory().unwrap();
        let (a, b) = colliding();
        let (a, b) = (service("a", a), service("b", b));
        assert_eq!(instance_ipv6(&prefix(), &a), instance_ipv6(&prefix(), &b));

        let first = address_of(&db, &prefix(), &a).unwrap();
        let second = address_of(&db, &prefix(), &b).unwrap();
        assert_eq!(
            first,
            instance_ipv6(&prefix(), &a),
            "the first keeps its own"
        );
        assert_ne!(first, second);
        assert_eq!(address_of(&db, &prefix(), &b).unwrap(), second, "recorded");

        let mut memory = InMemory::default();
        assert_ne!(
            memory.address_of(&prefix(), &a),
            memory.address_of(&prefix(), &b)
        );
    }

    // spec: INFRA#service-addresses
    #[test]
    fn a_service_address_lies_outside_pod_networks_and_infrastructure() {
        let db = Db::open_in_memory().unwrap();
        let mut addrs = Vec::new();
        for n in 0..64 {
            let instance = service(&format!("s{n}"), InstanceId::generate());
            addrs.push(address_of(&db, &prefix(), &instance).unwrap());
        }
        // Probing moves only the low bytes, never the kind.
        addrs.push(next(Ipv6Addr::from([
            0xfd, 0x5e, 0xed, 0x12, 0x34, 0x56, 1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff,
        ])));
        for addr in addrs {
            for kind in [ResourceKind::Deployment, ResourceKind::Job] {
                for slot in [0, 1, 255] {
                    assert!(!pod_network_prefix(&prefix(), kind, slot).contains(&addr));
                }
            }
            assert_ne!(addr, node_mount_addr(&prefix()));
            assert_ne!(addr, crate::system::resolver::resolver_addr(&prefix()));
            assert!(!crate::system::resolver::resolver_network_prefix(&prefix()).contains(&addr));
            assert_eq!(addr.octets()[6], ResourceKind::Service as u8);
        }
    }

    // spec: INFRA#service-addresses
    #[test]
    fn an_address_goes_with_its_instance() {
        let db = Db::open_in_memory().unwrap();
        let (a, b) = colliding();
        let (a, b) = (service("a", a), service("b", b));
        let first = address_of(&db, &prefix(), &a).unwrap();
        history::delete_instance(&db, a.id).unwrap();
        assert_eq!(address_of(&db, &prefix(), &b).unwrap(), first);
    }
}
