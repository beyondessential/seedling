//! Observational probes for host network capabilities.
//!
//! These functions inspect the kernel's routing and address tables; they
//! do not send packets and do not depend on DNS. A probe that cannot be made
//! returns an error, not "not available": it is undetermined, and the caller
//! decides what that stands for.

use std::net::IpAddr;

use futures_util::TryStreamExt;
use rtnetlink::packet_route::{
    address::AddressAttribute,
    route::{RouteAddress, RouteAttribute, RouteMessage, RouteType},
};

/// Probes whether the host has working IPv4 egress.
///
/// The check is the presence of a default IPv4 unicast route (`0.0.0.0/0`)
/// in the main routing table. IPv4 source address selection is not
/// inspected: RFC1918 hosts behind NAT still have egress, and a host that
/// has configured a default gateway has implicitly declared egress intent.
///
/// `Err` when the routes cannot be read: that is undetermined, not "no egress".
// spec: CONST#determining-host-constants
pub async fn detect_ipv4_egress() -> Result<bool, String> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| format!("IPv4 egress probe: failed to open rtnetlink: {e}"))?;
    tokio::spawn(connection);

    let egress = has_default_v4_route(&handle)
        .await
        .map_err(|e| format!("IPv4 egress probe: failed to enumerate routes: {e}"))?;
    tracing::info!(egress, "IPv4 egress probe result");
    Ok(egress)
}

/// Probes whether the host has working IPv6 egress to the internet.
///
/// Two conditions must both hold: the main routing table contains a
/// default IPv6 unicast route (`::/0`), and at least one non-loopback
/// interface carries an address in the global unicast range
/// `2000::/3`. ULAs (`fc00::/7`) and link-locals (`fe80::/10`) do not
/// qualify — an address in those ranges cannot source traffic that
/// reaches the wider internet.
///
/// The check is observational only: no DNS, no outbound packets.
///
/// `Err` when the route or address state cannot be read: that is undetermined,
/// not "no egress".
// spec: NAT64#ipv6-egress
pub async fn detect_ipv6_egress() -> Result<bool, String> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| format!("IPv6 egress probe: failed to open rtnetlink: {e}"))?;
    tokio::spawn(connection);

    let has_route = has_default_v6_route(&handle)
        .await
        .map_err(|e| format!("IPv6 egress probe: failed to enumerate routes: {e}"))?;
    let has_gua = has_global_v6_source(&handle)
        .await
        .map_err(|e| format!("IPv6 egress probe: failed to enumerate addresses: {e}"))?;

    let egress = has_route && has_gua;
    tracing::info!(
        default_route = has_route,
        global_source = has_gua,
        egress,
        "IPv6 egress probe result"
    );
    Ok(egress)
}

/// Whether `route` is a default unicast route (`/0`) of the main routing
/// table. The dump returns every table, and a default route living in a
/// policy-routing table is not this host's egress path.
// spec: LINUX#egress
fn is_main_table_default_route(route: &RouteMessage) -> bool {
    if route.header.destination_prefix_length != 0 {
        return false;
    }
    if !matches!(route.header.kind, RouteType::Unicast) {
        return false;
    }
    if crate::system::data_plane::routes::effective_table(route)
        != crate::system::data_plane::routes::MAIN_ROUTE_TABLE
    {
        return false;
    }
    // A true default route either omits the Destination attribute or sets it
    // to the unspecified address.
    let mut destinations = route.attributes.iter().filter_map(|a| match a {
        RouteAttribute::Destination(RouteAddress::Inet(addr)) => Some(addr.is_unspecified()),
        RouteAttribute::Destination(RouteAddress::Inet6(addr)) => Some(addr.is_unspecified()),
        _ => None,
    });
    destinations.all(|unspecified| unspecified)
}

async fn has_default_v4_route(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let query = rtnetlink::RouteMessageBuilder::<std::net::Ipv4Addr>::new().build();
    let mut stream = handle.route().get(query).execute();
    while let Some(route) = stream.try_next().await? {
        if is_main_table_default_route(&route) {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn has_default_v6_route(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let query = rtnetlink::RouteMessageBuilder::<std::net::Ipv6Addr>::new().build();
    let mut stream = handle.route().get(query).execute();
    while let Some(route) = stream.try_next().await? {
        if is_main_table_default_route(&route) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `addr` is a globally routable IPv6 address: one within `2000::/3`.
// spec: LINUX#egress
fn is_global_v6(addr: &std::net::Ipv6Addr) -> bool {
    (addr.segments()[0] & 0xe000) == 0x2000
}

async fn has_global_v6_source(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let mut stream = handle.address().get().execute();
    while let Some(msg) = stream.try_next().await? {
        for attr in &msg.attributes {
            if let AddressAttribute::Address(IpAddr::V6(addr)) = attr
                && is_global_v6(addr)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use rtnetlink::packet_route::route::RouteHeader;

    use super::*;

    fn default_route(table: u32, destination: Option<IpAddr>) -> RouteMessage {
        let mut m = RouteMessage::default();
        m.header = RouteHeader {
            kind: RouteType::Unicast,
            ..Default::default()
        };
        m.attributes.push(RouteAttribute::Table(table));
        match destination {
            Some(IpAddr::V4(a)) => m
                .attributes
                .push(RouteAttribute::Destination(RouteAddress::Inet(a))),
            Some(IpAddr::V6(a)) => m
                .attributes
                .push(RouteAttribute::Destination(RouteAddress::Inet6(a))),
            None => {}
        }
        m
    }

    // spec: LINUX#egress
    #[test]
    fn only_a_default_route_of_the_main_table_counts_for_either_family() {
        use crate::system::data_plane::routes::MAIN_ROUTE_TABLE;
        let v6 = Some(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED));
        let v4 = Some(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        assert!(is_main_table_default_route(&default_route(
            MAIN_ROUTE_TABLE,
            v6
        )));
        assert!(is_main_table_default_route(&default_route(
            MAIN_ROUTE_TABLE,
            v4
        )));
        assert!(is_main_table_default_route(&default_route(
            MAIN_ROUTE_TABLE,
            None
        )));
        assert!(!is_main_table_default_route(&default_route(100, v6)));
        assert!(!is_main_table_default_route(&default_route(100, v4)));
    }

    // spec: LINUX#egress
    #[test]
    fn a_globally_routable_address_is_one_within_2000_slash_3() {
        assert!(is_global_v6(&"2001:db8::1".parse().unwrap()));
        assert!(is_global_v6(&"3fff::1".parse().unwrap()));
        assert!(!is_global_v6(&"fd00::1".parse().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_v6(&"4000::1".parse().unwrap()));
        assert!(!is_global_v6(&"1fff::1".parse().unwrap()));
    }
}
