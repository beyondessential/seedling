use super::config::build_caddy_config;
use crate::system::translate::proxy::build_proxy_config;
use crate::system::types::{
    HttpRedirect, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute, ProxyRouteHandler,
    RouteRateLimit, VirtualHost,
};

fn default_proxy() -> crate::system::types::RouteProxy {
    crate::defs::service::ResolvedRouteProxy::default().into()
}

fn http_vhost(hostname: &str, upstream: &str) -> VirtualHost {
    VirtualHost {
        hostname: hostname.to_string(),
        tls_acme: false,
        redirect: None,
        routes: vec![ProxyRoute {
            prefix: "/".to_string(),
            handler: ProxyRouteHandler::ReverseProxy {
                upstreams: vec![format!("http://{upstream}")],
                proxy: default_proxy(),
            },
        }],
    }
}

fn https_vhost(hostname: &str, upstream: &str) -> VirtualHost {
    VirtualHost {
        hostname: hostname.to_string(),
        tls_acme: true,
        redirect: Some(HttpRedirect {
            from_port: 80,
            code: 308,
        }),
        routes: vec![ProxyRoute {
            prefix: "/".to_string(),
            handler: ProxyRouteHandler::ReverseProxy {
                upstreams: vec![format!("http://{upstream}")],
                proxy: default_proxy(),
            },
        }],
    }
}

#[test]
fn empty_config_produces_empty_servers() {
    let config = ProxyConfig::default();
    let json = build_caddy_config(&config);
    let servers = &json["apps"]["http"]["servers"];
    assert!(servers.as_object().is_none_or(|m| m.is_empty()));
}

#[test]
fn http_only_vhost_goes_in_http_server() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![http_vhost("example.com", "[fd5e::1]:3000")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let servers = &json["apps"]["http"]["servers"];
    assert!(servers["seedling_http"].is_object());
    assert!(servers["seedling_https"].is_null());
}

#[test]
fn https_vhost_goes_in_https_server_redirect_in_http() {
    let config = ProxyConfig {
        listeners: vec![
            ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            },
            ProxyListener {
                port: 80,
                proto: ProxyListenerProto::Http,
            },
        ],
        virtual_hosts: vec![https_vhost("example.com", "[fd5e::1]:3000")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let servers = &json["apps"]["http"]["servers"];
    assert!(
        servers["seedling_https"].is_object(),
        "missing https server"
    );
    assert!(servers["seedling_http"].is_object(), "missing http server");

    // https server should have proxy routes
    let https_routes = &servers["seedling_https"]["routes"];
    assert!(https_routes.as_array().is_some_and(|r| !r.is_empty()));

    // http server should have redirect route
    let http_routes = &servers["seedling_http"]["routes"];
    let redirect = &http_routes[0];
    assert_eq!(redirect["handle"][0]["handler"], "static_response");
    assert_eq!(redirect["handle"][0]["status_code"], 308);
}

// r[verify tls.strategy.acme-dns]
// r[verify tls.policy.apply]
#[test]
fn tls_acme_subjects_appear_in_automation() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "secure.example.com".to_string(),
            tls_acme: true,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                    proxy: default_proxy(),
                },
            }],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let subjects = &json["apps"]["tls"]["automation"]["policies"][0]["subjects"];
    assert_eq!(subjects[0], "secure.example.com");
}

// r[verify actuate.ingress.warm-certs]
#[test]
fn warm_cert_only_emits_certificates_automate_and_policy() {
    let mut config = ProxyConfig::default();
    config
        .warm_cert_hostnames
        .insert("warm.example.com".to_string());
    let json = build_caddy_config(&config);

    // No HTTP server is created — there are no routes.
    assert!(
        json["apps"]["http"]["servers"]
            .as_object()
            .is_none_or(|m| m.is_empty())
    );

    // The hostname appears in both the automation policy and certificates.automate.
    let subjects = &json["apps"]["tls"]["automation"]["policies"][0]["subjects"];
    assert_eq!(subjects[0], "warm.example.com");
    let automate = &json["apps"]["tls"]["certificates"]["automate"];
    assert_eq!(automate[0], "warm.example.com");
}

// r[verify actuate.ingress.warm-certs]
#[test]
fn warm_cert_skipped_when_already_routed() {
    let mut config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "shared.example.com".to_string(),
            tls_acme: true,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                    proxy: default_proxy(),
                },
            }],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    // Asking to warm a hostname that's already routed should be a no-op
    // (the hostname is already covered by lazy acquisition via the server block).
    config
        .warm_cert_hostnames
        .insert("shared.example.com".to_string());
    let json = build_caddy_config(&config);

    // Subjects appear once in the policy.
    let subjects = json["apps"]["tls"]["automation"]["policies"][0]["subjects"]
        .as_array()
        .expect("subjects array")
        .iter()
        .filter(|s| s.as_str() == Some("shared.example.com"))
        .count();
    assert_eq!(subjects, 1, "subject should not be duplicated");

    // certificates.automate is absent (or doesn't include the routed hostname).
    let automate = &json["apps"]["tls"]["certificates"];
    assert!(
        automate.is_null(),
        "certificates.automate should not be set when all warm hostnames are already routed"
    );
}

#[test]
fn dial_strips_http_scheme() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "x.com".to_string(),
            tls_acme: true,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec!["http://[fd5e:ed12:3456:0100::3]:3000".to_string()],
                    proxy: default_proxy(),
                },
            }],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let dial = &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][1]["upstreams"]
        [0]["dial"];
    assert_eq!(dial, "[fd5e:ed12:3456:0100::3]:3000");
}

#[test]
fn https_server_includes_quic_listener() {
    let config = ProxyConfig {
        listeners: vec![
            ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            },
            ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Quic,
            },
        ],
        virtual_hosts: vec![VirtualHost {
            hostname: "h3.example.com".to_string(),
            tls_acme: true,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                    proxy: default_proxy(),
                },
            }],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let listen = &json["apps"]["http"]["servers"]["seedling_https"]["listen"];
    let listen_strs: Vec<&str> = listen
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(listen_strs.contains(&":443"));
    assert_eq!(
        listen_strs.len(),
        1,
        "QUIC port duplicates HTTPS port, dedup should collapse them"
    );
}

// r[verify tls.cert.serve]
// r[verify tls.strategy.default]
#[test]
fn cert_endpoint_url_is_emitted_inside_automation_policy() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![https_vhost("example.com", "[fd5e::1]:3000")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: Some("http://[fd5e::ff:1]:8443/get".to_string()),
    };
    let json = build_caddy_config(&config);
    // Per Caddy's schema, get_certificate is a per-policy field, not
    // a top-level tls.certificates field.
    let policy = &json["apps"]["tls"]["automation"]["policies"][0];
    let getters = &policy["get_certificate"];
    let arr = getters
        .as_array()
        .expect("get_certificate must be an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["via"], "http");
    assert_eq!(arr[0]["url"], "http://[fd5e::ff:1]:8443/get");
    assert_eq!(policy["subjects"][0], "example.com");
    // For the public bucket we do NOT pin `issuers`: Caddy's default
    // chain (ACME) is correct here. The internal bucket pins
    // `issuers: [{module: internal}]`; see the dedicated test.
    assert!(
        policy["issuers"].is_null(),
        "issuers must be left unset on the public policy so the default ACME chain applies"
    );
    assert!(
        json["apps"]["tls"]["certificates"]["get_certificate"].is_null(),
        "get_certificate must not appear under tls.certificates"
    );
}

// r[verify tls.strategy.default]
#[test]
fn internal_hostnames_pin_caddy_internal_issuer() {
    // `node.localhost` cannot be issued by a public CA. With a single
    // unpinned policy Caddy would fail with
    // "subject does not qualify for a public certificate"; splitting the
    // policy so that internal hostnames carry an explicit `internal`
    // issuer is what makes Caddy reach for its self-signed CA instead.
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![
            https_vhost("public.example.com", "[fd5e::1]:3000"),
            https_vhost("node.localhost", "[fd5e::1]:3001"),
        ],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let policies = json["apps"]["tls"]["automation"]["policies"]
        .as_array()
        .expect("policies array");
    assert_eq!(policies.len(), 2, "one policy per bucket");

    let public = policies
        .iter()
        .find(|p| p["subjects"].as_array().unwrap()[0] == "public.example.com")
        .expect("public policy");
    assert!(
        public["issuers"].is_null(),
        "public bucket leaves issuers unset"
    );

    let internal = policies
        .iter()
        .find(|p| p["subjects"].as_array().unwrap()[0] == "node.localhost")
        .expect("internal policy");
    let issuers = internal["issuers"].as_array().expect("issuers array");
    assert_eq!(issuers.len(), 1);
    assert_eq!(issuers[0]["module"], "internal");
}

// r[verify tls.strategy.default]
#[test]
fn only_internal_hostnames_emit_just_internal_policy() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![https_vhost("node.localhost", "[fd5e::1]:3000")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let policies = json["apps"]["tls"]["automation"]["policies"]
        .as_array()
        .expect("policies array");
    assert_eq!(policies.len(), 1, "one bucket only");
    assert_eq!(policies[0]["issuers"][0]["module"], "internal");
    assert_eq!(policies[0]["subjects"][0], "node.localhost");
}

// r[verify service.http.route.routing]
#[test]
fn vhost_with_multiple_prefixes_emits_per_prefix_routes_longest_first() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "example.com".to_string(),
            tls_acme: true,
            redirect: None,
            // Caddy is order-sensitive within a server's routes; the
            // builder must emit `/api` before `/` so the API path matches
            // first. We assert that order regardless of how the input
            // was constructed.
            routes: vec![
                ProxyRoute {
                    prefix: "/".to_string(),
                    handler: ProxyRouteHandler::ReverseProxy {
                        upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                        proxy: default_proxy(),
                    },
                },
                ProxyRoute {
                    prefix: "/api".to_string(),
                    handler: ProxyRouteHandler::ReverseProxy {
                        upstreams: vec!["http://[fd5e::2]:3000".to_string()],
                        proxy: default_proxy(),
                    },
                },
            ],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let routes = &json["apps"]["http"]["servers"]["seedling_https"]["routes"];
    let routes = routes.as_array().unwrap();
    assert_eq!(routes.len(), 2);
    // Each route's match should carry the right host + path.
    assert_eq!(routes[0]["match"][0]["path"][0], "/api*");
    assert_eq!(routes[0]["match"][0]["host"][0], "example.com");
    // `/` route has no `path` matcher (just host).
    assert_eq!(routes[1]["match"][0]["host"][0], "example.com");
    assert!(routes[1]["match"][0]["path"].is_null());
}

#[test]
fn cert_endpoint_url_without_subjects_emits_no_tls_app() {
    // Without any TLS-terminating vhosts there is no automation policy
    // to attach get_certificate to and no SNI traffic to serve, so the
    // tls app is omitted entirely. Once an ingress declares a hostname
    // the next reconciler tick rebuilds the config with the policy +
    // get_certificate wired in.
    let config = ProxyConfig {
        listeners: vec![],
        virtual_hosts: vec![],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: Some("http://[fd5e::ff:1]:8443/get".to_string()),
    };
    let json = build_caddy_config(&config);
    assert!(json["apps"]["tls"].is_null());
}

#[test]
fn l4_routes_emit_layer4_servers_per_port_and_proto() {
    use crate::system::types::{L4Proto, L4Route};

    let config = ProxyConfig {
        listeners: vec![],
        virtual_hosts: vec![],
        l4_routes: vec![
            L4Route {
                port: 5432,
                proto: L4Proto::Tcp,
                upstreams: vec!["[fd5e::1]:5432".to_string()],
            },
            L4Route {
                port: 514,
                proto: L4Proto::Udp,
                upstreams: vec!["[fd5e::2]:514".to_string(), "[fd5e::3]:514".to_string()],
            },
        ],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);

    let tcp = &json["apps"]["layer4"]["servers"]["l4_tcp_5432"];
    assert_eq!(tcp["listen"][0], "tcp/:5432");
    assert_eq!(
        tcp["routes"][0]["handle"][0]["handler"], "proxy",
        "layer4 uses the generic proxy handler"
    );
    assert_eq!(
        tcp["routes"][0]["handle"][0]["upstreams"][0]["dial"][0],
        "[fd5e::1]:5432"
    );

    let udp = &json["apps"]["layer4"]["servers"]["l4_udp_514"];
    assert_eq!(udp["listen"][0], "udp/:514");
    let upstreams = udp["routes"][0]["handle"][0]["upstreams"]
        .as_array()
        .expect("upstreams array");
    assert_eq!(upstreams.len(), 2);
}

#[test]
fn no_l4_routes_means_no_layer4_app() {
    let json = build_caddy_config(&ProxyConfig::default());
    assert!(json["apps"]["layer4"].is_null());
}

// r[verify ingress.site.attachment]
#[test]
fn redirect_handler_emits_static_response_with_and_without_path_preservation() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 443,
            proto: ProxyListenerProto::Https,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "old.example.com".to_string(),
            tls_acme: true,
            redirect: None,
            routes: vec![
                ProxyRoute {
                    prefix: "/".to_string(),
                    handler: ProxyRouteHandler::Redirect {
                        url: "https://new.example.com/".to_string(),
                        code: 308,
                        preserve_path: true,
                    },
                },
                ProxyRoute {
                    prefix: "/legacy".to_string(),
                    handler: ProxyRouteHandler::Redirect {
                        url: "https://archive.example.com/page".to_string(),
                        code: 302,
                        preserve_path: false,
                    },
                },
            ],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let routes = json["apps"]["http"]["servers"]["seedling_https"]["routes"]
        .as_array()
        .expect("routes");

    // Longest prefix first: /legacy before /.
    let legacy = &routes[0]["handle"][0];
    assert_eq!(legacy["handler"], "static_response");
    assert_eq!(legacy["status_code"], 302);
    assert_eq!(
        legacy["headers"]["Location"][0], "https://archive.example.com/page",
        "verbatim redirect must not append the request path"
    );

    let root = &routes[1]["handle"][0];
    assert_eq!(root["status_code"], 308);
    assert_eq!(
        root["headers"]["Location"][0], "https://new.example.com{http.request.uri}",
        "path-preserving redirect appends the request URI after trimming the trailing slash"
    );
}

#[test]
fn http_to_https_redirect_targets_nonstandard_https_port() {
    let config = ProxyConfig {
        listeners: vec![
            ProxyListener {
                port: 8443,
                proto: ProxyListenerProto::Https,
            },
            ProxyListener {
                port: 8080,
                proto: ProxyListenerProto::Http,
            },
        ],
        virtual_hosts: vec![https_vhost("example.com", "[fd5e::1]:3000")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let redirect = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"][0];
    assert_eq!(
        redirect["headers"]["Location"][0],
        "https://{http.request.host}:8443{http.request.uri}"
    );
}

// ── Plaintext ingress serving ────────────────────────────────────────────────

fn plaintext_ingress(hostname: &str, port: u16) -> crate::defs::ingress::IngressDef {
    use crate::defs::{Port, ingress::HttpTermination};
    // The shape reconcile::site_proxy builds for an HTTP attachment on a site
    // ingress whose TLS provisioning mode is `none`.
    crate::defs::ingress::IngressDef {
        hostname: hostname.to_string(),
        port: Port::new(i64::from(port)).unwrap(),
        tls: false,
        dtls: false,
        http_terminate: Some(HttpTermination::Http1),
        redirect: None,
        description: None,
    }
}

fn service_upstream(port: u16) -> crate::system::translate::proxy::ServiceUpstream {
    crate::system::translate::proxy::ServiceUpstream {
        routes: vec![],
        service_ip: "fd5e:ed12:3456:200::1".parse().unwrap(),
        service_port: port,
        proxy: default_proxy(),
    }
}

// r[verify actuate.ingress.plaintext]
#[test]
fn plaintext_only_config_requests_no_certificate() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![http_vhost("clinic.local", "[fd5e::1]:80")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);

    assert!(
        json["apps"]["tls"].is_null(),
        "a plaintext-only config must request no certificate, got {}",
        json["apps"]["tls"]
    );
}

// r[verify actuate.ingress.plaintext]
#[test]
fn plaintext_vhost_gets_no_redirect_route() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![http_vhost("clinic.local", "[fd5e::1]:80")],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let routes = json["apps"]["http"]["servers"]["seedling_http"]["routes"].to_string();

    assert!(
        !routes.contains("static_response") && !routes.contains("Location"),
        "plaintext vhost declares no redirect, so none should be emitted: {routes}"
    );
}

// r[verify actuate.ingress.plaintext]
#[test]
fn plaintext_ingress_is_served_end_to_end() {
    // Translate then render, so this covers the whole path a `.local` host
    // takes: site ingress with no TLS, HTTP attachment on :80, forwarding to
    // an app service. Previously this produced an empty Caddy config.
    let proxy = build_proxy_config(
        &[(plaintext_ingress("clinic.local", 80), service_upstream(80))],
        &[],
    );
    let json = build_caddy_config(&proxy);
    let servers = &json["apps"]["http"]["servers"];

    assert!(
        servers["seedling_http"].is_object(),
        "plaintext host must be served over HTTP, got {}",
        serde_json::to_string(&json).unwrap()
    );
    assert_eq!(servers["seedling_http"]["listen"][0], ":80");
    let routes = servers["seedling_http"]["routes"].to_string();
    assert!(
        routes.contains("clinic.local"),
        "host matcher missing: {routes}"
    );
    assert!(
        routes.contains("fd5e:ed12:3456:200::1"),
        "upstream missing: {routes}"
    );
    assert!(
        json["apps"]["tls"].is_null(),
        "no certificate should be requested for a plaintext host"
    );
}

// r[verify actuate.ingress.plaintext]
#[test]
fn mixed_termination_on_one_hostname_serves_each_from_its_own_listener() {
    let mut secure = plaintext_ingress("clinic.local", 443);
    secure.tls = true;

    let proxy = build_proxy_config(
        &[
            (plaintext_ingress("clinic.local", 80), service_upstream(80)),
            (secure, service_upstream(80)),
        ],
        &[],
    );
    let json = build_caddy_config(&proxy);
    let servers = &json["apps"]["http"]["servers"];

    assert!(
        servers["seedling_http"].is_object(),
        "the plaintext route must still be served over HTTP: {}",
        serde_json::to_string(&json).unwrap()
    );
    assert!(
        servers["seedling_https"].is_object(),
        "the TLS route must be served over HTTPS"
    );
    assert_eq!(servers["seedling_http"]["listen"][0], ":80");
    assert_eq!(
        servers["seedling_https"]["routes"]
            .as_array()
            .map(|r| r.len()),
        Some(1),
        "only the TLS-terminating vhost belongs in the HTTPS server"
    );
}

// r[verify service.http.route.compression]
#[test]
fn reverse_proxy_routes_compress_ahead_of_the_proxy() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![http_vhost("app.example.com", "http://[fd5e::1]:3000")],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    // encode must wrap the proxy, so it comes first in the chain.
    assert_eq!(handle[0]["handler"], "encode");
    assert_eq!(handle[1]["handler"], "reverse_proxy");

    assert!(handle[0]["encodings"]["zstd"].is_object());
    assert!(handle[0]["encodings"]["gzip"].is_object());
    assert_eq!(handle[0]["prefer"][0], "zstd");
    assert_eq!(handle[0]["minimum_length"], 512);
    // Left out so the proxy applies its own text-like content-type matcher.
    assert!(handle[0]["match"].is_null());
}

// r[verify service.http.route.balancing]
#[test]
fn reverse_proxy_routes_carry_balancing_defaults() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![http_vhost("app.example.com", "http://[fd5e::1]:3000")],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let lb = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"][1]["load_balancing"];

    assert_eq!(lb["selection_policy"]["policy"], "round_robin");
    // Caddy durations as nanoseconds: 5s and 250ms.
    assert_eq!(lb["try_duration"], 5_000_000_000i64);
    assert_eq!(lb["try_interval"], 250_000_000i64);
}

// r[verify service.http.route.compression]
// r[verify service.http.route.balancing]
#[test]
fn redirect_routes_are_emitted_bare() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "old.example.com".to_string(),
            tls_acme: false,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::Redirect {
                    url: "https://new.example.com".to_string(),
                    code: 308,
                    preserve_path: true,
                },
            }],
        }],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    // A redirect has nothing to compress and no pool to balance across.
    assert_eq!(handle[0]["handler"], "static_response");
    assert!(handle[1].is_null());
}

// r[verify service.http.route.compression]
#[test]
fn compression_can_be_switched_off_for_a_route() {
    use crate::system::types::{RouteBalance, RouteProxy};

    let proxy = RouteProxy {
        compress: None,
        balance: RouteBalance {
            policy: "least_conn".to_string(),
            try_duration_secs: 10.0,
            interval_secs: 0.25,
        },
        rate_limit: None,
    };
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "app.example.com".to_string(),
            tls_acme: false,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::ReverseProxy {
                    upstreams: vec!["http://[fd5e::1]:3000".to_string()],
                    proxy,
                },
            }],
        }],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    assert_eq!(handle[0]["handler"], "reverse_proxy");
    assert_eq!(
        handle[0]["load_balancing"]["selection_policy"]["policy"],
        "least_conn"
    );
    assert_eq!(
        handle[0]["load_balancing"]["try_duration"],
        10_000_000_000i64
    );
}

/// A reverse-proxy route at `prefix` carrying `rate_limit`.
fn limited_route(prefix: &str, rate_limit: Option<RouteRateLimit>) -> ProxyRoute {
    use crate::system::types::{RouteBalance, RouteProxy};

    ProxyRoute {
        prefix: prefix.to_string(),
        handler: ProxyRouteHandler::ReverseProxy {
            upstreams: vec!["http://[fd5e::1]:3000".to_string()],
            proxy: RouteProxy {
                compress: None,
                balance: RouteBalance {
                    policy: "round_robin".to_string(),
                    try_duration_secs: 5.0,
                    interval_secs: 0.25,
                },
                rate_limit,
            },
        },
    }
}

fn vhost_with(hostname: &str, routes: Vec<ProxyRoute>) -> ProxyConfig {
    ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: hostname.to_string(),
            tls_acme: false,
            redirect: None,
            routes,
        }],
        ..Default::default()
    }
}

// r[verify service.http.route.rate-limiting]
#[test]
fn a_limited_route_carries_the_rate_limit_handler() {
    let config = vhost_with(
        "app.example.com",
        vec![limited_route(
            "/api",
            Some(RouteRateLimit {
                max_events: 1000,
                window_secs: 1.0,
            }),
        )],
    );
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    // First in the chain, so the excess never reaches the proxy.
    assert_eq!(handle[0]["handler"], "rate_limit");
    assert_eq!(handle[1]["handler"], "reverse_proxy");

    let zone = &handle[0]["rate_limits"]["http://app.example.com/api"];
    assert_eq!(zone["max_events"], 1000);
    // Caddy durations as nanoseconds: 1s.
    assert_eq!(zone["window"], 1_000_000_000i64);
    assert_eq!(zone["key"], "{http.request.client_ip}");
}

// r[verify service.http.route.rate-limiting]
#[test]
fn an_unlimited_route_carries_no_rate_limit_handler() {
    let config = vhost_with("app.example.com", vec![limited_route("/api", None)]);
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    assert_eq!(handle[0]["handler"], "reverse_proxy");
    assert!(handle[1].is_null());
}

// r[verify service.http.route.rate-limiting]
// r[verify infra.proxy.image.modules]
#[test]
fn emitted_zone_uses_only_fields_the_pinned_module_declares() {
    use super::config::RATE_LIMIT_ZONE_FIELDS;

    let config = vhost_with(
        "app.example.com",
        vec![limited_route(
            "/api",
            Some(RouteRateLimit {
                max_events: 10,
                window_secs: 1.0,
            }),
        )],
    );
    let json = build_caddy_config(&config);
    let zone = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"][0]["rate_limits"]
        ["http://app.example.com/api"];

    // Caddy decodes module config strictly, so a field the pinned tag does not
    // declare fails the whole document and drops ingress for every vhost on
    // the host, not merely the route that declared the limit. `ipv4_prefix`
    // and `ipv6_prefix` are the live example: they exist upstream but in no
    // released version, so emitting them would have been a host-wide outage
    // that no assertion on our own JSON would have caught.
    let emitted = zone.as_object().expect("zone is an object");
    let unknown: Vec<&String> = emitted
        .keys()
        .filter(|k| !RATE_LIMIT_ZONE_FIELDS.contains(&k.as_str()))
        .collect();
    assert!(
        unknown.is_empty(),
        "emitted rate-limit zone fields not declared by the pinned \
         caddy-ratelimit tag: {unknown:?}"
    );
}

// r[verify service.http.route.rate-limiting]
// r[verify service.http.route.routing]
#[test]
fn a_longer_prefix_carries_its_own_zone_ahead_of_the_shorter_one() {
    let config = vhost_with(
        "app.example.com",
        vec![
            limited_route(
                "/api",
                Some(RouteRateLimit {
                    max_events: 1000,
                    window_secs: 1.0,
                }),
            ),
            limited_route(
                "/api/login",
                Some(RouteRateLimit {
                    max_events: 10,
                    window_secs: 1.0,
                }),
            ),
        ],
    );
    let json = build_caddy_config(&config);
    let routes = &json["apps"]["http"]["servers"]["seedling_http"]["routes"];

    // Longest prefix first, and terminal, so a login request is counted
    // against the tighter zone alone.
    assert_eq!(routes[0]["match"][0]["path"][0], "/api/login*");
    assert_eq!(routes[0]["terminal"], true);
    assert_eq!(routes[1]["match"][0]["path"][0], "/api*");

    // Distinct zone names: the module pools zones by name process-wide, so a
    // shared name would put both prefixes in one bucket.
    let tight = &routes[0]["handle"][0]["rate_limits"];
    let loose = &routes[1]["handle"][0]["rate_limits"];
    assert_eq!(tight["http://app.example.com/api/login"]["max_events"], 10);
    assert_eq!(loose["http://app.example.com/api"]["max_events"], 1000);
    assert!(tight["http://app.example.com/api"].is_null());
}

// r[verify service.http.route.rate-limiting]
#[test]
fn the_same_prefix_on_two_hostnames_gets_distinct_zones() {
    let limit = Some(RouteRateLimit {
        max_events: 10,
        window_secs: 1.0,
    });
    let mut config = vhost_with("a.example.com", vec![limited_route("/api", limit)]);
    config.virtual_hosts.push(VirtualHost {
        hostname: "b.example.com".to_string(),
        tls_acme: false,
        redirect: None,
        routes: vec![limited_route("/api", limit)],
    });
    let json = build_caddy_config(&config);
    let routes = &json["apps"]["http"]["servers"]["seedling_http"]["routes"];

    assert!(!routes[0]["handle"][0]["rate_limits"]["http://a.example.com/api"].is_null());
    assert!(!routes[1]["handle"][0]["rate_limits"]["http://b.example.com/api"].is_null());
}

// r[verify service.http.route.rate-limiting]
#[test]
fn redirect_routes_are_never_rate_limited() {
    let config = ProxyConfig {
        listeners: vec![ProxyListener {
            port: 80,
            proto: ProxyListenerProto::Http,
        }],
        virtual_hosts: vec![VirtualHost {
            hostname: "old.example.com".to_string(),
            tls_acme: false,
            redirect: None,
            routes: vec![ProxyRoute {
                prefix: "/".to_string(),
                handler: ProxyRouteHandler::Redirect {
                    url: "https://new.example.com".to_string(),
                    code: 308,
                    preserve_path: true,
                },
            }],
        }],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let handle = &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"];

    assert_eq!(handle[0]["handler"], "static_response");
    assert!(handle[1].is_null());
}
// r[verify infra.proxy.upgrade.cache]
#[test]
fn old_cached_config_without_rate_limit_still_deserialises() {
    // The proxy config is cached as JSON and read back on startup, so a
    // document written by an older build must still load: a field added here
    // that is not optional on the wire would strand the cache and, with it,
    // the upgrade path that depends on it.
    let old = r#"{
      "listeners":[{"port":80,"proto":"Http"}],
      "virtual_hosts":[{"hostname":"app.example.com","tls_acme":false,"redirect":null,
        "routes":[{"prefix":"/","handler":{"ReverseProxy":{
          "upstreams":["http://[fd5e::1]:3000"],
          "proxy":{"compress":null,"balance":{"policy":"round_robin","try_duration_secs":5.0,"interval_secs":0.25}}
        }}}]}],
      "l4_routes":[],"warm_cert_hostnames":[],"cert_endpoint_url":null
    }"#;
    let parsed: Result<crate::system::types::ProxyConfig, _> = serde_json::from_str(old);
    let cfg = parsed.expect("a cached config from before rate limiting must still load");
    let crate::system::types::ProxyRouteHandler::ReverseProxy { proxy, .. } =
        &cfg.virtual_hosts[0].routes[0].handler
    else {
        panic!("expected reverse proxy")
    };
    assert_eq!(proxy.rate_limit, None);
}

// r[verify service.http.route.rate-limiting]
#[test]
fn one_hostname_terminating_both_ways_gets_a_zone_per_termination() {
    let limit = Some(RouteRateLimit {
        max_events: 10,
        window_secs: 1.0,
    });
    // Vhosts are keyed by hostname and termination, so this is one hostname
    // appearing twice in the same document. Sharing a zone name here would
    // put both terminations in one bucket, and whichever the module
    // provisioned first would silently govern the other.
    let config = ProxyConfig {
        listeners: vec![
            ProxyListener {
                port: 80,
                proto: ProxyListenerProto::Http,
            },
            ProxyListener {
                port: 443,
                proto: ProxyListenerProto::Https,
            },
        ],
        virtual_hosts: vec![
            VirtualHost {
                hostname: "app.example.com".to_string(),
                tls_acme: false,
                redirect: None,
                routes: vec![limited_route("/api", limit)],
            },
            VirtualHost {
                hostname: "app.example.com".to_string(),
                tls_acme: true,
                redirect: None,
                routes: vec![limited_route("/api", limit)],
            },
        ],
        ..Default::default()
    };
    let json = build_caddy_config(&config);
    let plain =
        &json["apps"]["http"]["servers"]["seedling_http"]["routes"][0]["handle"][0]["rate_limits"];
    let tls =
        &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][0]["rate_limits"];

    assert!(!plain["http://app.example.com/api"].is_null());
    assert!(!tls["https://app.example.com/api"].is_null());
    assert!(tls["http://app.example.com/api"].is_null());
}
