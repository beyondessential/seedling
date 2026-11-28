//! The host's IPv4 addresses as a pod reaches them through NAT64: in
//! `64:ff9b::` form, kept in the translation table's `host_v4_nat64` set so
//! that pod traffic to an ingress port on one of them is redirected to the
//! proxy before NAT64 translates it.

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr},
};

use futures_util::TryStreamExt;
use ipnet::{Ipv4Net, Ipv6Net};
use rtnetlink::{
    Handle,
    packet_route::{
        AddressFamily,
        address::{AddressAttribute, AddressMessage},
    },
};
use snafu::Snafu;

use seedling_core::{
    runtime::nat64_prefix::{NAT64_PREFIX_NET, synth_v4},
    system::{proxy_ipv4_subnet, resolver::resolver_ipv4_subnet},
};

#[derive(Debug, Snafu)]
pub enum ReadError {
    #[snafu(display("reading the host's IPv4 addresses failed: {source}"))]
    Dump { source: rtnetlink::Error },
    #[snafu(display(
        "the kernel reported an IPv4 address on interface {index} without the address"
    ))]
    Unaddressed { index: u32 },
}

/// The IPv4 addresses the host holds, each with the interface holding it, so
/// that an address held on two interfaces stays until both let it go.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct HostAddrs(BTreeSet<(u32, Ipv4Addr)>);

impl HostAddrs {
    /// The `host_v4_nat64` set's elements for these addresses.
    pub(super) fn elements(&self) -> Vec<Ipv6Net> {
        set_elements(self.0.iter().map(|(_, addr)| *addr))
    }
}

/// The IPv4 subnets of the proxy and resolver networks, which are in the set
/// whatever addresses the host holds.
// spec: INFRA#pod-traffic-to-the-host
fn infrastructure_subnets() -> [Ipv4Net; 2] {
    [proxy_ipv4_subnet(), resolver_ipv4_subnet()]
}

/// `net` in the NAT64 prefix.
fn nat64_net(net: Ipv4Net) -> Ipv6Net {
    Ipv6Net::new(
        synth_v4(net.network()),
        NAT64_PREFIX_NET.prefix_len() + net.prefix_len(),
    )
    .expect("an IPv4 prefix within a /96 is a valid IPv6 prefix")
}

/// The single element standing for `addr`, or `None` where an
/// infrastructure subnet already covers it. The set holds intervals, which
/// must not overlap.
fn address_element(addr: Ipv4Addr) -> Option<Ipv6Net> {
    if infrastructure_subnets()
        .iter()
        .any(|net| net.contains(&addr))
    {
        return None;
    }
    Some(nat64_net(Ipv4Net::from(addr)))
}

/// The `host_v4_nat64` set's elements: the proxy and resolver subnets, and
/// each of `addrs` outside them, once, in NAT64 form.
// spec: INFRA#pod-traffic-to-the-host
pub(super) fn set_elements(addrs: impl IntoIterator<Item = Ipv4Addr>) -> Vec<Ipv6Net> {
    let mut elements: BTreeSet<Ipv6Net> = infrastructure_subnets()
        .into_iter()
        .map(nat64_net)
        .collect();
    elements.extend(addrs.into_iter().filter_map(address_element));
    elements.into_iter().collect()
}

/// The IPv4 address `msg` reports and the interface holding it, or `None` for
/// an address of another family.
pub(super) fn held_v4(msg: &AddressMessage) -> Result<Option<(u32, Ipv4Addr)>, ReadError> {
    if msg.header.family != AddressFamily::Inet {
        return Ok(None);
    }
    let index = msg.header.index;
    // On a point-to-point link the address attribute is the peer's, and the
    // local attribute the host's own; elsewhere both are the host's.
    let local = msg.attributes.iter().find_map(|attr| match attr {
        AddressAttribute::Local(IpAddr::V4(addr)) => Some(*addr),
        _ => None,
    });
    let address = msg.attributes.iter().find_map(|attr| match attr {
        AddressAttribute::Address(IpAddr::V4(addr)) => Some(*addr),
        _ => None,
    });
    match local.or(address) {
        Some(addr) => Ok(Some((index, addr))),
        None => UnaddressedSnafu { index }.fail(),
    }
}

/// Every IPv4 address the host holds now.
pub(super) async fn read(handle: &Handle) -> Result<HostAddrs, ReadError> {
    let mut request = handle.address().get();
    request.message_mut().header.family = AddressFamily::Inet;
    let mut dump = request.execute();
    let mut held = BTreeSet::new();
    while let Some(msg) = dump
        .try_next()
        .await
        .map_err(|source| ReadError::Dump { source })?
    {
        held.extend(held_v4(&msg)?);
    }
    Ok(HostAddrs(held))
}
