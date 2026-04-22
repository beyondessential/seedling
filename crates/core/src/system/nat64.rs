use std::{fmt, net::IpAddr};

use clap::ValueEnum;
use futures_util::TryStreamExt;
use rtnetlink::packet_route::{
    address::AddressAttribute,
    route::{RouteAddress, RouteAttribute, RouteType},
};

/// Controls whether the runtime provides its own NAT64 translator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Nat64Mode {
    /// Probe for existing NAT64 infrastructure on startup; enable if absent.
    #[default]
    Auto,
    /// Always provide NAT64.
    Enabled,
    /// Never provide NAT64.
    Disabled,
}

impl fmt::Display for Nat64Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Enabled => f.write_str("enabled"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

/// Probes for existing NAT64+DNS64 infrastructure using RFC 7050.
///
/// Returns `true` if the network already provides NAT64 (and seedling should
/// not activate its own).
pub async fn detect_external_nat64() -> bool {
    match tokio::net::lookup_host("ipv4only.arpa:0").await {
        Ok(addrs) => {
            for addr in addrs {
                if addr.is_ipv6() {
                    tracing::info!(
                        addr = %addr.ip(),
                        "detected existing NAT64+DNS64 infrastructure via RFC 7050"
                    );
                    return true;
                }
            }
            tracing::info!("no NAT64+DNS64 detected (ipv4only.arpa returned no AAAA records)");
            false
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "NAT64 detection failed (could not resolve ipv4only.arpa); assuming no NAT64"
            );
            false
        }
    }
}

/// Determines whether seedling should activate its own NAT64 translator.
pub async fn should_activate_nat64(mode: Nat64Mode) -> bool {
    match mode {
        Nat64Mode::Enabled => true,
        Nat64Mode::Disabled => false,
        Nat64Mode::Auto => !detect_external_nat64().await,
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
