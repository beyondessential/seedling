use std::net::Ipv6Addr;

use ipnet::Ipv6Net;

pub use startup::{
    ResolverAddrs, ResolverStartupError, ensure_resolver_running, teardown_resolver,
};

mod config;
mod startup;

/// Derives the resolver network /64 prefix from the node prefix.
///
/// Uses subnet discriminant `0xfd`, distinct from the proxy (`0xff`)
/// and mount endpoint (`0xfe`) subnets.
pub fn resolver_network_prefix(node_prefix: &Ipv6Net) -> Ipv6Net {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xfd;
    Ipv6Net::new(Ipv6Addr::from(addr), 64).expect("64 is a valid IPv6 prefix length")
}

/// Returns the static IPv6 address assigned to the resolver container.
///
/// This is a well-known address at `::53` within the resolver network prefix,
/// chosen to match the DNS port for memorability.
pub fn resolver_addr(node_prefix: &Ipv6Net) -> Ipv6Addr {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xfd;
    addr[15] = 53;
    Ipv6Addr::from(addr)
}
