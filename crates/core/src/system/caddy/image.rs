//! What the runtime requires of the proxy image.
//!
//! The image is pinned by [`CADDY_IMAGE`](super::startup::CADDY_IMAGE) and
//! pulled at runtime, so nothing at compile time proves it provides what the
//! emitted configuration needs. This module is the single declaration both
//! halves of that check read: the tests below, which walk a configuration the
//! runtime actually emits, and the image workflow, which runs the built image
//! against the same list before publishing it.

/// Module ids the proxy image must provide, as declared in
/// `docker/caddy/required-modules.txt`.
///
/// Embedded rather than duplicated so the workflow and the runtime read one
/// list.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumed by the tests below and, as a file, by the image check in CI; \
                  the daemon has no reason to consult it at run time"
    )
)]
const REQUIRED_MODULES_LIST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docker/caddy/required-modules.txt"
));

/// Directory, under the proxy's data volume, holding its certificate cache.
///
/// The image's storage configuration must put the cache here, and
/// [`cert_observation`](super::cert_observation) reads it from here. Both use
/// this constant, so the contract has one statement rather than two that can
/// drift.
// r[impl infra.proxy.image.cert-cache]
pub(crate) const CERT_CACHE_DIR: &str = "caddy";

/// The module ids the proxy image must register.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumed by the tests below; the image itself is checked against the \
                  same file by the image workflow, not through this accessor"
    )
)]
// r[impl infra.proxy.image.modules]
pub(crate) fn required_modules() -> impl Iterator<Item = &'static str> {
    REQUIRED_MODULES_LIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Collect the Caddy module ids a built configuration references.
///
/// Caddy names a handler by its short name within the enclosing app's
/// namespace, so `{"handler": "reverse_proxy"}` under `apps.http` is the module
/// `http.handlers.reverse_proxy`. Apps that are stock Caddy (`http`, `tls`) are
/// not themselves module ids worth asserting; `layer4` is, because it is the
/// plugin.
#[cfg(test)]
fn referenced_modules(config: &serde_json::Value) -> std::collections::BTreeSet<String> {
    fn handlers(
        node: &serde_json::Value,
        namespace: &str,
        out: &mut std::collections::BTreeSet<String>,
    ) {
        match node {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(name)) = map.get("handler") {
                    out.insert(format!("{namespace}.handlers.{name}"));
                }
                for value in map.values() {
                    handlers(value, namespace, out);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    handlers(value, namespace, out);
                }
            }
            _ => {}
        }
    }

    let mut found = std::collections::BTreeSet::new();
    let Some(apps) = config.get("apps").and_then(|a| a.as_object()) else {
        return found;
    };

    for (app, node) in apps {
        // `layer4` is plugin-provided, so the app id itself must be present.
        if app == "layer4" {
            found.insert(app.clone());
        }
        handlers(node, app, &mut found);
    }

    found
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::system::caddy::config::build_caddy_config;
    use crate::system::types::{
        HttpRedirect, L4Proto, L4Route, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute,
        ProxyRouteHandler, VirtualHost,
    };

    /// A configuration exercising every feature `build_caddy_config` emits.
    ///
    /// The check below is only as complete as this fixture: a feature it does
    /// not exercise contributes no module id and so cannot be caught missing
    /// from the image. Add to it whenever the emitted configuration grows a
    /// handler.
    fn everything_on() -> ProxyConfig {
        ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 80,
                    proto: ProxyListenerProto::Http,
                },
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 443,
                    proto: ProxyListenerProto::Quic,
                },
            ],
            virtual_hosts: vec![
                VirtualHost {
                    hostname: "app.example.com".to_owned(),
                    tls_acme: true,
                    redirect: Some(HttpRedirect {
                        from_port: 80,
                        code: 308,
                    }),
                    routes: vec![ProxyRoute {
                        prefix: "/".to_owned(),
                        handler: ProxyRouteHandler::ReverseProxy {
                            upstreams: vec!["http://[fd5e::1]:3000".to_owned()],
                        },
                    }],
                },
                VirtualHost {
                    hostname: "old.example.com".to_owned(),
                    tls_acme: true,
                    redirect: None,
                    routes: vec![ProxyRoute {
                        prefix: "/".to_owned(),
                        handler: ProxyRouteHandler::Redirect {
                            url: "https://app.example.com".to_owned(),
                            code: 308,
                            preserve_path: true,
                        },
                    }],
                },
            ],
            l4_routes: vec![L4Route {
                port: 5432,
                proto: L4Proto::Tcp,
                upstreams: vec!["[fd5e::2]:5432".to_owned()],
            }],
            warm_cert_hostnames: BTreeSet::from(["warm.example.com".to_owned()]),
            cert_endpoint_url: Some("http://[fd5e::3]:8080/get_certificate".to_owned()),
        }
    }

    // r[verify infra.proxy.image.modules]
    #[test]
    fn every_emitted_module_is_declared_required() {
        let emitted = referenced_modules(&build_caddy_config(&everything_on()));
        assert!(!emitted.is_empty(), "fixture emitted no modules at all");

        let required: BTreeSet<&str> = required_modules().collect();
        let undeclared: Vec<&String> = emitted
            .iter()
            .filter(|module| !required.contains(module.as_str()))
            .collect();

        assert!(
            undeclared.is_empty(),
            "emitted configuration references modules not declared in \
             docker/caddy/required-modules.txt: {undeclared:?}. The image is \
             checked against that file, so an undeclared module makes the \
             whole configuration unappliable once it is emitted."
        );
    }

    // r[verify infra.proxy.image.modules]
    #[test]
    fn required_modules_are_well_formed() {
        let required: Vec<&str> = required_modules().collect();
        assert!(!required.is_empty(), "no required modules declared");
        for module in required {
            assert!(
                !module.contains(char::is_whitespace),
                "module id {module:?} contains whitespace"
            );
        }
    }

    // r[verify infra.proxy.image.cert-cache]
    #[test]
    fn cert_observation_reads_the_declared_cache_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = "certs.example.com";
        let cert_dir = dir
            .path()
            .join(CERT_CACHE_DIR)
            .join("certificates")
            .join("acme-v02.api.letsencrypt.org-directory")
            .join(host);
        std::fs::create_dir_all(&cert_dir).expect("create cert dir");
        std::fs::write(cert_dir.join(format!("{host}.crt")), b"x").expect("write cert");

        assert!(
            super::super::cert_observation::cert_present(dir.path(), host),
            "a certificate under {CERT_CACHE_DIR}/ must be observable; if this \
             fails the image has relocated its cache and cert_valid will never \
             be emitted"
        );
    }
}
