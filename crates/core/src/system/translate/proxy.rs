use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv6Addr,
};

use ipnet::Ipv6Net;

use crate::{
    defs::ingress::IngressDef,
    runtime::identity::ResourceInstance,
    system::types::{
        HttpRedirect, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute,
        ProxyRouteHandler, VirtualHost,
    },
};

/// A resolved upstream for one service.
///
/// `routes` lists the per-prefix HTTP routes declared by the BSL (each
/// `deployment.http(pod_port, svc.route(prefix))` binding), with upstreams
/// resolved to backend pod addresses so Caddy can do longest-prefix matching
/// and pick the right pod directly. Bypassing the service IP for HTTP is
/// the only way prefix routing can work — the service-IP path goes through
/// nftables ECMP DNAT which doesn't know about URL prefixes.
///
/// `service_ip` / `service_port` remain as a fallback for ingresses whose
/// backing service has no `http_bindings` at all (e.g. an HTTPS ingress
/// fronting a TCP-only service): in that case `routes` is empty and the
/// legacy single-`/` route through the service IP is emitted.
pub struct ServiceUpstream {
    pub routes: Vec<HttpForwardRoute>,
    pub service_ip: Ipv6Addr,
    pub service_port: u16,
}

/// One HTTP route on a service: the URL prefix declared in BSL, plus the
/// concrete pod-IP upstreams backing that prefix on the current tick.
// r[impl service.http.route.routing]
#[derive(Debug, Clone)]
pub struct HttpForwardRoute {
    pub prefix: String,
    /// `ip:port` upstreams, one per backing pod observed running this tick.
    pub upstreams: Vec<String>,
}

/// Resolved redirect target for a site-ingress attachment. Used in place of
/// a `ServiceUpstream` when the attachment answers requests with an HTTP
/// redirect instead of forwarding them to a backing service.
pub struct RedirectTarget {
    pub url: String,
    pub code: u16,
    pub preserve_path: bool,
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

/// Returns the node-wide mount endpoint address: `prefix[0..6]:fffe::1`.
///
/// Bytes 0–5 come from the first six octets of `prefix` (the node /48 part,
/// shared by all pod /64 prefixes derived from the same node).  Bytes 6–7
/// are fixed at `0xff, 0xfe` (the `fffe` infrastructure discriminant, above
/// the resource-kind range 0–9 and the proxy discriminant `0xff`).
/// Bytes 8–15 are zero except the last, which is `0x01`.
///
/// The address does not need to be assigned to any interface.  Containers
/// route it via the pod bridge gateway; nftables prerouting DNAT intercepts
/// it before any routing decision.
// r[impl infra.pod.mount]
pub fn node_mount_addr(prefix: &Ipv6Net) -> Ipv6Addr {
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&prefix.network().octets()[..6]);
    bytes[6] = 0xff;
    bytes[7] = 0xfe;
    bytes[15] = 0x01;
    Ipv6Addr::from(bytes)
}

/// Builds the full `ProxyConfig` from the current set of active ingresses
/// and their resolved service upstreams or redirect targets.
///
/// The Ingress → Service → running-pod-instance resolution is performed by
/// the caller; this function receives already-resolved data.
pub fn build_proxy_config(
    forwards: &[(IngressDef, ServiceUpstream)],
    redirects: &[(IngressDef, RedirectTarget)],
) -> ProxyConfig {
    let mut listener_set: BTreeSet<ProxyListener> = BTreeSet::new();
    let mut vhosts: BTreeMap<(String, bool), VirtualHost> = BTreeMap::new();

    for (ingress, upstream) in forwards {
        register_listeners(&mut listener_set, ingress);
        let vhost = ensure_vhost(&mut vhosts, ingress);
        // r[impl service.http.route.routing]
        // Prefer per-prefix routes when the BSL declared any: walk each route
        // declared by the service's http_bindings (longest prefix wins inside
        // Caddy) and emit a ProxyRoute pointing at the matching pod IPs. Fall
        // back to a single "/" route through the service IP when the service
        // has no http_bindings (TCP-only services fronted by an HTTPS
        // ingress, or a transient state where no pod has bound yet).
        if upstream.routes.is_empty() {
            let upstream_url =
                format!("http://[{}]:{}", upstream.service_ip, upstream.service_port);
            vhost.routes.push(ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec![upstream_url],
                },
            });
        } else {
            for route in &upstream.routes {
                let upstream_urls: Vec<String> = route
                    .upstreams
                    .iter()
                    .map(|u| format!("http://{u}"))
                    .collect();
                vhost.routes.push(ProxyRoute {
                    prefix: route.prefix.clone(),
                    handler: ProxyRouteHandler::ReverseProxy {
                        upstreams: upstream_urls,
                    },
                });
            }
        }
    }

    for (ingress, target) in redirects {
        register_listeners(&mut listener_set, ingress);
        let vhost = ensure_vhost(&mut vhosts, ingress);
        vhost.routes.push(ProxyRoute {
            prefix: "/".to_string(),
            handler: ProxyRouteHandler::Redirect {
                url: target.url.clone(),
                code: target.code,
                preserve_path: target.preserve_path,
            },
        });
    }

    ProxyConfig {
        listeners: listener_set.into_iter().collect(),
        virtual_hosts: vhosts.into_values().collect(),
        l4_routes: vec![],
        warm_cert_hostnames: BTreeSet::new(),
        cert_endpoint_url: None,
    }
}

// r[impl actuate.ingress.plaintext]
fn register_listeners(set: &mut BTreeSet<ProxyListener>, ingress: &IngressDef) {
    // An ingress serves HTTPS only when it terminates TLS *and* HTTP. HTTP
    // termination alone does not imply TLS: a site-ingress attachment whose
    // parent's TLS provisioning mode is `none` terminates HTTP as plaintext,
    // and so listens for HTTP and takes no QUIC listener (HTTP/3 is TLS-only).
    let is_https = ingress.tls && ingress.http_terminate.is_some();

    set.insert(ProxyListener {
        port: ingress.port.get(),
        proto: if is_https {
            ProxyListenerProto::Https
        } else {
            ProxyListenerProto::Http
        },
    });

    if is_https {
        set.insert(ProxyListener {
            port: ingress.port.get(),
            proto: ProxyListenerProto::Quic,
        });
    }

    if let Some(redirect) = &ingress.redirect {
        set.insert(ProxyListener {
            port: redirect.port.get(),
            proto: ProxyListenerProto::Http,
        });
    }
}

// r[impl actuate.ingress.plaintext]
// Vhosts are keyed by `(hostname, tls)`, not by hostname alone: one hostname
// may carry a plaintext ingress on one port and a TLS-terminating one on
// another, and each has to reach the listener matching its own termination.
// Keying by hostname alone would merge them into a single vhost whose
// `tls_acme` is the OR of both, serving the plaintext routes from the HTTPS
// server and dropping them from the HTTP one.
fn ensure_vhost<'a>(
    vhosts: &'a mut BTreeMap<(String, bool), VirtualHost>,
    ingress: &IngressDef,
) -> &'a mut VirtualHost {
    let vhost = vhosts
        .entry((ingress.hostname.clone(), ingress.tls))
        .or_insert_with(|| VirtualHost {
            hostname: ingress.hostname.clone(),
            tls_acme: ingress.tls,
            redirect: None,
            routes: vec![],
        });

    if let Some(redirect) = &ingress.redirect {
        vhost.redirect = Some(HttpRedirect {
            from_port: redirect.port.get(),
            code: redirect.code,
        });
    }

    vhost
}

/// Augment a ProxyConfig with warm-cert hostnames collected from the per-app
/// `OperationProgress.warm_cert_hostnames` sets. Hostnames already present as a
/// vhost (i.e. routed) are skipped — Caddy's vhost-driven acquisition handles
/// those, and adding them to `automate` is redundant.
// r[impl actuate.ingress.warm-certs]
pub fn augment_with_warm_certs(config: &mut ProxyConfig, warm_hostnames: BTreeSet<String>) {
    let already_routed: BTreeSet<&str> = config
        .virtual_hosts
        .iter()
        .filter(|vh| vh.tls_acme)
        .map(|vh| vh.hostname.as_str())
        .collect();

    for host in warm_hostnames {
        if !already_routed.contains(host.as_str()) {
            config.warm_cert_hostnames.insert(host);
        }
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
            app: seedling_protocol::names::AppName::new("test").unwrap(),
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

    #[test]
    fn node_mount_addr_uses_fffe_discriminant() {
        let prefix = test_prefix(); // fd5e:ed12:3456::/48
        let addr = node_mount_addr(&prefix);
        let octets = addr.octets();
        assert_eq!(&octets[..6], &[0xfd, 0x5e, 0xed, 0x12, 0x34, 0x56]);
        assert_eq!(octets[6], 0xff);
        assert_eq!(octets[7], 0xfe);
        assert_eq!(&octets[8..15], &[0u8; 7]);
        assert_eq!(octets[15], 0x01);
    }

    #[test]
    fn node_mount_addr_same_from_node_or_pod_prefix() {
        let node: Ipv6Net = "fd5e:ed12:3456::/48".parse().unwrap();
        let pod: Ipv6Net = "fd5e:ed12:3456:0500::/64".parse().unwrap();
        assert_eq!(node_mount_addr(&node), node_mount_addr(&pod));
    }

    fn ing(hostname: &str, port: u16, tls: bool, http: bool) -> IngressDef {
        use crate::defs::Port;
        use crate::defs::ingress::HttpTermination;
        IngressDef {
            hostname: hostname.to_string(),
            port: Port::new(i64::from(port)).unwrap(),
            tls,
            dtls: false,
            http_terminate: http.then_some(HttpTermination::Http1),
            redirect: None,
            description: None,
        }
    }

    fn upstream(port: u16) -> ServiceUpstream {
        ServiceUpstream {
            routes: vec![],
            service_ip: "fd5e:ed12:3456:200::1".parse().unwrap(),
            service_port: port,
        }
    }

    fn has(cfg: &ProxyConfig, port: u16, proto: ProxyListenerProto) -> bool {
        cfg.listeners
            .iter()
            .any(|l| l.port == port && l.proto == proto)
    }

    // r[verify actuate.ingress.plaintext]
    #[test]
    fn plaintext_http_ingress_listens_for_http() {
        // The shape a site-ingress attachment produces when its parent's TLS
        // provisioning mode is `none`: terminates HTTP, terminates no TLS.
        let cfg = build_proxy_config(&[(ing("clinic.local", 80, false, true), upstream(80))], &[]);

        assert!(
            has(&cfg, 80, ProxyListenerProto::Http),
            "plaintext HTTP ingress must listen for HTTP, got {:?}",
            cfg.listeners
        );
        assert!(
            !has(&cfg, 80, ProxyListenerProto::Https),
            "plaintext HTTP ingress must not listen for HTTPS"
        );
    }

    // r[verify actuate.ingress.plaintext]
    #[test]
    fn plaintext_http_ingress_takes_no_quic_listener() {
        let cfg = build_proxy_config(&[(ing("clinic.local", 80, false, true), upstream(80))], &[]);
        assert!(
            !has(&cfg, 80, ProxyListenerProto::Quic),
            "HTTP/3 is TLS-only, so a plaintext ingress must take no QUIC listener"
        );
    }

    // r[verify actuate.ingress.plaintext]
    #[test]
    fn tls_http_ingress_still_listens_for_https_and_quic() {
        let cfg = build_proxy_config(
            &[(ing("clinic.example.com", 443, true, true), upstream(80))],
            &[],
        );
        assert!(has(&cfg, 443, ProxyListenerProto::Https));
        assert!(has(&cfg, 443, ProxyListenerProto::Quic));
        assert!(!has(&cfg, 443, ProxyListenerProto::Http));
    }

    // r[verify actuate.ingress.plaintext]
    #[test]
    fn tls_passthrough_ingress_takes_no_quic_listener() {
        // `Terminate.Tls + Output.Tcp`: terminates TLS but not HTTP, so it
        // serves no HTTP/3 and must be unaffected by the plaintext handling.
        let cfg = build_proxy_config(
            &[(ing("db.example.com", 5432, true, false), upstream(5432))],
            &[],
        );
        assert!(!has(&cfg, 5432, ProxyListenerProto::Quic));
    }

    // r[verify actuate.ingress.plaintext]
    #[test]
    fn plaintext_and_tls_ingresses_on_one_hostname_stay_separate() {
        // One hostname, plaintext on :80 and TLS-terminating on :443. Keying
        // vhosts by hostname alone would merge these and serve the plaintext
        // routes from the HTTPS server.
        let cfg = build_proxy_config(
            &[
                (ing("clinic.local", 80, false, true), upstream(80)),
                (ing("clinic.local", 443, true, true), upstream(80)),
            ],
            &[],
        );

        assert_eq!(cfg.virtual_hosts.len(), 2, "got {:?}", cfg.virtual_hosts);
        let plain = cfg.virtual_hosts.iter().filter(|v| !v.tls_acme).count();
        let secure = cfg.virtual_hosts.iter().filter(|v| v.tls_acme).count();
        assert_eq!((plain, secure), (1, 1));
        assert!(
            cfg.virtual_hosts
                .iter()
                .all(|v| v.hostname == "clinic.local"),
            "both vhosts describe the same hostname"
        );
        assert!(has(&cfg, 80, ProxyListenerProto::Http));
        assert!(has(&cfg, 443, ProxyListenerProto::Https));
    }
}
