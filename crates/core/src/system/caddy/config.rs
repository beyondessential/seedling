use serde_json::{Value, json};

use crate::runtime::tls::state::is_caddy_internal;
use crate::system::types::{
    L4Proto, ProxyConfig, ProxyListenerProto, RouteBalance, RouteCompress, RouteHeaderOps,
    RouteHeaders, RouteRateLimit, VirtualHost,
};

/// Ports declared for both a plaintext and a TLS listener.
///
/// The HTTP and HTTPS servers are built independently from the listener set,
/// and nothing stopped both from listing `:P`. Caddy refuses a config where
/// two servers bind the same address, and `POST /config/` is all-or-nothing —
/// so one such pair invalidated every route and every cert policy on the
/// node, not just the ingress that caused it.
///
/// Reported rather than quietly resolved: dropping one side would silently
/// serve an ingress on a protocol its author did not ask for.
// r[impl actuate.ingress.plaintext]
pub(crate) fn conflicting_listener_ports(config: &ProxyConfig) -> Vec<u16> {
    use std::collections::BTreeSet;

    let mut plaintext: BTreeSet<u16> = BTreeSet::new();
    let mut tls: BTreeSet<u16> = BTreeSet::new();
    for l in &config.listeners {
        match l.proto {
            ProxyListenerProto::Http => {
                plaintext.insert(l.port);
            }
            ProxyListenerProto::Https | ProxyListenerProto::Quic => {
                tls.insert(l.port);
            }
        }
    }
    plaintext.intersection(&tls).copied().collect()
}

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

    // r[impl ingress.persistent-connections]
    // Each server below carries its listeners and its routes and nothing
    // else. That absence is the implementation: the proxy keeps HTTP/1.1
    // connections alive by default, with an idle timeout it defaults to five
    // minutes, so a client reusing a connection is what happens when we say
    // nothing. Adding `idle_timeout`, `keepalive_interval`, `keepalive_idle`,
    // or `keepalive_count` to a server object here would take that away
    // silently — a request would still be answered, one connection at a time.
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

        // r[impl actuate.ingress.plaintext]
        // A vhost that terminates no TLS is served here, as plaintext, and is
        // absent from the TLS automation subjects below so no certificate is
        // requested for its hostname.
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
        // We emit one or two automation policies — one for hostnames the
        // proxy handles with its internal CA (`.localhost`, `.local`,
        // `.internal`, IP literals, single-label names), and one for the
        // rest. Caddy's automation only auto-picks the internal CA for
        // these names when no policy covers them; once we list a policy
        // for a subject, the unpinned default chain is ACME-only and
        // would fail on names public CAs cannot issue for. Splitting the
        // policy explicitly pins the internal issuer for that bucket.
        //
        // For each policy the chain is:
        //
        //   1. `get_certificate` (when cert_endpoint_url is set): Caddy
        //      asks the daemon by SNI. A 200 returns the runtime-managed
        //      cert (acme-dns / manual / CSR-derived); a 204 (no content)
        //      tells Caddy to fall through.
        //   2. The policy's issuer — Caddy's default chain (ACME) for the
        //      public bucket, the explicit `internal` module for the
        //      internal bucket.
        let (internal_subjects, public_subjects): (Vec<&str>, Vec<&str>) = all_subjects
            .iter()
            .copied()
            .partition(|h| is_caddy_internal(h));

        let mut policies: Vec<Value> = Vec::with_capacity(2);
        if !public_subjects.is_empty() {
            policies.push(Value::Object(build_policy(
                &public_subjects,
                config.cert_endpoint_url.as_deref(),
                None,
            )));
        }
        if !internal_subjects.is_empty() {
            policies.push(Value::Object(build_policy(
                &internal_subjects,
                config.cert_endpoint_url.as_deref(),
                Some(json!([{ "module": "internal" }])),
            )));
        }

        let mut tls = json!({ "automation": { "policies": policies } });
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

fn build_policy(
    subjects: &[&str],
    cert_endpoint_url: Option<&str>,
    issuers: Option<Value>,
) -> serde_json::Map<String, Value> {
    let mut policy = serde_json::Map::new();
    policy.insert("subjects".to_string(), json!(subjects));
    if let Some(url) = cert_endpoint_url {
        policy.insert(
            "get_certificate".to_string(),
            json!([{ "via": "http", "url": url }]),
        );
    }
    if let Some(issuers) = issuers {
        policy.insert("issuers".to_string(), issuers);
    }
    // Eager: we control which subjects Caddy should know about (routed
    // vhosts plus warm-cert hostnames), and we want the cert obtained
    // when we tell Caddy about it, not lazily on first SNI.
    policy.insert("on_demand".to_string(), Value::Bool(false));
    policy
}

fn proxy_routes_for_vhost(vh: &VirtualHost) -> Vec<Value> {
    // r[impl service.http.route.routing]
    // Caddy evaluates routes within a server in order and a `/` matcher
    // matches every request, so longer prefixes must come first or they
    // get shadowed. Sort by prefix length descending; ties (rare) keep
    // their input order.
    let mut routes = vh.routes.clone();
    routes.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
    routes
        .iter()
        .map(|route| {
            let match_expr = if route.prefix == "/" {
                json!({ "host": [&vh.hostname] })
            } else {
                json!({ "host": [&vh.hostname], "path": [format!("{}*", route.prefix)] })
            };

            let handle = match &route.handler {
                crate::system::types::ProxyRouteHandler::ReverseProxy { upstreams, proxy } => {
                    let upstreams: Vec<Value> = upstreams
                        .iter()
                        .map(|u| {
                            let dial = u.strip_prefix("http://").unwrap_or(u).to_string();
                            json!({ "dial": dial })
                        })
                        .collect();

                    let mut chain: Vec<Value> = Vec::with_capacity(4);
                    // r[impl service.http.route.headers]
                    // Ahead of everything else, because its response
                    // operations are deferred to when the response headers are
                    // written. Sitting first is what puts them on a response
                    // the proxy produced in the upstream's place — a
                    // rate-limit rejection, or a failure to reach any upstream
                    // — as well as on one an upstream returned.
                    if !proxy.headers.is_empty() {
                        chain.push(headers_handler(&proxy.headers));
                    }
                    // r[impl service.http.route.rate-limiting]
                    // Ahead of compression and the proxy: an over-limit
                    // request is answered without engaging either, so the
                    // excess costs a backend nothing.
                    if let Some(rate_limit) = &proxy.rate_limit {
                        chain.push(rate_limit_handler(rate_limit));
                    }
                    // r[impl service.http.route.compression]
                    // `encode` wraps the response writer, so it has to sit
                    // ahead of the proxy in the chain to see what comes back.
                    // Caddy skips a response that already carries a
                    // Content-Encoding, so an upstream that compressed for
                    // itself is passed through untouched.
                    if let Some(compress) = &proxy.compress {
                        chain.push(encode_handler(compress));
                    }
                    chain.push(json!({
                        "handler": "reverse_proxy",
                        "upstreams": upstreams,
                        // r[impl service.http.route.balancing]
                        "load_balancing": load_balancing(&proxy.balance),
                    }));
                    Value::Array(chain)
                }
                // r[impl ingress.site.attachment]
                crate::system::types::ProxyRouteHandler::Redirect {
                    url,
                    code,
                    preserve_path,
                } => {
                    // Caddy expands `{http.request.uri}` to the full path
                    // + query of the incoming request, which is what
                    // operators expect from a "preserve path" redirect.
                    // For the URL-verbatim case we strip any trailing slash
                    // so the Location is exactly what the operator typed.
                    let location = if *preserve_path {
                        format!("{}{{http.request.uri}}", url.trim_end_matches('/'))
                    } else {
                        url.clone()
                    };
                    json!([{
                        "handler": "static_response",
                        "status_code": code,
                        "headers": { "Location": [location] },
                    }])
                }
            };

            json!({
                "match": [match_expr],
                "handle": handle,
                "terminal": true,
            })
        })
        .collect()
}

// r[impl service.http.route.headers]
fn headers_handler(headers: &RouteHeaders) -> Value {
    let mut handler = serde_json::Map::new();
    handler.insert("handler".into(), json!("headers"));
    if !headers.request.is_empty() {
        handler.insert("request".into(), header_ops(&headers.request));
    }
    if !headers.response.is_empty() {
        let mut response = header_ops(&headers.response);
        // Applied when the response headers are written rather than on the way
        // in, so they reach whatever response the route ends up serving rather
        // than only one that came back from an upstream.
        response["deferred"] = json!(true);
        handler.insert("response".into(), response);
    }
    Value::Object(handler)
}

/// One direction's operations in the proxy's own vocabulary.
///
/// `replace` is emitted as the proxy's `set`, which is assignment. The proxy
/// has its own `replace`, but that one is a substring substitution over the
/// existing value and means something else entirely; emitting it here would
/// turn every assignment into a search-and-replace that silently does nothing
/// when the value is not already present.
fn header_ops(ops: &RouteHeaderOps) -> Value {
    let mut out = serde_json::Map::new();
    if !ops.replace.is_empty() {
        out.insert("set".into(), json!(ops.replace));
    }
    if !ops.add.is_empty() {
        out.insert("add".into(), json!(ops.add));
    }
    if !ops.remove.is_empty() {
        out.insert("delete".into(), json!(ops.remove));
    }
    Value::Object(out)
}

// r[impl service.http.route.rate-limiting]
fn rate_limit_handler(limit: &RouteRateLimit) -> Value {
    // The zone name identifies the declaration and is built in
    // `reconcile::proxy`; see `RouteRateLimit::zone` for why it is not derived
    // from anything here.
    //
    // `client_ip` is the address the proxy attributes to the request, and
    // carries no port: the module masks a key only when it parses as a bare
    // address, and silently leaves anything else whole.
    //
    // Every key emitted must be one the pinned module declares, or Caddy's
    // strict decoding fails the whole document: see `caddy::image`.
    json!({
        "handler": "rate_limit",
        "rate_limits": {
            limit.zone.to_string(): {
                "key": "{http.request.client_ip}",
                "window": secs_to_nanos(limit.window_secs),
                "max_events": limit.max_events,
                // Only the v6 prefix is set. The module leaves an address
                // untouched when the prefix for its version is unset, so IPv4
                // clients are counted per address while IPv6 clients are
                // counted per /64. A party holds its whole /64, so counting
                // those separately would hand it a budget per address and the
                // limit would not bind it at all.
                "ipv6_prefix": 64,
            }
        }
    })
}

// r[impl service.http.route.compression]
fn encode_handler(compress: &RouteCompress) -> Value {
    // `encodings` is a module map keyed by encoder name; `prefer` is what
    // carries the order when the client expresses no preference.
    let encodings: serde_json::Map<String, Value> = compress
        .encodings
        .iter()
        .map(|e| (e.clone(), json!({})))
        .collect();

    let mut handler = serde_json::Map::new();
    handler.insert("handler".to_string(), json!("encode"));
    handler.insert("encodings".to_string(), Value::Object(encodings));
    handler.insert("prefer".to_string(), json!(compress.encodings));
    handler.insert("minimum_length".to_string(), json!(compress.minimum_length));
    // Left out unless the app named its own set, so the proxy applies its
    // default matcher of text-like content types.
    if let Some(types) = &compress.content_types {
        handler.insert(
            "match".to_string(),
            json!({ "headers": { "Content-Type": types } }),
        );
    }
    Value::Object(handler)
}

// r[impl service.http.route.balancing]
fn load_balancing(balance: &RouteBalance) -> Value {
    json!({
        "selection_policy": { "policy": balance.policy },
        "try_duration": secs_to_nanos(balance.try_duration_secs),
        "try_interval": secs_to_nanos(balance.interval_secs),
    })
}

/// Caddy durations accept a JSON number of nanoseconds, which is exact for the
/// fractional-second intervals the BSL allows.
fn secs_to_nanos(secs: f64) -> i64 {
    (secs * 1_000_000_000.0).round() as i64
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
