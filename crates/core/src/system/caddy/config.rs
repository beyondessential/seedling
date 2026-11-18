use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::runtime::tls::state::is_caddy_internal;
use crate::system::types::{
    HttpRedirect, L4Proto, L4Route, ProxyConfig, ProxyListenerProto, RedirectSegment, RouteBalance,
    RouteCompress, RouteHeaderOps, RouteHeaders, RouteRateLimit, VirtualHost,
};

/// An address and transport one server of the rendered configuration binds.
///
/// A transport is either `tcp` or `udp`. HTTP/3 binds the UDP address of the
/// HTTPS server it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Bind {
    pub transport: &'static str,
    pub port: u16,
}

impl std::fmt::Display for Bind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.transport, self.port)
    }
}

/// A rendered proxy document, with every address its servers bind.
pub(crate) struct Rendered {
    pub json: Value,
    binds: Vec<Bind>,
}

impl Rendered {
    /// Addresses the document binds more than once, across every server it
    /// emits: HTTP, HTTPS and HTTP/3, layer 4, and DTLS.
    ///
    /// Caddy refuses a config where two servers bind the same address, and
    /// `POST /config/` is all-or-nothing, so one such pair invalidates every
    /// route and every cert policy on the node, not just the ingress that
    /// caused it. The claims the runtime resolves keep this from happening;
    /// this is the check that a mistake in them fails as one ingress's apply,
    /// and not as the whole document's rejection.
    ///
    /// Reported rather than quietly resolved: dropping one side would silently
    /// serve an ingress on a protocol its author did not ask for.
    // spec: PRX#shared-ports
    pub(crate) fn conflicting_binds(&self) -> Vec<Bind> {
        let mut seen: BTreeSet<&Bind> = BTreeSet::new();
        let mut conflicts: BTreeSet<Bind> = BTreeSet::new();
        for bind in &self.binds {
            if !seen.insert(bind) {
                conflicts.insert(bind.clone());
            }
        }
        conflicts.into_iter().collect()
    }
}

/// The TLS versions an ingress that terminates TLS accepts.
// spec: ING#termination
fn tls_versions() -> Value {
    json!({ "protocol_min": "tls1.2", "protocol_max": "tls1.3" })
}

pub(crate) fn build_caddy_config(config: &ProxyConfig) -> Value {
    render(config).json
}

/// Render the proxy document for `config`, once, with what it binds.
pub(crate) fn render(config: &ProxyConfig) -> Rendered {
    let mut binds: Vec<Bind> = Vec::new();
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

    // spec: PRX#persistent-connections
    // Each server below carries its listeners, its routes and, for plaintext,
    // the switch that keeps it plaintext, and nothing else. That absence is
    // the implementation: the proxy keeps HTTP/1.1
    // connections alive by default, with an idle timeout it defaults to five
    // minutes, so a client reusing a connection is what happens when we say
    // nothing. Adding `idle_timeout`, `keepalive_interval`, `keepalive_idle`,
    // or `keepalive_count` to a server object here would take that away
    // silently — a request would still be answered, one connection at a time.
    let mut servers = serde_json::Map::new();

    // spec: PRX#port-scope
    // One server per listener port. A server's routes match on host (and
    // path), never on the port the request arrived on, so a server spanning
    // several ports would answer every vhost on every one of them, and two
    // ingresses for one hostname on different ports would share a route list.
    // Binding each server to a single port makes the proxy's own listener
    // the port match.
    //
    // --- HTTPS servers ---
    // HTTP/3 shares the TCP port number, so a QUIC listener lands on the
    // server of its port rather than on a server of its own.
    let https_listen_ports: BTreeSet<u16> =
        https_ports.iter().chain(&quic_ports).copied().collect();

    // spec: PRX#shared-ports
    // TLS the proxy terminates for a connection that is not HTTP, by the name
    // the client sends, per port.
    let mut tls_streams: BTreeMap<u16, Vec<&L4Route>> = BTreeMap::new();
    for route in config
        .l4_routes
        .iter()
        .filter(|r| r.proto == L4Proto::Tcp && r.terminate_tls)
    {
        tls_streams.entry(route.port).or_default().push(route);
    }
    // Ports where an HTTPS server is emitted, and so also takes the streams.
    let mut shared_ports: BTreeSet<u16> = BTreeSet::new();

    for port in https_listen_ports {
        let routes: Vec<Value> = config
            .virtual_hosts
            .iter()
            .filter(|vh| vh.tls_acme && vh.port == port)
            .flat_map(proxy_routes_for_vhost)
            .collect();

        if !routes.is_empty() {
            // spec: ING#termination
            // Both TLS versions are named rather than left to the proxy's
            // defaults, so a change of those cannot narrow what an ingress
            // accepts.
            let mut server = json!({
                "listen": [format!(":{port}")],
                "routes": routes,
                "tls_connection_policies": [tls_versions()],
            });
            // spec: PRX#shared-ports
            // A TLS-terminating ingress that is not HTTP shares the port by
            // SNI. The proxy's layer 4 module reads the hello and handles the
            // connection of such a name itself; any other falls through to
            // the HTTPS server. The TLS wrapper that follows is the proxy's
            // own, and has to be named for the layer 4 wrapper to come ahead
            // of it: left out, it is placed first, and the layer 4 module
            // would be reading the handshake's ciphertext.
            if let Some(streams) = tls_streams.get(&port) {
                server["listener_wrappers"] = json!([
                    { "wrapper": "layer4", "routes": tls_stream_routes(streams) },
                    { "wrapper": "tls" },
                ]);
                shared_ports.insert(port);
            }
            servers.insert(format!("seedling_https_{port}"), server);
            binds.push(Bind {
                transport: "tcp",
                port,
            });
            if quic_ports.contains(&port) {
                binds.push(Bind {
                    transport: "udp",
                    port,
                });
            }
        }
    }

    // --- HTTP servers ---
    for &port in http_ports.iter().collect::<BTreeSet<_>>() {
        let mut routes: Vec<Value> = Vec::new();

        // spec: PRX#plaintext-ingresses
        // A vhost that terminates no TLS is served here, as plaintext, and is
        // absent from the TLS automation subjects below so no certificate is
        // requested for its hostname.
        for vh in &config.virtual_hosts {
            if let Some(redirect) = &vh.redirect {
                // spec: PRX#port-scope
                // Answers on the port the redirect names, which is not the
                // port of the ingress that declared it.
                if redirect.from_port == port {
                    routes.push(redirect_route(&vh.hostname, redirect));
                }
            } else if !vh.tls_acme && vh.port == port {
                routes.extend(proxy_routes_for_vhost(vh));
            }
        }

        if !routes.is_empty() {
            // spec: PRX#plaintext-ingresses
            // The proxy turns TLS on for any server listening off its HTTP
            // port that matches hostnames, so left to itself it would answer
            // a plaintext ingress on `:8080`, or a redirect declared there,
            // with TLS. On the HTTP port this changes nothing: the proxy
            // already leaves a server there in plaintext.
            servers.insert(
                format!("seedling_http_{port}"),
                json!({
                    "listen": [format!(":{port}")],
                    "routes": routes,
                    "automatic_https": { "disable": true },
                }),
            );
            binds.push(Bind {
                transport: "tcp",
                port,
            });
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
        // spec: RT#certificate-warming
        // spec: CHAIN#default-chain
        // spec: TLS#serving
        // spec: ING#certificates
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
        // spec: CHAIN#proxy-strategies
        // Subjects the proxy may renew only later than its default are split
        // out by the point they are given, since the ratio belongs to a policy
        // and not to a subject. Everything else keeps the one policy it had.
        let (held_back, public_subjects): (Vec<&str>, Vec<&str>) = public_subjects
            .iter()
            .copied()
            .partition(|h| config.renewal_ratios.contains_key(*h));
        if !public_subjects.is_empty() {
            policies.push(Value::Object(build_policy(
                &public_subjects,
                config.cert_endpoint_url.as_deref(),
                None,
                None,
            )));
        }
        let mut by_ratio: std::collections::BTreeMap<u64, (f64, Vec<&str>)> =
            std::collections::BTreeMap::new();
        for host in held_back {
            let ratio = config.renewal_ratios[host];
            by_ratio
                .entry(ratio.to_bits())
                .or_insert_with(|| (ratio, Vec::new()))
                .1
                .push(host);
        }
        for (ratio, subjects) in by_ratio.into_values() {
            policies.push(Value::Object(build_policy(
                &subjects,
                config.cert_endpoint_url.as_deref(),
                None,
                Some(ratio),
            )));
        }
        if !internal_subjects.is_empty() {
            policies.push(Value::Object(build_policy(
                &internal_subjects,
                config.cert_endpoint_url.as_deref(),
                Some(json!([{ "module": "internal" }])),
                None,
            )));
        }

        let mut tls = json!({ "automation": { "policies": policies } });
        if !warm_subjects.is_empty() {
            let warm_list: Vec<&str> = warm_subjects.iter().copied().collect();
            tls["certificates"] = json!({ "automate": warm_list });
        }
        apps["tls"] = tls;
    }

    // spec: PRX#shared-ports
    // One server per port and transport. The routes of a port that carries
    // TLS are told apart by the SNI each answers to, so a handshake with no
    // name, or one no route answers to, matches none and is closed rather than
    // handed to whichever route came first.
    let mut l4_servers = serde_json::Map::new();
    let mut l4_ports: BTreeSet<(L4Proto, u16)> = BTreeSet::new();
    for route in &config.l4_routes {
        // Served by the HTTPS server of its port.
        if route.proto == L4Proto::Tcp && route.terminate_tls && shared_ports.contains(&route.port)
        {
            continue;
        }
        l4_ports.insert((route.proto, route.port));
    }
    for (proto, port) in l4_ports {
        let proto_str = match proto {
            L4Proto::Tcp => "tcp",
            L4Proto::Udp => "udp",
        };
        let routes: Vec<Value> = config
            .l4_routes
            .iter()
            .filter(|r| r.proto == proto && r.port == port)
            .map(l4_route)
            .collect();
        l4_servers.insert(
            format!("l4_{proto_str}_{port}"),
            json!({
                "listen": [format!("{proto_str}/:{port}")],
                "routes": routes,
            }),
        );
        binds.push(Bind {
            transport: proto_str,
            port,
        });
    }
    if !l4_servers.is_empty() {
        apps["layer4"] = json!({ "servers": l4_servers });
    }

    // spec: PRX#dtls-terminating-ingresses
    // A server of the proxy's own DTLS module per port, which listens on its
    // own socket rather than through the layer 4 module: that one splits a
    // UDP port into a flow per remote address before any handler runs, so
    // could neither follow a session whose client changed address nor keep
    // the session for the idle period the requirement gives.
    if !config.dtls_routes.is_empty() {
        let mut dtls_servers = serde_json::Map::new();
        for route in &config.dtls_routes {
            dtls_servers.insert(
                format!("dtls_{}", route.port),
                json!({
                    "listen": [format!(":{}", route.port)],
                    "hostname": route.hostname,
                    "upstream": route.upstream,
                    "ingress": route.ingress,
                }),
            );
            binds.push(Bind {
                transport: "udp",
                port: route.port,
            });
        }
        apps["seedling_dtls"] = json!({ "servers": dtls_servers });
    }

    Rendered {
        json: json!({ "admin": { "listen": "unix//run/caddy-admin/admin.sock" }, "apps": apps }),
        binds,
    }
}

/// One layer 4 route: the traffic passed through to its upstreams, or, where
/// it terminates TLS, the plaintext stream inside the session for the name it
/// is answering to.
// spec: ING#termination
// spec: PRX#shared-ports
fn l4_route(route: &L4Route) -> Value {
    let upstreams: Vec<Value> = route
        .upstreams
        .iter()
        .map(|u| json!({ "dial": [u] }))
        .collect();
    let proxy = json!({ "handler": "proxy", "upstreams": upstreams });
    if !route.terminate_tls {
        return json!({ "handle": [proxy] });
    }
    // A route cached before routes carried a hostname terminates TLS for any
    // name, as the daemon that wrote it rendered it, so replaying it
    // reproduces what was running; the next reconciliation tick rebuilds it
    // from the declaration, with its name.
    // spec: PRX#proxy-upgrades
    match &route.hostname {
        Some(hostname) => json!({
            "match": [{ "tls": { "sni": [hostname] } }],
            "handle": [tls_handler(), proxy],
        }),
        None => json!({ "handle": [tls_handler(), proxy] }),
    }
}

/// Routes for the TLS the proxy terminates ahead of an HTTPS server: one for
/// each name, and no route for any other, which falls through to the server.
fn tls_stream_routes(routes: &[&L4Route]) -> Vec<Value> {
    routes.iter().copied().map(l4_route).collect()
}

/// The proxy's TLS handler takes its certificate from the proxy's own TLS app,
/// by SNI, as an HTTPS ingress's does: the hostname is among those the TLS app
/// automates.
// spec: ING#termination
fn tls_handler() -> Value {
    json!({ "handler": "tls", "connection_policies": [tls_versions()] })
}

fn build_policy(
    subjects: &[&str],
    cert_endpoint_url: Option<&str>,
    issuers: Option<Value>,
    renewal_window_ratio: Option<f64>,
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
    if let Some(ratio) = renewal_window_ratio {
        policy.insert("renewal_window_ratio".to_string(), json!(ratio));
    }
    // Eager: we control which subjects Caddy should know about (routed
    // vhosts plus warm-cert hostnames), and we want the cert obtained
    // when we tell Caddy about it, not lazily on first SNI.
    policy.insert("on_demand".to_string(), Value::Bool(false));
    policy
}

fn proxy_routes_for_vhost(vh: &VirtualHost) -> Vec<Value> {
    // spec: PRX#route-emission
    // Caddy evaluates routes within a server in order and a `/` matcher
    // matches every request, so longer prefixes must come first or they
    // get shadowed. Sort by prefix length descending; ties (rare) keep
    // their input order.
    let mut routes = vh.routes.clone();
    routes.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
    routes
        .iter()
        .map(|route| {
            let match_expr = match &route.handler {
                // spec: PRX#redirect-routes
                // Matched on segment boundaries, unlike a proxied prefix. The
                // tail is spliced into a `Location`, so a request that merely
                // begins with the prefix — `/v1/login.example.net` under
                // `/v1/login` — would contribute a tail that is not a path at
                // all, and against an absolute-URL target would name a host
                // the caller chose rather than the app.
                crate::system::types::ProxyRouteHandler::RouteRedirect { .. } => {
                    let prefix = redirect_prefix(&route.prefix);
                    json!({
                        "host": [&vh.hostname],
                        "path": [prefix, format!("{prefix}/*")],
                    })
                }
                _ if route.prefix == "/" => json!({ "host": [&vh.hostname] }),
                _ => json!({ "host": [&vh.hostname], "path": [format!("{}*", route.prefix)] }),
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
                    // spec: PRX#header-operations
                    // Ahead of everything else, because its response
                    // operations are deferred to when the response headers are
                    // written. Sitting first is what puts them on a response
                    // the proxy produced in the upstream's place — a
                    // rate-limit rejection, or a failure to reach any upstream
                    // — as well as on one an upstream returned.
                    if !proxy.headers.is_empty() {
                        chain.push(headers_handler(&proxy.headers));
                    }
                    // spec: PRX#route-selection
                    // Answered at once rather than left to the proxy, which
                    // would hold the request for its retry window first.
                    if upstreams.is_empty() {
                        chain.push(json!({
                            "handler": "static_response",
                            "status_code": 503,
                        }));
                        return json!({
                            "match": [match_expr],
                            "handle": chain,
                            "terminal": true,
                        });
                    }
                    // spec: PRX#rate-limiting
                    // Ahead of compression and the proxy: an over-limit
                    // request is answered without engaging either, so the
                    // excess costs a backend nothing.
                    if let Some(rate_limit) = &proxy.rate_limit {
                        chain.push(rate_limit_handler(rate_limit));
                    }
                    // spec: PRX#compression
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
                        // spec: PRX#balancing
                        "load_balancing": load_balancing(&proxy.balance),
                    }));
                    Value::Array(chain)
                }
                // spec: PRX#redirect-routes
                crate::system::types::ProxyRouteHandler::RouteRedirect {
                    target,
                    code,
                    headers,
                } => {
                    let (location, needs_request_parts) = redirect_location(target);

                    let mut chain: Vec<Value> = Vec::with_capacity(3);
                    // spec: PRX#header-operations
                    // First, as on a proxied route: its response operations
                    // are deferred to when the headers are written, which is
                    // what puts them on the response this route serves.
                    if !headers.is_empty() {
                        chain.push(headers_handler(&RouteHeaders {
                            request: RouteHeaderOps::default(),
                            response: headers.clone(),
                        }));
                    }
                    if needs_request_parts {
                        chain.push(request_parts_handler(&route.prefix));
                    }
                    chain.push(json!({
                        "handler": "static_response",
                        "status_code": code,
                        "headers": { "Location": [location] },
                    }));
                    Value::Array(chain)
                }
                // spec: SITE#attachments
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

// spec: PRX#header-operations
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

// spec: PRX#rate-limiting
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

// spec: PRX#compression
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

// spec: PRX#balancing
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

/// The placeholders [`request_parts_handler`] defines, each empty when the
/// request carries no such part.
const TAIL_VAR: &str = "{seedling.redirect.tail}";
const QUERY_VAR: &str = "{seedling.redirect.query}";

/// The prefix a redirect route matches and measures its tail from.
///
/// The declaration layer already normalises a prefix to this spelling, so this
/// only has work to do for a configuration cached before it did. Trimming here
/// as well keeps the matcher and the extraction agreeing with each other
/// whatever the document holds.
fn redirect_prefix(prefix: &str) -> &str {
    let trimmed = prefix.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

/// Build the `Location` template for a route redirect, and say whether it
/// names any part of the request.
// spec: PRX#redirect-routes
fn redirect_location(target: &[RedirectSegment]) -> (String, bool) {
    let mut location = String::new();
    let mut names_request_parts = false;
    for segment in target {
        match segment {
            // Safe to inline: a braced word is refused in a declared target,
            // so nothing here is read back as a placeholder of its own.
            RedirectSegment::Literal(text) => location.push_str(text),
            RedirectSegment::Tail => {
                location.push_str(TAIL_VAR);
                names_request_parts = true;
            }
            RedirectSegment::Query => {
                location.push_str(QUERY_VAR);
                names_request_parts = true;
            }
        }
    }
    (location, names_request_parts)
}

/// Define [`TAIL_VAR`] and [`QUERY_VAR`] for the handlers after it.
///
/// Both are cut out of the request line in its escaped form, which is the
/// form a `Location` has to carry. Reading the tail off the decoded path
/// instead would let a `%3F` in a path segment arrive at the client as the
/// `?` that starts a query.
///
/// Taking the prefix off the request with a rewrite and reading the remainder
/// back would be the obvious alternative, and is not equivalent: a request
/// for exactly the prefix leaves an empty path, which the proxy reports as
/// `/` rather than as nothing, so the tail would never be empty.
///
/// The two are defined together because they are one cut of one string.
// spec: PRX#redirect-routes
fn request_parts_handler(prefix: &str) -> Value {
    let prefix = regexp_literal(redirect_prefix(prefix));
    json!({
        "handler": "map",
        "source": "{http.request.uri}",
        "destinations": [TAIL_VAR, QUERY_VAR],
        "mappings": [{
            // Case-insensitive because the path matcher that chose this route
            // is: left case-sensitive, a request for `/V1/Login/reset` would
            // match the route, miss here, and be redirected to the bare target
            // with its path remainder and query dropped silently.
            //
            // Braces are excluded from both captures. The proxy substitutes a
            // braced word from its own state, its environment among it, which
            // is why a declared target may not carry one — and the request
            // line is written by a client rather than by the app. Excluding
            // them means a brace-carrying request falls to the defaults below
            // instead of putting client text where a placeholder is read.
            "input_regexp": format!("(?i)^{prefix}([^?{{}}]*)(\\?[^{{}}]*)?$"),
            "outputs": ["${1}", "${2}"],
        }],
        // A request that does not match — its escaped form not beginning with
        // the prefix, or carrying a brace — contributes nothing, rather than
        // contributing something nothing checked.
        "defaults": ["", ""],
    })
}

/// Escape a literal for the proxy's regular-expression syntax.
///
/// Only ASCII punctuation is escaped: the syntax rejects a backslash before a
/// character outside ASCII, and such a character is a literal already.
fn regexp_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() && !ch.is_ascii_alphanumeric() && ch != '_' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

// spec: ING#redirect-to-https
fn redirect_route(hostname: &str, redirect: &HttpRedirect) -> Value {
    let location = if redirect.to_port == 443 {
        "https://{http.request.host}{http.request.uri}".to_string()
    } else {
        format!(
            "https://{{http.request.host}}:{}{{http.request.uri}}",
            redirect.to_port
        )
    };

    json!({
        "match": [{ "host": [hostname] }],
        "handle": [{
            "handler": "static_response",
            "status_code": redirect.code,
            "headers": { "Location": [location] },
        }],
        "terminal": true,
    })
}
