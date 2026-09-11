use std::net::Ipv6Addr;

use futures_util::TryStreamExt;
use rtnetlink::{
    Handle, RouteMessageBuilder,
    packet_route::route::{RouteAttribute, RouteProtocol, RouteType},
};

use super::{DataPlaneError, NftablesDataPlane};
use crate::system::types::ServiceRoute;

/// Format an rtnetlink error into a descriptive message similar to iproute2 output.
fn format_rtnetlink_error(err: &rtnetlink::Error, context: &str) -> String {
    match err {
        rtnetlink::Error::NetlinkError(msg) => {
            // `to_io()` gives us the standard OS error description (e.g.
            // "File exists", "No such file or directory") from the kernel
            // errno, which mirrors what iproute2 would print.
            let io_err = msg.to_io();
            let errno = msg.code.map(|c| c.get().unsigned_abs());
            let hint = match errno {
                Some(e) if e == libc::EEXIST as u32 => " (route already exists)",
                Some(e) if e == libc::EPERM as u32 => " (missing CAP_NET_ADMIN?)",
                Some(e) if e == libc::ESRCH as u32 => " (stale nexthop?)",
                _ => "",
            };
            format!("{context}: {io_err}{hint}")
        }
        other => format!("{context}: {other}"),
    }
}

/// Extract the IPv6 destination address and prefix length from a route message.
fn route_destination(msg: &rtnetlink::packet_route::route::RouteMessage) -> Option<(Ipv6Addr, u8)> {
    for attr in &msg.attributes {
        if let RouteAttribute::Destination(rtnetlink::packet_route::route::RouteAddress::Inet6(
            addr,
        )) = attr
        {
            return Some((*addr, msg.header.destination_prefix_length));
        }
    }
    None
}

impl NftablesDataPlane {
    /// Delete all seedling-managed IPv6 static /128 routes in the fd5e::/16
    /// range using rtnetlink.
    #[tracing::instrument(level = "trace", skip(self))]
    pub(super) async fn delete_managed_routes(
        &self,
        handle: &Handle,
    ) -> Result<(), DataPlaneError> {
        let query = RouteMessageBuilder::<Ipv6Addr>::new()
            .protocol(RouteProtocol::Static)
            .build();

        let mut stream = handle.route().get(query).execute();

        // Collect matching routes first so we don't hold the stream across deletes.
        let mut to_delete = Vec::new();
        while let Some(route) = stream
            .try_next()
            .await
            .map_err(|e| DataPlaneError::Netlink {
                source: format_rtnetlink_error(&e, "listing IPv6 static routes").into(),
                backtrace: std::backtrace::Backtrace::capture(),
            })?
        {
            // Filter: must be protocol static, table main, /128, in fd5e::/16.
            if route.header.protocol != RouteProtocol::Static {
                continue;
            }

            let table = effective_table(&route);
            if table != MAIN_ROUTE_TABLE {
                continue;
            }

            if let Some((addr, prefix_len)) = route_destination(&route) {
                if prefix_len != 128 {
                    continue;
                }
                let octets = addr.octets();
                if octets[0] != 0xfd || octets[1] != 0x5e {
                    continue;
                }
                to_delete.push(route);
            }
        }

        for route in to_delete {
            let dst_desc = route_destination(&route)
                .map(|(a, l)| format!("{a}/{l}"))
                .unwrap_or_else(|| "unknown".to_owned());

            handle
                .route()
                .del(route)
                .execute()
                .await
                .map_err(|e| DataPlaneError::Netlink {
                    source: format_rtnetlink_error(&e, &format!("deleting route {dst_desc}"))
                        .into(),
                    backtrace: std::backtrace::Backtrace::capture(),
                })?;
        }

        Ok(())
    }

    /// Add or replace a service route using rtnetlink.
    #[tracing::instrument(level = "trace", skip(self))]
    pub(super) async fn add_service_route(
        &self,
        handle: &Handle,
        svc: &ServiceRoute,
    ) -> Result<(), DataPlaneError> {
        let dst_desc = format!("{}/128", svc.service_ip);

        match svc.backends.len() {
            0 => {
                let route = RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .protocol(RouteProtocol::Static)
                    .kind(RouteType::BlackHole)
                    .table_id(254)
                    .build();

                handle
                    .route()
                    .add(route)
                    .replace()
                    .execute()
                    .await
                    .map_err(|e| DataPlaneError::Netlink {
                        source: format_rtnetlink_error(
                            &e,
                            &format!("replacing blackhole route {dst_desc}"),
                        )
                        .into(),
                        backtrace: std::backtrace::Backtrace::capture(),
                    })?;
            }
            1 => {
                let route = RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .gateway(svc.backends[0])
                    .protocol(RouteProtocol::Static)
                    .table_id(254)
                    .build();

                handle
                    .route()
                    .add(route)
                    .replace()
                    .execute()
                    .await
                    .map_err(|e| DataPlaneError::Netlink {
                        source: format_rtnetlink_error(
                            &e,
                            &format!("replacing route {dst_desc} via {}", svc.backends[0]),
                        )
                        .into(),
                        backtrace: std::backtrace::Backtrace::capture(),
                    })?;
            }
            _ => {
                let nexthops: Vec<_> = svc
                    .backends
                    .iter()
                    .map(|b| {
                        rtnetlink::RouteNextHopBuilder::new_ipv6()
                            .via(std::net::IpAddr::V6(*b))
                            .expect("IPv6 address for IPv6 nexthop builder")
                            .build()
                    })
                    .collect();

                let route = RouteMessageBuilder::<Ipv6Addr>::new()
                    .destination_prefix(svc.service_ip, 128)
                    .protocol(RouteProtocol::Static)
                    .table_id(254)
                    .multipath(nexthops)
                    .build();

                handle
                    .route()
                    .add(route)
                    .replace()
                    .execute()
                    .await
                    .map_err(|e| {
                        let backends: Vec<_> =
                            svc.backends.iter().map(ToString::to_string).collect();
                        DataPlaneError::Netlink {
                            source: format_rtnetlink_error(
                                &e,
                                &format!(
                                    "replacing multipath route {dst_desc} nexthops [{}]",
                                    backends.join(", ")
                                ),
                            )
                            .into(),
                            backtrace: std::backtrace::Backtrace::capture(),
                        }
                    })?;
            }
        }

        Ok(())
    }
}

/// Get the effective routing table ID from a route message.
///
/// The kernel stores table IDs in two places: a u8 field in the header (max
/// 255) and an optional `RTA_TABLE` attribute for larger values. The attribute
/// takes precedence when present.
/// The `main` routing table. A default route confined to a policy-routing
/// table is not the system's egress path, so anything asking "can this host
/// reach the internet" has to look here specifically.
pub(crate) const MAIN_ROUTE_TABLE: u32 = 254;

pub(crate) fn effective_table(msg: &rtnetlink::packet_route::route::RouteMessage) -> u32 {
    for attr in &msg.attributes {
        if let RouteAttribute::Table(t) = attr {
            return *t;
        }
    }
    msg.header.table as u32
}

#[cfg(test)]
mod tests {
    use rtnetlink::packet_route::route::{RouteAttribute, RouteHeader, RouteMessage};

    use super::{MAIN_ROUTE_TABLE, effective_table};

    fn msg(header_table: u8, attr_table: Option<u32>) -> RouteMessage {
        let mut m = RouteMessage::default();
        m.header = RouteHeader {
            table: header_table,
            ..Default::default()
        };
        if let Some(t) = attr_table {
            m.attributes.push(RouteAttribute::Table(t));
        }
        m
    }

    // r[verify infra.nat64.ipv6-egress]
    // The kernel keeps the table id in a u8 header field and, for ids that do
    // not fit, an RTA_TABLE attribute that takes precedence. Reading only the
    // header would put a large-id route in whichever table the u8 happened to
    // truncate to.
    #[test]
    fn the_table_attribute_wins_over_the_header_field() {
        assert_eq!(effective_table(&msg(254, None)), MAIN_ROUTE_TABLE);
        assert_eq!(effective_table(&msg(0, Some(1000))), 1000);
        assert_eq!(effective_table(&msg(254, Some(1000))), 1000);
    }

    // r[verify infra.nat64.ipv6-egress]
    #[test]
    fn a_policy_table_is_not_the_main_table() {
        assert_ne!(effective_table(&msg(0, Some(100))), MAIN_ROUTE_TABLE);
    }
}
