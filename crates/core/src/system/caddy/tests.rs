use super::*;
use crate::system::types::{
    HttpRedirect, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute, VirtualHost,
};

fn http_vhost(hostname: &str, upstream: &str) -> VirtualHost {
    VirtualHost {
        hostname: hostname.to_string(),
        tls_acme: false,
        redirect: None,
        routes: vec![ProxyRoute {
            prefix: "/".to_string(),
            upstreams: vec![format!("http://{upstream}")],
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
            upstreams: vec![format!("http://{upstream}")],
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
                upstreams: vec!["http://[fd5e::1]:3000".to_string()],
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
                upstreams: vec!["http://[fd5e::1]:3000".to_string()],
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
                upstreams: vec!["http://[fd5e:ed12:3456:0100::3]:3000".to_string()],
            }],
        }],
        l4_routes: vec![],
        warm_cert_hostnames: Default::default(),
        cert_endpoint_url: None,
    };
    let json = build_caddy_config(&config);
    let dial = &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][0]["upstreams"]
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
                upstreams: vec!["http://[fd5e::1]:3000".to_string()],
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
    // We deliberately do NOT pin `issuers`: Caddy's default chain picks
    // ACME for public hostnames and the internal CA for non-public ones,
    // which is the right behaviour for tls.strategy.default.
    assert!(
        policy["issuers"].is_null(),
        "issuers must be left unset so Caddy's default chain applies"
    );
    assert!(
        json["apps"]["tls"]["certificates"]["get_certificate"].is_null(),
        "get_certificate must not appear under tls.certificates"
    );
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
