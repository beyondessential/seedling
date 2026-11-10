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
                // An encoder is named by its key in the `encodings` map, not
                // by a `handler` field.
                if let Some(serde_json::Value::Object(encodings)) = map.get("encodings") {
                    for name in encodings.keys() {
                        out.insert(format!("{namespace}.encoders.{name}"));
                    }
                }
                // A selection policy is named by the inline `policy` key of
                // the `selection_policy` object.
                if let Some(serde_json::Value::Object(policy)) = map.get("selection_policy")
                    && let Some(serde_json::Value::String(name)) = policy.get("policy")
                {
                    out.insert(format!(
                        "{namespace}.reverse_proxy.selection_policies.{name}"
                    ));
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

/// The keys the emitted `rate_limit` handler may carry, at the tag pinned in
/// `docker/caddy/Containerfile`, alongside the module list that pin is
/// otherwise checked against.
///
/// Caddy decodes module config strictly, so emitting a key the pinned tag does
/// not declare fails the whole proxy document and drops ingress for every
/// vhost on the host — not merely the route that declared the limit. That
/// applies at both levels of the handler, so both are listed: the handler
/// object and each zone within it.
///
/// These are the keys we emit and have checked the tag against, not everything
/// it accepts: the list is a whitelist to be widened deliberately. Emitting a
/// further key means confirming the pinned tag declares it and adding it here
/// — `ipv4_prefix` and `ipv6_prefix`, for instance, are declared only on the
/// commit the Containerfile pins and by no release, so emitting one against
/// the previous pin would have been a host-wide outage.
#[cfg(test)]
pub(super) const RATE_LIMIT_HANDLER_FIELDS: &[&str] = &["handler", "rate_limits"];

#[cfg(test)]
pub(super) const RATE_LIMIT_ZONE_FIELDS: &[&str] =
    &["key", "window", "max_events", "ipv4_prefix", "ipv6_prefix"];

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::system::caddy::config::build_caddy_config;
    use crate::system::types::{
        HttpRedirect, L4Proto, L4Route, ProxyConfig, ProxyListener, ProxyListenerProto, ProxyRoute,
        ProxyRouteHandler, RedirectSegment, RouteCompress, RouteHeaderOps, RouteHeaders,
        RouteProxy, RouteZone, VirtualHost,
    };

    /// Default proxy settings, with a rate limit added: limiting is off by
    /// default, so the defaults alone would leave its handler out of the
    /// fixture and its module unchecked. Header operations are set in both
    /// directions, in every verb the emitter has.
    fn rate_limited_proxy() -> Box<RouteProxy> {
        let resolved = crate::defs::service::ResolvedRouteProxy {
            rate_limit: Some(crate::defs::service::ResolvedRateLimit {
                settings: crate::defs::service::RateLimitSettings {
                    max_events: 1000,
                    window_secs: 1.0,
                },
                scope: crate::defs::service::RateLimitScope::Service,
            }),
            ..Default::default()
        };
        let mut proxy = RouteProxy::from_resolved(resolved, || RouteZone("demo/web".to_string()));
        proxy.headers = RouteHeaders {
            request: header_ops(),
            response: header_ops(),
        };
        Box::new(proxy)
    }

    fn header_ops() -> RouteHeaderOps {
        RouteHeaderOps {
            replace: BTreeMap::from([("Cache-Control".to_owned(), vec!["no-store".to_owned()])]),
            add: BTreeMap::from([("Vary".to_owned(), vec!["Accept".to_owned()])]),
            remove: vec!["Server".to_owned()],
        }
    }

    /// Default proxy settings with the balancing policy named and compression
    /// on, so each selection policy and each encoder is emitted somewhere.
    fn compressed_proxy(policy: &str, content_types: Option<Vec<String>>) -> Box<RouteProxy> {
        let mut proxy = RouteProxy::unlimited(Default::default());
        proxy.balance.policy = policy.to_owned();
        proxy.compress = Some(RouteCompress {
            encodings: vec!["zstd".to_owned(), "gzip".to_owned()],
            minimum_length: 512,
            content_types,
        });
        Box::new(proxy)
    }

    fn proxied(prefix: &str, proxy: Box<RouteProxy>) -> ProxyRoute {
        ProxyRoute {
            prefix: prefix.to_owned(),
            handler: ProxyRouteHandler::ReverseProxy {
                upstreams: vec!["http://[fd5e::1]:3000".to_owned()],
                proxy,
            },
        }
    }

    /// A configuration exercising every feature `build_caddy_config` emits.
    ///
    /// Both the module-id check and the validation against the real image are
    /// only as complete as this fixture: a feature it does not exercise
    /// contributes no module id and no field, so it cannot be caught missing
    /// from the image. Add to it whenever the emitted configuration grows a
    /// handler, a field, or a shape.
    fn everything_on() -> ProxyConfig {
        let tls_vhost = |hostname: &str, routes: Vec<ProxyRoute>| VirtualHost {
            hostname: hostname.to_owned(),
            tls_acme: true,
            port: 443,
            redirect: None,
            routes,
        };

        let mut app = tls_vhost(
            "app.example.com",
            vec![
                proxied("/", rate_limited_proxy()),
                proxied("/api", compressed_proxy("least_conn", None)),
                proxied(
                    "/assets",
                    compressed_proxy("random", Some(vec!["text/css".to_owned()])),
                ),
                proxied("/first", compressed_proxy("first", None)),
                ProxyRoute {
                    prefix: "/old".to_owned(),
                    handler: ProxyRouteHandler::RouteRedirect {
                        target: vec![
                            RedirectSegment::Literal("https://new.example.com/v2".to_owned()),
                            RedirectSegment::Tail,
                            RedirectSegment::Query,
                        ],
                        code: 308,
                        headers: header_ops(),
                    },
                },
            ],
        );
        app.redirect = Some(HttpRedirect {
            from_port: 80,
            to_port: 443,
            code: 308,
        });

        // The same hostname on a second port, which is a server of its own.
        let mut admin = tls_vhost(
            "app.example.com",
            vec![proxied("/", compressed_proxy("round_robin", None))],
        );
        admin.port = 8443;

        ProxyConfig {
            listeners: vec![
                ProxyListener {
                    port: 8443,
                    proto: ProxyListenerProto::Https,
                },
                ProxyListener {
                    port: 8443,
                    proto: ProxyListenerProto::Quic,
                },
                ProxyListener {
                    port: 80,
                    proto: ProxyListenerProto::Http,
                },
                ProxyListener {
                    port: 8080,
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
                app,
                admin,
                tls_vhost(
                    "old.example.com",
                    vec![ProxyRoute {
                        prefix: "/".to_owned(),
                        handler: ProxyRouteHandler::Redirect {
                            url: "https://app.example.com".to_owned(),
                            code: 308,
                            preserve_path: true,
                        },
                    }],
                ),
                VirtualHost {
                    hostname: "plain.example.com".to_owned(),
                    tls_acme: false,
                    port: 8080,
                    redirect: None,
                    routes: vec![proxied("/", compressed_proxy("round_robin", None))],
                },
                tls_vhost(
                    "slow.example.com",
                    vec![proxied("/", compressed_proxy("round_robin", None))],
                ),
                // Names the proxy's own CA issues for, which take the
                // internal-issuer automation policy rather than the default.
                tls_vhost(
                    "app.internal",
                    vec![proxied("/", compressed_proxy("round_robin", None))],
                ),
            ],
            l4_routes: vec![
                L4Route {
                    port: 5432,
                    proto: L4Proto::Tcp,
                    upstreams: vec!["[fd5e::2]:5432".to_owned()],
                },
                L4Route {
                    port: 5353,
                    proto: L4Proto::Udp,
                    upstreams: vec!["[fd5e::2]:5353".to_owned()],
                },
            ],
            warm_cert_hostnames: BTreeSet::from(["warm.example.com".to_owned()]),
            cert_endpoint_url: Some("http://[fd5e::3]:8080/get_certificate".to_owned()),
            renewal_ratios: BTreeMap::from([("slow.example.com".to_owned(), 0.5)]),
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

    // r[impl infra.proxy.image.config]
    /// Run `caddy validate` in the image over `config`, returning its verdict
    /// and everything it said, prefixed with the image and runtime it ran in.
    ///
    /// Without `SEEDLING_CADDY_IMAGE` this is the pinned published image, which
    /// lags a Containerfile change until the bumped tag is published; naming
    /// it is what tells a stale image apart from an emitter the image rejects.
    ///
    /// `validate` provisions every module, which is what decodes each one's own
    /// fields strictly; it parses the whole document rather than only its outer
    /// shape, so no running container or admin API is needed.
    fn validate_in_image(config: &serde_json::Value) -> (bool, String) {
        let image = std::env::var("SEEDLING_CADDY_IMAGE")
            .unwrap_or_else(|_| crate::system::caddy::startup::CADDY_IMAGE.to_owned());
        let runtime = std::env::var("CONTAINER_RUNTIME").unwrap_or_else(|_| "podman".to_owned());

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("caddy.json"),
            serde_json::to_vec_pretty(config).expect("serialise config"),
        )
        .expect("write config");

        let output = std::process::Command::new(&runtime)
            .args(["run", "--rm", "-v"])
            .arg(format!("{}:/cfg:ro,Z", dir.path().display()))
            .arg(&image)
            .args(["caddy", "validate", "--config", "/cfg/caddy.json"])
            .output()
            .unwrap_or_else(|e| panic!("could not run `{runtime}`: {e}"));
        let said = format!(
            "validated in {image} with {runtime}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), said)
    }

    /// Insert `bogus_field` into every object that carries a `handler` key.
    fn with_unknown_field(config: &mut serde_json::Value) {
        match config {
            serde_json::Value::Object(map) => {
                if map.contains_key("handler") {
                    map.insert("bogus_field".to_owned(), serde_json::json!(1));
                }
                for value in map.values_mut() {
                    with_unknown_field(value);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(with_unknown_field),
            _ => {}
        }
    }

    // r[verify infra.proxy.image.config]
    #[test]
    #[ignore = "needs a container runtime and the proxy image; run by the caddy-config workflow"]
    fn emitted_config_is_accepted_by_the_proxy_image() {
        let emitted = build_caddy_config(&everything_on());

        // A check that cannot fail proves nothing: if `validate` accepted a
        // handler carrying a field no module declares, a pass below would only
        // mean the command ran.
        let mut tampered = emitted.clone();
        with_unknown_field(&mut tampered);
        let (accepted, said) = validate_in_image(&tampered);
        assert!(
            !accepted && said.contains("unknown field"),
            "the image accepted a configuration carrying an undeclared field, so \
             validating against it cannot catch one. {said}"
        );

        let (accepted, said) = validate_in_image(&emitted);
        assert!(
            accepted,
            "the proxy image rejects the configuration the runtime emits, which \
             would drop ingress for every vhost on the host. If this is the \
             pinned image and the Containerfile changed, set SEEDLING_CADDY_IMAGE \
             to an image built from it. {said}"
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
