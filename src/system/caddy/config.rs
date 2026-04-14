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
        https_listens.push(format!(":{p}/quic"));
    }
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
    let tls_subjects: Vec<&str> = config
        .virtual_hosts
        .iter()
        .filter(|vh| vh.tls_acme)
        .map(|vh| vh.hostname.as_str())
        .collect();

    let mut apps = json!({ "http": { "servers": servers } });

    if !tls_subjects.is_empty() {
        apps["tls"] = json!({
            "automation": {
                "policies": [{
                    "subjects": tls_subjects,
                    "issuers": [{ "module": "acme" }],
                }]
            }
        });
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
