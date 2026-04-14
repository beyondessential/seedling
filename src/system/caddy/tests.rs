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
    };
    let json = build_caddy_config(&config);
    let subjects = &json["apps"]["tls"]["automation"]["policies"][0]["subjects"];
    assert_eq!(subjects[0], "secure.example.com");
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
    };
    let json = build_caddy_config(&config);
    let dial = &json["apps"]["http"]["servers"]["seedling_https"]["routes"][0]["handle"][0]["upstreams"]
        [0]["dial"];
    assert_eq!(dial, "[fd5e:ed12:3456:0100::3]:3000");
}
