//! Observational probes for host network capabilities.
//!
//! These functions inspect the kernel's routing and address tables; they
//! do not send packets and do not depend on DNS. Errors are treated as
//! "not available" rather than propagated, so they are safe to run at
//! daemon startup without gating the rest of initialisation on them.

use std::net::IpAddr;

use futures_util::TryStreamExt;
use rtnetlink::packet_route::{
    address::AddressAttribute,
    route::{RouteAddress, RouteAttribute, RouteType},
};

/// Probes whether the host has working IPv4 egress.
///
/// The check is the presence of a default IPv4 unicast route (`0.0.0.0/0`)
/// in the main routing table. IPv4 source address selection is not
/// inspected: RFC1918 hosts behind NAT still have egress, and a host that
/// has configured a default gateway has implicitly declared egress intent.
///
/// Returns `false` on any rtnetlink error.
pub async fn detect_ipv4_egress() -> bool {
    let (connection, handle, _) = match rtnetlink::new_connection() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "IPv4 egress probe: failed to open rtnetlink; assuming no egress"
            );
            return false;
        }
    };
    tokio::spawn(connection);

    match has_default_v4_route(&handle).await {
        Ok(v) => {
            tracing::info!(egress = v, "IPv4 egress probe result");
            v
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "IPv4 egress probe: failed to enumerate routes; assuming no egress"
            );
            false
        }
    }
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
/// Returns `false` on any rtnetlink error; an unreachable kernel
/// interface is treated as the more conservative "no egress" case.
// r[impl infra.nat64.ipv6-egress]
pub async fn detect_ipv6_egress() -> bool {
    let (connection, handle, _) = match rtnetlink::new_connection() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "IPv6 egress probe: failed to open rtnetlink; assuming no egress"
            );
            return false;
        }
    };
    tokio::spawn(connection);

    let has_route = match has_default_v6_route(&handle).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "IPv6 egress probe: failed to enumerate routes; assuming no egress"
            );
            return false;
        }
    };
    let has_gua = match has_global_v6_source(&handle).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "IPv6 egress probe: failed to enumerate addresses; assuming no egress"
            );
            return false;
        }
    };

    let egress = has_route && has_gua;
    tracing::info!(
        default_route = has_route,
        global_source = has_gua,
        egress,
        "IPv6 egress probe result"
    );
    egress
}

async fn has_default_v4_route(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let query = rtnetlink::RouteMessageBuilder::<std::net::Ipv4Addr>::new().build();
    let mut stream = handle.route().get(query).execute();
    while let Some(route) = stream.try_next().await? {
        if route.header.destination_prefix_length != 0 {
            continue;
        }
        if !matches!(route.header.kind, RouteType::Unicast) {
            continue;
        }
        let dest_is_default = !route
            .attributes
            .iter()
            .any(|a| matches!(a, RouteAttribute::Destination(_)))
            || route.attributes.iter().any(|a| {
                matches!(
                    a,
                    RouteAttribute::Destination(RouteAddress::Inet(addr))
                        if addr.is_unspecified()
                )
            });
        if dest_is_default {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn has_default_v6_route(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let query = rtnetlink::RouteMessageBuilder::<std::net::Ipv6Addr>::new().build();
    let mut stream = handle.route().get(query).execute();
    while let Some(route) = stream.try_next().await? {
        if route.header.destination_prefix_length != 0 {
            continue;
        }
        if !matches!(route.header.kind, RouteType::Unicast) {
            continue;
        }
        // A true default route either omits the Destination attribute or
        // sets it to `::`. Either way, a /0 unicast route in the main
        // routing table means the kernel has somewhere to send traffic.
        let dest_is_default = !route
            .attributes
            .iter()
            .any(|a| matches!(a, RouteAttribute::Destination(_)))
            || route.attributes.iter().any(|a| {
                matches!(
                    a,
                    RouteAttribute::Destination(RouteAddress::Inet6(addr))
                        if addr.is_unspecified()
                )
            });
        if dest_is_default {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn has_global_v6_source(handle: &rtnetlink::Handle) -> Result<bool, rtnetlink::Error> {
    let mut stream = handle.address().get().execute();
    while let Some(msg) = stream.try_next().await? {
        for attr in &msg.attributes {
            if let AddressAttribute::Address(IpAddr::V6(addr)) = attr
                && (addr.segments()[0] & 0xe000) == 0x2000
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
