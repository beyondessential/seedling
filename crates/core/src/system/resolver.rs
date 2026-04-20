use std::net::Ipv6Addr;

use ipnet::{Ipv4Net, Ipv6Net};

pub use forwarder::spawn_dns_forwarder;
pub use startup::{
    ResolverAddrs, ResolverStartupError, ensure_resolver_running, teardown_resolver,
};

mod config;
mod forwarder;
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
// r[impl infra.resolver.address]
pub fn resolver_addr(node_prefix: &Ipv6Net) -> Ipv6Addr {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xfd;
    addr[15] = 53;
    Ipv6Addr::from(addr)
}

/// Returns the host-side bridge gateway IPv6 address of the resolver
/// network (`<prefix>:fd00::1`). Netavark assigns this address when the
/// resolver bridge is created; it is the address containers reach when
/// sending to their default gateway, and — on the host side — the
/// address the in-process DNS forwarder binds to.
pub fn resolver_gateway_addr(node_prefix: &Ipv6Net) -> Ipv6Addr {
    let bytes = node_prefix.network().octets();
    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&bytes[..6]);
    addr[6] = 0xfd;
    addr[15] = 1;
    Ipv6Addr::from(addr)
}

/// Fixed IPv4 subnet for the resolver network.
///
/// The resolver network is dual-stack so that CoreDNS can forward queries
/// to upstream IPv4 DNS servers.
pub fn resolver_ipv4_subnet() -> Ipv4Net {
    "10.89.254.0/24".parse().expect("valid IPv4 subnet")
}
