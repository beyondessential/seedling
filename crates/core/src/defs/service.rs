use std::sync::Weak;

use rhai::{CustomType, EvalAltResult, Map, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{
    Freezable, Holder, Port,
    app::AppDef,
    export::ExportOptions,
    ingress::Ingress,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};
pub use proxy::{
    BalanceSettings, CompressDecl, CompressSettings, DEFAULT_REDIRECT_CODE, Encoding,
    GroupedHeaderOps, HeaderName, HeaderOp, HeaderRules, HeaderSettings, LbPolicy, ProxySettings,
    REDIRECT_OWNED_HEADER, RateLimitDecl, RateLimitScope, RateLimitSettings, RedirectSegment,
    ResolvedBalance, ResolvedCompress, ResolvedRateLimit, ResolvedRouteProxy, RouteRedirect,
    default_content_types, resolve,
};

mod proxy;

// l[impl service.type]
#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub http: Option<HttpServiceDef>,
    /// How the proxy picks among this service's backends. Service-wide
    /// rather than HTTP-only: a balancing policy is a property of a pool of
    /// backends, and non-HTTP ingress traffic has one too.
    // l[impl service.balance]
    pub balance: BalanceSettings,
    pub exported: Option<ExportOptions>,
    // l[impl bsl.resource.description]
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: ResourceName,
    pub def: Holder<ServiceDef>,
    /// Weak back-reference to the owning `AppDef` so that `ingress()` can
    /// register the created `Ingress` into `app_def.resources`.
    pub(super) app_def: Option<Weak<arc_swap::ArcSwap<AppDef>>>,
    pub frozen: bool,
}

impl super::Freezable for Service {
    // l[impl app.resources.context.immutable]
    fn is_frozen(&self) -> bool {
        // Anonymous services use an empty name as a sentinel (see app/service.rs);
        // they remain mutable inside the action that creates them.
        self.frozen || (!self.name.is_empty() && is_in_action_closure())
    }
}

impl Service {
    pub fn new(name: ResourceName) -> Self {
        Self {
            name,
            def: Default::default(),
            app_def: None,
            frozen: false,
        }
    }

    pub(super) fn new_with_app(
        name: ResourceName,
        app_def: Weak<arc_swap::ArcSwap<AppDef>>,
    ) -> Self {
        Self {
            name,
            def: Default::default(),
            app_def: Some(app_def),
            frozen: false,
        }
    }
}

impl CustomType for Service {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Service")
            // l[impl service.port]
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    Ok(ServicePort {
                        service: this.clone().into(),
                        port,
                    })
                },
            )
            // l[impl service.http]
            .with_fn(
                "http",
                |this: &mut Self| -> Result<HttpService, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone().into(),
                        port: Port::from_u16(80),
                    })
                },
            )
            .with_fn(
                "http",
                |this: &mut Self, port: i64| -> Result<HttpService, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone().into(),
                        port,
                    })
                },
            )
            // l[impl service.balance]
            .with_fn(
                "balance",
                |this: &mut Self, config: Map| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().balance = proxy::parse_balance(config)?;
                    Ok(this.clone())
                },
            )
            // l[impl service.exported]
            .with_fn(
                "exported",
                |this: &mut Self| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.as_str().is_empty() {
                        return Err("only named services can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions::default());
                    Ok(this.clone())
                },
            )
            // l[impl service.exported]
            .with_fn(
                "exported",
                |this: &mut Self, options: Map| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.as_str().is_empty() {
                        return Err("only named services can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions::from_rhai_map(options)?);
                    Ok(this.clone())
                },
            )
            .with_fn(
                "ingress",
                |this: &mut Self,
                 hostname: &str,
                 port: i64|
                 -> Result<Ingress, Box<EvalAltResult>> {
                    declare_ingress(this, hostname, port)
                },
            )
            // l[impl bsl.resource.description]
            .with_fn(
                "description",
                |this: &mut Self, desc: &str| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().description = Some(desc.to_owned());
                    Ok(this.clone())
                },
            );
    }
}

/// Register an ingress on `service`. Shared between `Service::ingress`
/// and `HttpService::ingress` so both surfaces produce the same
/// validation, identity, conflict semantics, and resource-tree side
/// effects. The `HttpService` route just picks the underlying
/// `Service` out of its `BoundService` and calls this.
// l[impl ingress.type]
// l[impl ingress.conflicts]
pub(crate) fn declare_ingress(
    service: &mut Service,
    hostname: &str,
    port: i64,
) -> Result<Ingress, Box<EvalAltResult>> {
    service.ensure_unfrozen()?;
    let port = Port::new(port)?;
    validate_hostname(hostname)?;
    let ingress = Ingress::new(service.clone(), hostname.into(), port);
    if let Some(arc) = service.app_def.as_ref().and_then(Weak::upgrade) {
        let id = ResourceId {
            kind: ResourceKind::Ingress,
            name: ingress.name.clone(),
        };
        // Conflict check: a prior ingress with the same (hostname, port)
        // keyed by the same resource name in this app must not be
        // overwritten. Throwing lets the script catch + handle it;
        // silently overriding would erase a previous declaration.
        if arc.load().resources.contains_key(&id) {
            return Err(format!(
                "ingress conflict: ({hostname}, {}) is already declared in this app",
                port.get()
            )
            .into());
        }
        let ingress_clone = ingress.clone();
        arc.rcu(|d| {
            let mut d = (**d).clone();
            d.resources
                .insert(id.clone(), Resource::Ingress(ingress_clone.clone()));
            d
        });
    }
    Ok(ingress)
}

// Reference-to-a-service carried by pod bindings. It's either an app's own
// `Service` (declared in the same script) or an `ExternalService` slot whose
// concrete target is supplied by the operator at runtime via
// `external_service_mappings`. Downstream consumers should normally go
// through [`BoundService::name`] and [`BoundService::is_external`] rather
// than matching the variants inline.
#[derive(Debug, Clone)]
pub enum BoundService {
    App(Service),
    External(ExternalService),
}

impl BoundService {
    pub fn name(&self) -> &ResourceName {
        match self {
            Self::App(s) => &s.name,
            Self::External(e) => &e.name,
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(self, Self::External(_))
    }
}

impl BoundService {
    /// Mutate the service-wide balancing settings behind this view.
    fn with_balance<R>(
        &mut self,
        f: impl FnOnce(&mut BalanceSettings) -> R,
    ) -> Result<R, Box<EvalAltResult>> {
        match self {
            Self::App(s) => {
                s.ensure_unfrozen()?;
                let mut def = s.def.lock();
                Ok(f(&mut def.balance))
            }
            Self::External(e) => {
                let mut def = e.def.lock();
                Ok(f(&mut def.balance))
            }
        }
    }

    /// Record that the service is served through `prefix`, without disturbing
    /// any settings already declared on it.
    ///
    /// Deliberately outside the frozen check that guards the setting
    /// builders: `route()` has always been callable on a service captured by
    /// an action closure, and noting that a prefix exists is not a
    /// configuration change an action should be barred from making.
    fn register_route(&mut self, prefix: &str) {
        let record = |def: &mut Option<HttpServiceDef>| {
            def.get_or_insert_default()
                .routes
                .entry(prefix.to_owned())
                .or_default();
        };
        match self {
            Self::App(s) => record(&mut s.def.lock().http),
            Self::External(e) => record(&mut e.def.lock().http),
        }
    }

    /// Whether `prefix` is declared as a redirect on this service.
    // l[impl service.http.route.redirect]
    fn is_redirect(&self, prefix: &str) -> bool {
        let declared = |def: &Option<HttpServiceDef>| {
            def.as_ref().is_some_and(|h| h.redirect(prefix).is_some())
        };
        match self {
            Self::App(s) => declared(&s.def.lock().http),
            Self::External(e) => declared(&e.def.lock().http),
        }
    }

    /// Record that a pod binds `prefix`, refusing one already declared as a
    /// redirect.
    ///
    /// Outside the frozen check for the same reason [`register_route`] is:
    /// noting that a prefix is spoken for is not a configuration change.
    ///
    /// [`register_route`]: Self::register_route
    // l[impl service.http.route.redirect]
    fn record_binding(&mut self, prefix: &str) -> Result<(), Box<EvalAltResult>> {
        let service = self.name().to_string();
        let record = |def: &mut Option<HttpServiceDef>| -> Result<(), Box<EvalAltResult>> {
            let decl = def
                .get_or_insert_default()
                .routes
                .entry(prefix.to_owned())
                .or_default();
            match &mut decl.kind {
                RouteKind::Proxied { bound } => {
                    *bound = true;
                    Ok(())
                }
                RouteKind::Redirect(_) => Err(refuse_prefix_serves_both(prefix, &service)),
            }
        };
        match self {
            Self::App(s) => record(&mut s.def.lock().http),
            Self::External(e) => record(&mut e.def.lock().http),
        }
    }

    /// Mutate the `HttpServiceDef` of whichever service backs this view,
    /// creating it if the app has not called `.http()` yet.
    fn with_http_def<R>(
        &mut self,
        f: impl FnOnce(&mut HttpServiceDef) -> R,
    ) -> Result<R, Box<EvalAltResult>> {
        match self {
            Self::App(s) => {
                s.ensure_unfrozen()?;
                let mut def = s.def.lock();
                Ok(f(def.http.get_or_insert_default()))
            }
            Self::External(e) => {
                let mut def = e.def.lock();
                Ok(f(def.http.get_or_insert_default()))
            }
        }
    }
}

impl From<Service> for BoundService {
    fn from(s: Service) -> Self {
        Self::App(s)
    }
}

impl From<ExternalService> for BoundService {
    fn from(e: ExternalService) -> Self {
        Self::External(e)
    }
}

// l[impl service.port]
#[derive(Debug, Clone)]
pub struct ServicePort {
    pub service: BoundService,
    pub port: Port,
}

impl CustomType for ServicePort {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("ServicePort");
    }
}

// l[impl service.http]
#[derive(Debug, Default, Clone)]
pub struct HttpServiceDef {
    /// Compression for every route of this service that does not set its own.
    /// Compression is HTTP-shaped, so unlike balancing it lives here.
    pub compress: Option<CompressDecl>,
    /// The limit every route of this service that does not set its own is
    /// held to. Rate limiting is applied by the HTTP proxy, so like
    /// compression it lives here rather than on the Service.
    // l[impl service.http.rate-limit]
    pub rate_limit: Option<RateLimitDecl>,
    /// The header operations every route of this service inherits, each of
    /// which a route may override for the headers it names.
    // l[impl service.http.headers]
    pub headers: HeaderSettings,
    /// Every URL prefix this service is served through, with what the app
    /// declared on it. An entry carrying nothing is still a route:
    /// registering the prefix is how the service knows which routes exist
    /// without having to walk the pods that bind them.
    pub routes: std::collections::BTreeMap<String, RouteDecl>,
}

/// What one declared prefix is, and what was declared on it.
#[derive(Debug, Default, Clone)]
pub struct RouteDecl {
    /// What the app declared for this prefix. A redirect is refused most of
    /// these, having nothing for them to act on, but still carries the
    /// response header operations of a response it serves.
    pub settings: ProxySettings,
    pub kind: RouteKind,
}

/// Whether a prefix is served by a pod or answered with a redirect.
///
/// One value rather than a second map keyed the same way: "a prefix is either
/// redirected or proxied" is then something the type cannot express otherwise,
/// rather than an invariant each entry point has to defend for itself. The
/// binding sits in the proxied arm for the same reason — a bound prefix
/// cannot become a redirect by construction, rather than by check.
// l[impl service.http.route.redirect]
#[derive(Debug, Clone)]
pub enum RouteKind {
    /// Served by whichever pods bind the prefix. `bound` records that at
    /// least one `http(pod_port, route)` named it, which is what makes a
    /// redirect on the same prefix a contradiction rather than a preference.
    Proxied {
        bound: bool,
    },
    Redirect(RouteRedirect),
}

impl Default for RouteKind {
    fn default() -> Self {
        Self::Proxied { bound: false }
    }
}

/// The canonical spelling of a route prefix.
///
/// A trailing slash carries no meaning in a prefix — `/v1/login/` claims the
/// same requests as `/v1/login` — so it is dropped once, here, where the
/// prefix becomes a map key. Normalising at emission instead would let two
/// spellings pass the "either redirected or proxied" check as different
/// prefixes and then emit two routes claiming the same requests.
// l[impl service.http.route]
pub fn normalise_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_end_matches('/');
    if trimmed.is_empty() {
        // Every spelling of the root is the root, `//` among them: left as it
        // was written it would trim to nothing at emission and match every
        // request on the hostname.
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Refuse a prefix asked to be both redirected and proxied.
///
/// One wording for both declaration orders: the clash is the same fact
/// whichever of the two was written first.
// l[impl service.http.route.redirect]
fn refuse_prefix_serves_both(prefix: &str, service: &str) -> Box<EvalAltResult> {
    format!(
        "`{prefix}` on service `{service}` is asked to be both a redirect and a prefix a pod \
         is bound to; a prefix is either redirected or proxied"
    )
    .into()
}

impl HttpServiceDef {
    /// What the app declared for one prefix, whatever serves it.
    pub fn settings(&self, prefix: &str) -> Option<&ProxySettings> {
        self.routes.get(prefix).map(|r| &r.settings)
    }

    /// The redirect declared on one prefix, if that is what it is.
    // l[impl service.http.route.redirect]
    pub fn redirect(&self, prefix: &str) -> Option<&RouteRedirect> {
        match &self.routes.get(prefix)?.kind {
            RouteKind::Redirect(redirect) => Some(redirect),
            RouteKind::Proxied { .. } => None,
        }
    }

    /// Every prefix this service answers with a redirect.
    // l[impl service.http.route.redirect]
    pub fn redirects(&self) -> impl Iterator<Item = (&String, &RouteRedirect)> {
        self.routes
            .iter()
            .filter_map(|(prefix, decl)| match &decl.kind {
                RouteKind::Redirect(redirect) => Some((prefix, redirect)),
                RouteKind::Proxied { .. } => None,
            })
    }
}

impl ServiceDef {
    /// The service-level view resolution works against, gathering balancing
    /// from the service and compression from its HTTP surface.
    pub fn proxy_settings(&self) -> ProxySettings {
        proxy::service_settings(self.http.as_ref(), &self.balance)
    }
}

#[derive(Debug, Clone)]
pub struct HttpService {
    pub service: BoundService,
    pub port: Port,
}

impl CustomType for HttpService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("HttpService")
            // l[impl service.http.route]
            .with_fn(
                "route",
                |this: &mut Self, prefix: &str| -> Result<HttpServiceRoute, Box<EvalAltResult>> {
                    if prefix.is_empty() || !prefix.starts_with('/') {
                        return Err(
                            "route prefix must be a non-empty string starting with '/'".into()
                        );
                    }
                    // Canonical from here on: every map key, clash check and
                    // matcher downstream reads this spelling, so they agree on
                    // what one prefix is.
                    let prefix = normalise_prefix(prefix);
                    this.service.register_route(&prefix);
                    Ok(HttpServiceRoute {
                        http: this.clone(),
                        prefix,
                    })
                },
            )
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    let port = Port::new(port)?;
                    Ok(ServicePort {
                        service: this.service.clone(),
                        port,
                    })
                },
            )
            // l[impl service.http.compress]
            .with_fn(
                "compress",
                |this: &mut Self, enabled: bool| -> Result<Self, Box<EvalAltResult>> {
                    let decl = compress_decl(enabled);
                    this.service.with_http_def(|d| d.compress = Some(decl))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.compress]
            .with_fn(
                "compress",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_compress(config)?;
                    this.service
                        .with_http_def(|d| d.compress = Some(CompressDecl::Enabled(settings)))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.balance]
            // A pass-through to the backing Service, so that declaring the
            // policy mid-chain reads naturally without balancing becoming an
            // HTTP-only concept.
            .with_fn(
                "balance",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_balance(config)?;
                    this.service.with_balance(|b| *b = settings)?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.rate-limit]
            .with_fn(
                "rate_limit",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_rate_limit(config)?;
                    this.service
                        .with_http_def(|d| d.rate_limit = Some(RateLimitDecl::Enabled(settings)))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.rate-limit]
            .with_fn(
                "rate_limit",
                |this: &mut Self, enabled: bool| -> Result<Self, Box<EvalAltResult>> {
                    let decl = rate_limit_decl(enabled)?;
                    this.service.with_http_def(|d| d.rate_limit = Some(decl))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.headers]
            .with_fn(
                "headers",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_headers(config)?;
                    this.service
                        .with_http_def(|d| d.headers.layer_over(settings))?;
                    Ok(this.clone())
                },
            )
            // l[impl ingress.type]
            // Pass-through to the underlying Service: declaring an
            // ingress on `svc.http()` is just a chaining-friendly way
            // of declaring it on `svc`. The resulting Ingress is bound
            // to the Service, not to the HttpService — HttpService is
            // a per-call view, not a separate resource.
            .with_fn(
                "ingress",
                |this: &mut Self,
                 hostname: &str,
                 port: i64|
                 -> Result<Ingress, Box<EvalAltResult>> {
                    let BoundService::App(ref mut svc) = this.service else {
                        return Err("ingress() is only valid on app-declared services; \
                             external services cannot have ingresses"
                            .into());
                    };
                    declare_ingress(svc, hostname, port)
                },
            );
    }
}

impl HttpService {
    /// The `/` route this service is served through when a pod binds the
    /// service itself rather than one of its prefixes.
    pub(super) fn root_route(mut self) -> HttpServiceRoute {
        self.service.register_route("/");
        HttpServiceRoute {
            http: self,
            prefix: "/".into(),
        }
    }
}

// l[impl service.http.route]
#[derive(Debug, Clone)]
pub struct HttpServiceRoute {
    pub http: HttpService,
    pub prefix: String,
}

impl CustomType for HttpServiceRoute {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("HttpServiceRoute")
            // l[impl service.http.compress]
            .with_fn(
                "compress",
                |this: &mut Self, enabled: bool| -> Result<Self, Box<EvalAltResult>> {
                    let decl = compress_decl(enabled);
                    this.with_route_settings(Some("compress"), |s| s.compress = Some(decl))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.compress]
            .with_fn(
                "compress",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_compress(config)?;
                    this.with_route_settings(Some("compress"), |s| {
                        s.compress = Some(CompressDecl::Enabled(settings))
                    })?;
                    Ok(this.clone())
                },
            )
            // l[impl service.balance]
            .with_fn(
                "balance",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_balance(config)?;
                    this.with_route_settings(Some("balance"), |s| s.balance = settings)?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.rate-limit]
            .with_fn(
                "rate_limit",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_rate_limit(config)?;
                    this.with_route_settings(Some("rate_limit"), |s| {
                        s.rate_limit = Some(RateLimitDecl::Enabled(settings))
                    })?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.rate-limit]
            .with_fn(
                "rate_limit",
                |this: &mut Self, enabled: bool| -> Result<Self, Box<EvalAltResult>> {
                    let decl = rate_limit_decl(enabled)?;
                    this.with_route_settings(Some("rate_limit"), |s| s.rate_limit = Some(decl))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.headers]
            .with_fn(
                "headers",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let settings = proxy::parse_headers(config)?;
                    // l[impl service.http.route.redirect]
                    // A redirect sends no request onward, and computes the one
                    // response header an operation here could displace.
                    if this.http.service.is_redirect(&this.prefix) {
                        if !settings.request.is_empty() {
                            return Err(proxy::refuse_request_headers_on_redirect(&this.prefix));
                        }
                        if settings.response.names(REDIRECT_OWNED_HEADER) {
                            return Err(proxy::refuse_location_on_redirect(&this.prefix));
                        }
                    }
                    this.with_route_settings(None, |s| s.headers.layer_over(settings))?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.route.redirect]
            .with_fn(
                "redirect",
                |this: &mut Self, to: &str| -> Result<Self, Box<EvalAltResult>> {
                    let redirect = proxy::parse_redirect_positional(to, None)?;
                    this.declare_redirect(redirect)?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.route.redirect]
            .with_fn(
                "redirect",
                |this: &mut Self, to: &str, code: i64| -> Result<Self, Box<EvalAltResult>> {
                    let redirect = proxy::parse_redirect_positional(to, Some(code))?;
                    this.declare_redirect(redirect)?;
                    Ok(this.clone())
                },
            )
            // l[impl service.http.route.redirect]
            .with_fn(
                "redirect",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    let redirect = proxy::parse_redirect_map(config)?;
                    this.declare_redirect(redirect)?;
                    Ok(this.clone())
                },
            );
    }
}

/// `rate_limit(false)` switches limiting off. `rate_limit(true)` has nothing to
/// mean — there is no default limit to enable — so it is refused rather than
/// silently doing nothing.
// l[impl service.http.rate-limit]
fn rate_limit_decl(enabled: bool) -> Result<RateLimitDecl, Box<EvalAltResult>> {
    if enabled {
        return Err(
            "rate_limit(true) is not valid: a rate limit has no default, \
             so it must be declared as a map with `max_events` and `window`"
                .into(),
        );
    }
    Ok(RateLimitDecl::Disabled)
}

/// `compress(true)` means the defaults, which is an enabled declaration that
/// names no fields; `compress(false)` switches it off.
fn compress_decl(enabled: bool) -> CompressDecl {
    if enabled {
        CompressDecl::Enabled(CompressSettings::default())
    } else {
        CompressDecl::Disabled
    }
}

impl HttpServiceRoute {
    /// Record that a pod binds this route's prefix.
    // l[impl service.http.route.redirect]
    pub(super) fn record_binding(&mut self) -> Result<(), Box<EvalAltResult>> {
        let prefix = self.prefix.clone();
        self.http.service.record_binding(&prefix)
    }

    /// Record a redirect on this route, refusing the prefixes and the
    /// neighbouring settings a redirect cannot live alongside.
    ///
    /// A second `redirect()` replaces the first: the route is still a
    /// redirect either way, so there is nothing for the two to contradict
    /// each other about.
    // l[impl service.http.route.redirect]
    fn declare_redirect(&mut self, redirect: RouteRedirect) -> Result<(), Box<EvalAltResult>> {
        if self.prefix == "/" {
            return Err(
                "a redirect on `/` answers for the whole hostname, which is a site \
                 ingress redirect attachment for an operator to make rather than an app"
                    .into(),
            );
        }

        let prefix = self.prefix.clone();
        let service = self.http.service.name().to_string();
        self.http.service.with_http_def(|d| {
            let decl = d.routes.entry(prefix.clone()).or_default();
            if let RouteKind::Proxied { bound: true } = decl.kind {
                return Err(refuse_prefix_serves_both(&prefix, &service));
            }
            // The one enumeration of what a redirect leaves nothing to act
            // on, for the declaration order that wrote the setting first.
            // `with_route_settings` refuses the other order.
            proxy::refuse_settings_for_redirect(&prefix, &decl.settings)?;
            decl.kind = RouteKind::Redirect(redirect);
            Ok(())
        })?
    }

    /// Apply a route-level setting, refusing it on a route that is a redirect.
    ///
    /// Every route setting goes through here, so `setting` naming one is what
    /// refuses it: a setting added later cannot reach a redirect route without
    /// its author having decided which it is. `None` is for the settings a
    /// redirect does carry — header operations, which a response it serves can
    /// bear — and those check themselves for what a redirect cannot take.
    // l[impl service.http.route.redirect]
    fn with_route_settings<R>(
        &mut self,
        setting: Option<&str>,
        f: impl FnOnce(&mut ProxySettings) -> R,
    ) -> Result<R, Box<EvalAltResult>> {
        let prefix = self.prefix.clone();
        self.http.service.with_http_def(|d| {
            let decl = d.routes.entry(prefix.clone()).or_default();
            if let (RouteKind::Redirect(_), Some(setting)) = (&decl.kind, setting) {
                return Err(proxy::refuse_on_redirect(&prefix, setting));
            }
            Ok(f(&mut decl.settings))
        })?
    }
}

// l[impl service.external]
#[derive(Debug, Default, Clone)]
pub struct ExternalServiceDef {
    // l[impl bsl.resource.description]
    pub description: Option<String>,
    pub http: Option<HttpServiceDef>,
    // l[impl service.balance]
    pub balance: BalanceSettings,
}

impl ExternalServiceDef {
    /// The service-level view resolution works against. Mirrors
    /// [`ServiceDef::proxy_settings`]: an external-service slot carries the
    /// same HTTP surface, so a setting added to one belongs to both, and
    /// gathering them in one place per def is what stops the next one being
    /// added to only half of them.
    pub fn proxy_settings(&self) -> ProxySettings {
        proxy::service_settings(self.http.as_ref(), &self.balance)
    }
}

// l[impl service.external]
#[derive(Debug, Clone)]
pub struct ExternalService {
    pub name: ResourceName,
    pub def: Holder<ExternalServiceDef>,
}

impl CustomType for ExternalService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ExternalService")
            // l[impl service.external]
            // An external-service slot exposes the same port/http surface a
            // native service does, but the resulting ServicePort/HttpService
            // carries `BoundService::External`, so the reconciler knows to
            // resolve the backend via `external_service_mappings`.
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    let port = Port::new(port)?;
                    Ok(ServicePort {
                        service: this.clone().into(),
                        port,
                    })
                },
            )
            .with_fn(
                "http",
                |this: &mut Self| -> Result<HttpService, Box<EvalAltResult>> {
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone().into(),
                        port: Port::from_u16(80),
                    })
                },
            )
            .with_fn(
                "http",
                |this: &mut Self, port: i64| -> Result<HttpService, Box<EvalAltResult>> {
                    let port = Port::new(port)?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone().into(),
                        port,
                    })
                },
            )
            // l[impl service.balance]
            .with_fn(
                "balance",
                |this: &mut Self, config: Map| -> Result<Self, Box<EvalAltResult>> {
                    this.def.lock().balance = proxy::parse_balance(config)?;
                    Ok(this.clone())
                },
            )
            // l[impl bsl.resource.description]
            .with_fn("description", |this: &mut Self, desc: &str| -> Self {
                this.def.lock().description = Some(desc.to_owned());
                this.clone()
            });
    }
}

// l[impl ingress.hostname]
fn validate_hostname(hostname: &str) -> Result<(), Box<EvalAltResult>> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(format!("hostname must be 1–253 characters, got {}", hostname.len()).into());
    }

    if hostname.contains('*') {
        return Err("wildcard hostnames are not permitted".into());
    }

    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!(
                "each hostname label must be 1–63 characters, got '{}' ({})",
                label,
                label.len()
            )
            .into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(
                format!("hostname label must not start or end with a hyphen: '{label}'").into(),
            );
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("hostname label contains invalid characters: '{label}'").into());
        }
    }

    Ok(())
}
