use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv6Addr, SocketAddr},
};

use ipnet::Ipv6Net;

use crate::{
    defs::ingress::IngressDef,
    runtime::identity::ResourceInstance,
    system::types::{
        HttpRedirect, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute, VirtualHost,
    },
};

/// A resolved upstream for one service: the service's stable IPv6 address
/// and the internal port Caddy should send traffic to.
pub struct ServiceUpstream {
    pub service_ip: Ipv6Addr,
    pub service_port: u16,
}

/// Derives the IPv6 address for a resource instance using the ULA encoding.
///
/// Address layout: `fd5e:edXX:XXXX:KKUU:UUUU:UUUU:UUUU:UUUU/128`
/// - Bytes  0–5 : node_prefix /48
/// - Byte   6   : `ResourceKind` discriminant (`KK`)
/// - Byte   7   : `uuid[0]` (`UU`)
/// - Bytes  8–15: `uuid[1..9]`
pub fn instance_ipv6(node_prefix: &Ipv6Net, instance: &ResourceInstance) -> Ipv6Addr {
    debug_assert_eq!(node_prefix.prefix_len(), 48, "node prefix must be /48");

    let prefix_bytes = node_prefix.network().octets();
    let uuid_bytes = instance.id.0.as_bytes();
    let kind_byte = instance.kind as u8;

    let mut addr = [0u8; 16];
    addr[..6].copy_from_slice(&prefix_bytes[..6]);
    addr[6] = kind_byte;
    addr[7] = uuid_bytes[0];
    addr[8..16].copy_from_slice(&uuid_bytes[1..9]);

    Ipv6Addr::from(addr)
}

/// Derives the pod network /64 prefix for a pod instance.
///
/// Prefix layout: `fd5e:edXX:XXXX:KKUU::/64` — identical to `instance_ipv6`
/// but with the interface ID (bytes 8–15) zeroed.
pub fn pod_network_prefix(node_prefix: &Ipv6Net, instance: &ResourceInstance) -> Ipv6Net {
    let addr = instance_ipv6(node_prefix, instance);
    let mut bytes = addr.octets();
    bytes[8..].fill(0);
    Ipv6Net::new(Ipv6Addr::from(bytes), 64).expect("64 is a valid IPv6 prefix length")
}

/// Returns the mount endpoint address for a pod's /64 prefix: `prefix::1000`.
///
/// This address is assigned to the host bridge and is the DNAT6 destination
/// that containers use to reach mounted services via the `localmount` hostname.
///
/// `::2` is intentionally avoided: netavark sequentially assigns that address
/// to the first container on the network, which would produce a host-local
/// address collision and cause route additions to fail with EINVAL.
pub fn pod_mount_addr(pod_prefix: &Ipv6Net) -> Ipv6Addr {
    let mut bytes = pod_prefix.network().octets();
    bytes[8..].fill(0);
    bytes[14] = 0x10;
    // bytes[15] is already 0 from fill above; result is prefix::1000
    Ipv6Addr::from(bytes)
}

/// Builds the full `ProxyConfig` from the current set of active ingresses
/// and their resolved service upstreams.
///
/// The Ingress → Service → running-pod-instance resolution is performed by
/// the caller; this function receives already-resolved data.
///
/// `_caddy_addr` is the Caddy admin API address, reserved for callers that
/// also need to build `DataPlaneRules` pointing to the same container.
pub fn build_proxy_config(
    ingresses: &[(IngressDef, ServiceUpstream)],
    _caddy_addr: SocketAddr,
) -> ProxyConfig {
    let mut listener_set: BTreeSet<ProxyListener> = BTreeSet::new();
    let mut vhosts: BTreeMap<String, VirtualHost> = BTreeMap::new();

    for (ingress, upstream) in ingresses {
        let is_https = ingress.http_terminate.is_some();

        // Main port listener.
        listener_set.insert(ProxyListener {
            port: ingress.port,
            proto: if is_https {
                ProxyListenerProto::Https
            } else {
                ProxyListenerProto::Http
            },
        });

        // HTTP/3 QUIC listener on the same port.
        if ingress.quic {
            listener_set.insert(ProxyListener {
                port: ingress.port,
                proto: ProxyListenerProto::Quic,
            });
        }

        // Plain-HTTP listener for the redirect source port.
        if let Some(redirect) = &ingress.redirect {
            listener_set.insert(ProxyListener {
                port: redirect.port,
                proto: ProxyListenerProto::Http,
            });
        }

        // Caddy always contacts the service over plain HTTP; TLS is
        // terminated by Caddy, not by the backing service.
        let upstream_url = format!("http://[{}]:{}", upstream.service_ip, upstream.service_port);

        let vhost = vhosts
            .entry(ingress.hostname.clone())
            .or_insert_with(|| VirtualHost {
                hostname: ingress.hostname.clone(),
                tls_acme: false,
                redirect: None,
                routes: vec![],
            });

        if ingress.tls {
            vhost.tls_acme = true;
        }

        if let Some(redirect) = &ingress.redirect {
            vhost.redirect = Some(HttpRedirect {
                from_port: redirect.port,
                code: redirect.code,
            });
        }

        vhost.routes.push(ProxyRoute {
            prefix: "/".to_string(),
            upstreams: vec![upstream_url],
        });
    }

    ProxyConfig {
        listeners: listener_set.into_iter().collect(),
        virtual_hosts: vhosts.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{defs::resource::ResourceKind, runtime::identity::InstanceId};

    fn test_prefix() -> Ipv6Net {
        "fd5e:ed12:3456::/48".parse().unwrap()
    }

    fn make_instance(kind: ResourceKind) -> ResourceInstance {
        ResourceInstance {
            id: InstanceId(uuid::Uuid::from_bytes([
                0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
                0x99, 0x00,
            ])),
            app: "test".into(),
            kind,
            name: Some("foo".into()),
            variant: crate::runtime::identity::InstanceVariant::Singleton,
            display_name: "test-foo".into(),
        }
    }

    #[test]
    fn instance_ipv6_embeds_node_prefix() {
        let prefix = test_prefix();
        let instance = make_instance(ResourceKind::Deployment);
        let addr = instance_ipv6(&prefix, &instance);
        let octets = addr.octets();
        // First 6 bytes must match the /48 prefix fd5e:ed12:3456
        assert_eq!(&octets[..6], &[0xfd, 0x5e, 0xed, 0x12, 0x34, 0x56]);
    }

    #[test]
    fn instance_ipv6_encodes_kind_byte() {
        let prefix = test_prefix();
        let dep = make_instance(ResourceKind::Deployment);
        let svc = make_instance(ResourceKind::Service);
        let dep_addr = instance_ipv6(&prefix, &dep);
        let svc_addr = instance_ipv6(&prefix, &svc);
        // Same UUID but different kinds → different addresses
        assert_ne!(dep_addr, svc_addr);
        // Kind byte is at index 6
        assert_eq!(dep_addr.octets()[6], ResourceKind::Deployment as u8);
        assert_eq!(svc_addr.octets()[6], ResourceKind::Service as u8);
    }

    #[test]
    fn pod_network_prefix_is_64() {
        let prefix = test_prefix();
        let instance = make_instance(ResourceKind::Deployment);
        let net = pod_network_prefix(&prefix, &instance);
        assert_eq!(net.prefix_len(), 64);
        // Interface ID portion must be zeroed
        let octets = net.network().octets();
        assert_eq!(&octets[8..], &[0u8; 8]);
    }

    #[test]
    fn pod_prefix_matches_instance_address_upper_64() {
        let prefix = test_prefix();
        let instance = make_instance(ResourceKind::Job);
        let addr = instance_ipv6(&prefix, &instance);
        let net = pod_network_prefix(&prefix, &instance);
        // The /64 network address must match the first 8 bytes of the instance address
        assert_eq!(&addr.octets()[..8], &net.network().octets()[..8]);
    }
}
