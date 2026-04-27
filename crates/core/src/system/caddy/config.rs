use serde_json::{Value, json};

use crate::system::types::{L4Proto, ProxyConfig, ProxyListenerProto, VirtualHost};

pub(crate) fn build_caddy_config(config: &ProxyConfig) -> Value {
    let http_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Http)
        .map(|l| l.port)
        .collect();

    let https_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Https)
        .map(|l| l.port)
        .collect();

    let quic_ports: Vec<u16> = config
        .listeners
        .iter()
        .filter(|l| l.proto == ProxyListenerProto::Quic)
        .map(|l| l.port)
        .collect();

    let mut servers = serde_json::Map::new();

    // --- HTTPS server ---
    let mut https_listens: Vec<String> = https_ports.iter().map(|p| format!(":{p}")).collect();

    for p in &quic_ports {
        https_listens.push(format!(":{p}"));
    }

    https_listens.sort();
    https_listens.dedup();

    if !https_listens.is_empty() {
        let https_routes: Vec<Value> = config
            .virtual_hosts
            .iter()
            .filter(|vh| vh.tls_acme)
            .flat_map(proxy_routes_for_vhost)
            .collect();

        if !https_routes.is_empty() {
            servers.insert(
                "seedling_https".to_string(),
                json!({ "listen": https_listens, "routes": https_routes }),
            );
        }
    }

    // --- HTTP server ---
    let http_listens: Vec<String> = http_ports.iter().map(|p| format!(":{p}")).collect();
    if !http_listens.is_empty() {
        let mut http_routes: Vec<Value> = Vec::new();

        for vh in &config.virtual_hosts {
            if let Some(redirect) = &vh.redirect {
                http_routes.push(redirect_route(&vh.hostname, redirect.code, &https_ports));
            } else if !vh.tls_acme {
                http_routes.extend(proxy_routes_for_vhost(vh));
            }
        }

        if !http_routes.is_empty() {
            servers.insert(
                "seedling_http".to_string(),
                json!({ "listen": http_listens, "routes": http_routes }),
            );
        }
    }

    // --- TLS automation ---
    // Subjects covered by the automation policy: routed TLS vhosts plus any
    // warm-cert hostnames that aren't already in routed vhosts.
    let routed_subjects: std::collections::BTreeSet<&str> = config
        .virtual_hosts
        .iter()
        .filter(|vh| vh.tls_acme)
        .map(|vh| vh.hostname.as_str())
        .collect();
    let warm_subjects: std::collections::BTreeSet<&str> = config
        .warm_cert_hostnames
        .iter()
        .map(|h| h.as_str())
        .filter(|h| !routed_subjects.contains(h))
        .collect();
    let mut all_subjects: Vec<&str> = routed_subjects
        .iter()
        .chain(warm_subjects.iter())
        .copied()
        .collect();
    all_subjects.sort();
    all_subjects.dedup();

    let mut apps = json!({ "http": { "servers": servers } });

    if !all_subjects.is_empty() {
        // r[impl actuate.ingress.warm-certs]
        // r[impl tls.strategy.default]
        // r[impl tls.cert.serve]
        // l[impl ingress.certificates]
        // For routed subjects, Caddy acquires the cert lazily on first
        // request to the matching server; for warm-only subjects, no server
        // matches and Caddy must be told explicitly via certificates.automate.
        //
        // The single automation policy here covers every TLS-terminating
        // hostname. Its issuer defaults to Caddy's ACME-HTTP-01 against
        // Let's Encrypt (tls.strategy.default). When the daemon's cert
        // endpoint is set, we add `get_certificate` to the same policy so
        // Caddy first asks the daemon by SNI: a 200 returns the
        // runtime-managed cert (acme-dns / manual / CSR-derived); a 404
        // falls through to the policy's regular issuer.
        let mut policy = serde_json::Map::new();
        policy.insert("subjects".to_string(), json!(all_subjects));
        if let Some(url) = &config.cert_endpoint_url {
            policy.insert(
                "get_certificate".to_string(),
                json!([{
                    "via": "http",
                    "url": url,
                }]),
            );
        }

        let mut tls = json!({
            "automation": {
                "policies": [Value::Object(policy)]
            }
        });
        if !warm_subjects.is_empty() {
            let warm_list: Vec<&str> = warm_subjects.iter().copied().collect();
            tls["certificates"] = json!({ "automate": warm_list });
        }
        apps["tls"] = tls;
    }

    if !config.l4_routes.is_empty() {
        let mut l4_servers = serde_json::Map::new();

        for route in &config.l4_routes {
            let proto_str = match route.proto {
                L4Proto::Tcp => "tcp",
                L4Proto::Udp => "udp",
            };
            let server_name = format!("l4_{proto_str}_{}", route.port);
            let listen = format!("{proto_str}/:{}", route.port);

            let upstreams: Vec<Value> = route
                .upstreams
                .iter()
                .map(|u| json!({ "dial": [u] }))
                .collect();

            l4_servers.insert(
                server_name,
                json!({
                    "listen": [listen],
                    "routes": [{
                        "handle": [{
                            "handler": "proxy",
                            "upstreams": upstreams,
                        }]
                    }]
                }),
            );
        }

        apps["layer4"] = json!({ "servers": l4_servers });
    }

    json!({ "admin": { "listen": "unix//run/caddy-admin/admin.sock" }, "apps": apps })
}

fn proxy_routes_for_vhost(vh: &VirtualHost) -> Vec<Value> {
    vh.routes
        .iter()
        .map(|route| {
            let match_expr = if route.prefix == "/" {
                json!({ "host": [&vh.hostname] })
            } else {
                json!({ "host": [&vh.hostname], "path": [format!("{}*", route.prefix)] })
            };

            let upstreams: Vec<Value> = route
                .upstreams
                .iter()
                .map(|u| {
                    let dial = u.strip_prefix("http://").unwrap_or(u).to_string();
                    json!({ "dial": dial })
                })
                .collect();

            json!({
                "match": [match_expr],
                "handle": [{
                    "handler": "reverse_proxy",
                    "upstreams": upstreams,
                }],
                "terminal": true,
            })
        })
        .collect()
}

fn redirect_route(hostname: &str, code: u16, https_ports: &[u16]) -> Value {
    let target_port = https_ports.first().copied().unwrap_or(443);
    let location = if target_port == 443 {
        "https://{http.request.host}{http.request.uri}".to_string()
    } else {
        format!("https://{{http.request.host}}:{target_port}{{http.request.uri}}")
    };

    json!({
        "match": [{ "host": [hostname] }],
        "handle": [{
            "handler": "static_response",
            "status_code": code,
            "headers": { "Location": [location] },
        }],
        "terminal": true,
    })
}
