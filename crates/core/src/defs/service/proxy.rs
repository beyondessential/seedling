//! Compression and balancing settings declared on an HTTP Service and on its
//! individual routes.
//!
//! Declared settings keep every field optional so that resolution can tell
//! "the app asked for this value" from "the app said nothing", which is what
//! [`l[service.http.proxy-settings.resolution]`] needs to fall back field by
//! field. Resolved settings have no optionality left except compression being
//! switched off entirely.

use std::{cmp::Ordering, collections::BTreeMap};

use rhai::{Dynamic, EvalAltResult, Map};

/// Caddy's own defaults, read from the source of the pinned proxy image so the
/// emitter can leave a field out whenever it still holds its default.
pub const DEFAULT_MINIMUM_LENGTH: u64 = 512;

/// Bounds on a declared rate limit.
///
/// Sanity ceilings, not tuning. The proxy holds a ring of `max_events`
/// timestamps per distinct client and reclaims it once that client's newest
/// request has aged past the window, so a declaration sets both how large each
/// ring is and how long it survives — in a process every app on the host
/// shares. Reclamation itself is automatic: the pinned module sweeps every
/// minute by default, with the sweeper started unconditionally, so nothing
/// needs to be emitted to switch it on.
///
/// The floor is a plausibility bound like the rest: a window is emitted as
/// whole nanoseconds and only rounds away below about half a nanosecond, far
/// under anything here.
pub const MIN_WINDOW_SECS: f64 = 0.001;
pub const MAX_WINDOW_SECS: f64 = 3_600.0;
pub const MAX_MAX_EVENTS: u64 = 1_000;
pub const DEFAULT_TRY_DURATION_SECS: f64 = 5.0;
pub const DEFAULT_INTERVAL_SECS: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Encoding {
    Zstd,
    Gzip,
}

impl Encoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Gzip => "gzip",
        }
    }

    fn parse(s: &str) -> Result<Self, Box<EvalAltResult>> {
        match s {
            "zstd" => Ok(Self::Zstd),
            "gzip" => Ok(Self::Gzip),
            other => Err(format!(
                "compress `encodings` entries must be \"zstd\" or \"gzip\"; got '{other}'"
            )
            .into()),
        }
    }
}

/// zstd first, gzip second: the order Caddy itself prefers.
pub fn default_encodings() -> Vec<Encoding> {
    vec![Encoding::Zstd, Encoding::Gzip]
}

/// The proxy's own default matcher: markup, stylesheets, scripts, JSON and
/// other structured text, SVG, icons and fonts. Held here so the resolved
/// settings can be reported in full, while the emitter leaves the matcher out
/// whenever the app has not replaced this set.
pub fn default_content_types() -> Vec<String> {
    [
        "application/atom+xml*",
        "application/eot*",
        "application/font*",
        "application/geo+json*",
        "application/graphql+json*",
        "application/graphql-response+json*",
        "application/javascript*",
        "application/json*",
        "application/ld+json*",
        "application/manifest+json*",
        "application/opentype*",
        "application/otf*",
        "application/rss+xml*",
        "application/truetype*",
        "application/ttf*",
        "application/vnd.api+json*",
        "application/vnd.ms-fontobject*",
        "application/wasm*",
        "application/x-httpd-cgi*",
        "application/x-javascript*",
        "application/x-opentype*",
        "application/x-otf*",
        "application/x-perl*",
        "application/x-protobuf*",
        "application/x-ttf*",
        "application/xhtml+xml*",
        "application/xml*",
        "font/ttf*",
        "font/otf*",
        "image/svg+xml*",
        "image/vnd.microsoft.icon*",
        "image/x-icon*",
        "multipart/bag*",
        "multipart/mixed*",
        "text/*",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LbPolicy {
    RoundRobin,
    LeastConn,
    Random,
    First,
}

impl LbPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::LeastConn => "least_conn",
            Self::Random => "random",
            Self::First => "first",
        }
    }

    fn parse(s: &str) -> Result<Self, Box<EvalAltResult>> {
        match s {
            "round_robin" => Ok(Self::RoundRobin),
            "least_conn" => Ok(Self::LeastConn),
            "random" => Ok(Self::Random),
            "first" => Ok(Self::First),
            other => Err(format!(
                "balance `policy` must be one of \"round_robin\", \"least_conn\", \
                 \"random\", \"first\"; got '{other}'"
            )
            .into()),
        }
    }
}

/// Compression fields the app named. A field left `None` is one the app did
/// not mention at this level, not one it set to a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompressSettings {
    pub encodings: Option<Vec<Encoding>>,
    pub minimum_length: Option<u64>,
    pub content_types: Option<Vec<String>>,
}

/// Compression is tri-state at each level: unmentioned, switched off, or
/// switched on carrying whichever fields were named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressDecl {
    Disabled,
    Enabled(CompressSettings),
}

/// A declared rate limit. Both fields are required, so neither is optional:
/// a limit means nothing without a count and a window to count over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitSettings {
    pub max_events: u64,
    pub window_secs: f64,
}

/// Rate limiting is tri-state at each level: unmentioned, switched off, or
/// switched on carrying the limit. Unlike compression it has no default, so
/// "unmentioned everywhere" means no limiting rather than a default limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateLimitDecl {
    Disabled,
    Enabled(RateLimitSettings),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BalanceSettings {
    pub policy: Option<LbPolicy>,
    pub try_duration_secs: Option<f64>,
    pub interval_secs: Option<f64>,
}

/// What one level (a service, or one of its routes) declared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxySettings {
    pub compress: Option<CompressDecl>,
    pub balance: BalanceSettings,
    pub rate_limit: Option<RateLimitDecl>,
    /// Header operations declared at this level. Empty is the whole of "this
    /// level said nothing", so there is no optionality to carry: resolution
    /// layers one map over the other rather than picking between them.
    pub headers: HeaderSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCompress {
    pub encodings: Vec<Encoding>,
    pub minimum_length: u64,
    /// `None` means the proxy's own default set of text-like content types,
    /// which the emitter expresses by leaving the matcher out.
    pub content_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBalance {
    pub policy: LbPolicy,
    pub try_duration_secs: f64,
    pub interval_secs: f64,
}

/// The settings actually in force on one route. `compress` is `None` when
/// compression is off for that route, `rate_limit` when the route is not
/// rate limited.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRouteProxy {
    pub compress: Option<ResolvedCompress>,
    pub balance: ResolvedBalance,
    pub rate_limit: Option<ResolvedRateLimit>,
    pub headers: HeaderSettings,
}

/// Which level declared the limit in force.
///
/// This decides the budget's identity, not just its provenance: a limit
/// declared on the service is one budget shared by every route inheriting it,
/// where the same settings declared on a route are that route's own budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitScope {
    Service,
    Route,
}

/// The limit in force on a route, and the level that declared it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedRateLimit {
    pub settings: RateLimitSettings,
    pub scope: RateLimitScope,
}

// ---------------------------------------------------------------------------
// Header manipulation
// ---------------------------------------------------------------------------

/// A header field name.
///
/// Holds the name as the app spelled it, so `app.describe` reads back what was
/// written, but compares and orders case-insensitively because HTTP field
/// names are. Keeping that in the type is what stops a route's
/// `cache-control` from being treated as a different header than a service's
/// `Cache-Control` and silently failing to override it.
// l[impl service.http.headers.fields]
#[derive(Debug, Clone)]
pub struct HeaderName(String);

impl HeaderName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn folded(&self) -> impl Iterator<Item = u8> + '_ {
        self.0.bytes().map(|b| b.to_ascii_lowercase())
    }

    pub fn parse(name: &str) -> Result<Self, Box<EvalAltResult>> {
        if name.is_empty() {
            return Err("a header name must not be empty".into());
        }
        // RFC 9110 token characters. A name outside them cannot go on the wire
        // at all, so it is refused where the error still names the script that
        // wrote it rather than emitted for the proxy to reject at load.
        if let Some(c) = name.chars().find(|c| !is_tchar(*c)) {
            return Err(format!(
                "header name `{name}` contains '{c}', which is not valid in an HTTP field name"
            )
            .into());
        }
        if let Some(what) = proxy_owned(name) {
            return Err(format!(
                "`{name}` describes {what} rather than the message, and belongs to the proxy, \
                 which holds the connection to the client and the connection to the pod as two \
                 separate things; it cannot be set by an app"
            )
            .into());
        }
        Ok(Self(name.to_owned()))
    }
}

/// The headers an app must not touch, and what each describes.
///
/// Setting one corrupts the exchange rather than shaping it. Keep-alive, the
/// one an app would otherwise legitimately reach for, is served without being
/// asked for — see `r[ingress.persistent-connections]`.
fn proxy_owned(name: &str) -> Option<&'static str> {
    const CONNECTION: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ];
    if CONNECTION.iter().any(|c| name.eq_ignore_ascii_case(c)) {
        return Some("the connection itself");
    }
    if name.eq_ignore_ascii_case("content-length") {
        return Some("the message's framing");
    }
    None
}

fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

impl PartialEq for HeaderName {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for HeaderName {}

impl PartialOrd for HeaderName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeaderName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.folded().cmp(other.folded())
    }
}

/// What happens to one header.
///
/// A name carries exactly one of these per direction once resolved, which is
/// why declaring a second for the same name is refused rather than resolved by
/// some order of application.
// l[impl service.http.headers.fields]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderOp {
    /// Set the header to these values, discarding whatever the message carried.
    Replace(Vec<String>),
    /// Add these values, keeping whatever the message carried.
    Add(Vec<String>),
    /// Discard the header entirely.
    Remove,
}

/// The operations declared in one direction, keyed by header name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderRules(pub BTreeMap<HeaderName, HeaderOp>);

impl HeaderRules {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The operations grouped by which one they are, which is how both the
    /// proxy and `app.describe` take them.
    ///
    /// One body for both: a header reported under one operation and emitted
    /// under another would make the description a description of something
    /// else. Each name appears in exactly one group, because resolution leaves
    /// it carrying exactly one operation.
    pub fn grouped(&self) -> GroupedHeaderOps {
        let mut out = GroupedHeaderOps::default();
        for (name, op) in &self.0 {
            let name = name.as_str().to_owned();
            match op {
                HeaderOp::Replace(values) => {
                    out.replace.insert(name, values.clone());
                }
                HeaderOp::Add(values) => {
                    out.add.insert(name, values.clone());
                }
                HeaderOp::Remove => out.remove.push(name),
            }
        }
        out
    }
}

/// [`HeaderRules`] grouped by operation.
///
/// Names keep the spelling the app used, and stay in the case-insensitive
/// order the rules are keyed by, so two routes declaring the same headers
/// report and emit them identically however each spelled them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupedHeaderOps {
    pub replace: BTreeMap<String, Vec<String>>,
    pub add: BTreeMap<String, Vec<String>>,
    pub remove: Vec<String>,
}

/// Header operations at one level, in both directions.
// l[impl service.http.headers]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderSettings {
    pub request: HeaderRules,
    pub response: HeaderRules,
}

impl HeaderSettings {
    pub fn is_empty(&self) -> bool {
        self.request.is_empty() && self.response.is_empty()
    }
}

impl Default for ResolvedRouteProxy {
    fn default() -> Self {
        resolve(&ProxySettings::default(), None)
    }
}

/// The service-level view resolution works against, gathered from the two
/// places a service keeps them: compression and rate limiting on its HTTP
/// surface, balancing on the service itself.
///
/// One body, so a setting added here reaches every kind of service that has
/// one rather than only the kind whose method was edited.
pub fn service_settings(
    http: Option<&crate::defs::service::HttpServiceDef>,
    balance: &BalanceSettings,
) -> ProxySettings {
    ProxySettings {
        compress: http.and_then(|h| h.compress.clone()),
        balance: balance.clone(),
        rate_limit: http.and_then(|h| h.rate_limit),
        headers: http.map(|h| h.headers.clone()).unwrap_or_default(),
    }
}

// l[impl service.http.proxy-settings.resolution]
/// Resolve one route's settings.
///
/// Compression and balancing resolve field by field: each is taken from the
/// route if the route named it, otherwise from the service if the service
/// named it, otherwise from the field's default. Rate limiting resolves as a
/// whole unit and has no default — see [`resolve_rate_limit`].
///
/// Passing `None` for `route` resolves the service's own values. That is not
/// what the synthesised `/` route takes: a service may declare settings for
/// `/` without a pod bound to it yet, so its caller resolves against the
/// declared `/` route where there is one.
pub fn resolve(service: &ProxySettings, route: Option<&ProxySettings>) -> ResolvedRouteProxy {
    let route_balance = route.map(|r| &r.balance);
    let route_compress = route.and_then(|r| r.compress.as_ref());
    let route_rate_limit = route.and_then(|r| r.rate_limit.as_ref());

    ResolvedRouteProxy {
        compress: resolve_compress(service.compress.as_ref(), route_compress),
        balance: resolve_balance(&service.balance, route_balance),
        rate_limit: resolve_rate_limit(service.rate_limit.as_ref(), route_rate_limit),
        headers: resolve_headers(&service.headers, route.map(|r| &r.headers)),
    }
}

/// Header operations resolve per name, independently within each direction: a
/// header the route names takes the route's operation, and one only the
/// service names takes the service's. A route therefore adds to the headers
/// the service declared and overrides only those it names, without restating
/// the rest.
///
/// Each name ends up carrying exactly one operation, which is what makes the
/// order operations are applied in unobservable.
fn resolve_headers(service: &HeaderSettings, route: Option<&HeaderSettings>) -> HeaderSettings {
    let Some(route) = route else {
        return service.clone();
    };
    HeaderSettings {
        request: layer_headers(&service.request, &route.request),
        response: layer_headers(&service.response, &route.response),
    }
}

fn layer_headers(service: &HeaderRules, route: &HeaderRules) -> HeaderRules {
    let mut out = service.clone();
    for (name, op) in &route.0 {
        // Removed before inserting: inserting over an equal key keeps the key
        // already present, which would report the service's spelling of a
        // header whose operation came from the route.
        out.0.remove(name);
        out.0.insert(name.clone(), op.clone());
    }
    out
}

/// Rate limiting resolves as a whole rather than field by field: a route
/// declaring a limit replaces the service's outright, rather than merging with
/// it, because a count and a window only mean anything together. A route that
/// switched limiting off is not limited even where the service declared one.
fn resolve_rate_limit(
    service: Option<&RateLimitDecl>,
    route: Option<&RateLimitDecl>,
) -> Option<ResolvedRateLimit> {
    match (route, service) {
        (Some(RateLimitDecl::Enabled(s)), _) => Some(ResolvedRateLimit {
            settings: *s,
            scope: RateLimitScope::Route,
        }),
        (Some(RateLimitDecl::Disabled), _) => None,
        (None, Some(RateLimitDecl::Enabled(s))) => Some(ResolvedRateLimit {
            settings: *s,
            scope: RateLimitScope::Service,
        }),
        (None, Some(RateLimitDecl::Disabled) | None) => None,
    }
}

fn resolve_compress(
    service: Option<&CompressDecl>,
    route: Option<&CompressDecl>,
) -> Option<ResolvedCompress> {
    // The nearest level that mentioned compression at all decides whether it
    // happens; absent any mention it happens, which is what makes compression
    // the default rather than something every app must opt into.
    let enabled = match route.or(service) {
        Some(CompressDecl::Disabled) => false,
        Some(CompressDecl::Enabled(_)) | None => true,
    };
    if !enabled {
        return None;
    }

    // A level that switched compression off contributes no field values, so
    // an enabling route over a disabling service falls back to the defaults.
    let named = |decl: Option<&CompressDecl>| match decl {
        Some(CompressDecl::Enabled(s)) => Some(s.clone()),
        _ => None,
    };
    let route = named(route);
    let service = named(service);

    Some(ResolvedCompress {
        encodings: route
            .as_ref()
            .and_then(|s| s.encodings.clone())
            .or_else(|| service.as_ref().and_then(|s| s.encodings.clone()))
            .unwrap_or_else(default_encodings),
        minimum_length: route
            .as_ref()
            .and_then(|s| s.minimum_length)
            .or_else(|| service.as_ref().and_then(|s| s.minimum_length))
            .unwrap_or(DEFAULT_MINIMUM_LENGTH),
        content_types: route
            .as_ref()
            .and_then(|s| s.content_types.clone())
            .or_else(|| service.as_ref().and_then(|s| s.content_types.clone())),
    })
}

fn resolve_balance(service: &BalanceSettings, route: Option<&BalanceSettings>) -> ResolvedBalance {
    let policy = route
        .and_then(|r| r.policy)
        .or(service.policy)
        .unwrap_or(LbPolicy::RoundRobin);
    let try_duration_secs = route
        .and_then(|r| r.try_duration_secs)
        .or(service.try_duration_secs)
        .unwrap_or(DEFAULT_TRY_DURATION_SECS);
    let mut interval_secs = route
        .and_then(|r| r.interval_secs)
        .or(service.interval_secs)
        .unwrap_or(DEFAULT_INTERVAL_SECS);

    // Each level is checked on its own when parsed, which cannot see a zero
    // interval at one level meeting a non-zero try duration at the other.
    // Caddy spins the CPU on that pairing, so refuse to emit it.
    if interval_secs == 0.0 && try_duration_secs > 0.0 {
        tracing::warn!(
            try_duration_secs,
            "balance interval of zero would spin against a non-zero try duration; \
             using the default interval"
        );
        interval_secs = DEFAULT_INTERVAL_SECS;
    }

    ResolvedBalance {
        policy,
        try_duration_secs,
        interval_secs,
    }
}

// ---------------------------------------------------------------------------
// BSL map parsing
// ---------------------------------------------------------------------------

// l[impl service.http.compress.fields]
pub(super) fn parse_compress(mut map: Map) -> Result<CompressSettings, Box<EvalAltResult>> {
    let encodings = match map.remove("encodings") {
        None => None,
        Some(value) => {
            let arr = value
                .try_cast::<rhai::Array>()
                .ok_or_else(|| -> Box<EvalAltResult> {
                    "compress `encodings` must be an array of strings".into()
                })?;
            if arr.is_empty() {
                return Err("compress `encodings` must not be empty".into());
            }
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                let s = v.into_string().map_err(|t| -> Box<EvalAltResult> {
                    format!("compress `encodings` entries must be strings, got {t}").into()
                })?;
                out.push(Encoding::parse(&s)?);
            }
            Some(out)
        }
    };

    let minimum_length = match map.remove("minimum_length") {
        None => None,
        Some(value) => {
            let n = value.as_int().map_err(|t| -> Box<EvalAltResult> {
                format!("compress `minimum_length` must be an integer number of bytes, got {t}")
                    .into()
            })?;
            if n < 0 {
                return Err(
                    format!("compress `minimum_length` must not be negative, got {n}").into(),
                );
            }
            Some(n as u64)
        }
    };

    let content_types = match map.remove("content_types") {
        None => None,
        Some(value) => {
            let arr = value
                .try_cast::<rhai::Array>()
                .ok_or_else(|| -> Box<EvalAltResult> {
                    "compress `content_types` must be an array of strings".into()
                })?;
            if arr.is_empty() {
                return Err("compress `content_types` must not be empty".into());
            }
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(v.into_string().map_err(|t| -> Box<EvalAltResult> {
                    format!("compress `content_types` entries must be strings, got {t}").into()
                })?);
            }
            Some(out)
        }
    };

    reject_unknown(&map, "compress")?;

    Ok(CompressSettings {
        encodings,
        minimum_length,
        content_types,
    })
}

// l[impl service.balance]
pub(super) fn parse_balance(mut map: Map) -> Result<BalanceSettings, Box<EvalAltResult>> {
    let policy = match map.remove("policy") {
        None => None,
        Some(value) => {
            let s = value.into_string().map_err(|t| -> Box<EvalAltResult> {
                format!("balance `policy` must be a string, got {t}").into()
            })?;
            Some(LbPolicy::parse(&s)?)
        }
    };

    let try_duration_secs = take_seconds(&mut map, "try_duration")?;
    let interval_secs = take_seconds(&mut map, "interval")?;

    // A zero interval only makes sense when this same map also switches
    // retrying off; otherwise the pairing is the CPU-spinning one.
    if interval_secs == Some(0.0) && try_duration_secs != Some(0.0) {
        return Err(
            "balance `interval` may only be zero when `try_duration` is also zero, \
                    because retrying without a pause spins whenever every upstream is unreachable"
                .into(),
        );
    }

    reject_unknown(&map, "balance")?;

    Ok(BalanceSettings {
        policy,
        try_duration_secs,
        interval_secs,
    })
}

// l[impl service.http.rate-limit.fields]
pub(super) fn parse_rate_limit(mut map: Map) -> Result<RateLimitSettings, Box<EvalAltResult>> {
    let max_events = map.remove("max_events");
    let window = map.remove("window");

    // Before the required-field checks: with both known keys already taken,
    // whatever is left is a key the caller invented. Reporting `max_event: 10`
    // as a missing `max_events` would name everything except the typo.
    reject_unknown(&map, "rate_limit")?;

    let max_events = match max_events {
        None => return Err("rate_limit requires `max_events`".into()),
        Some(value) => {
            let n = value.as_int().map_err(|t| -> Box<EvalAltResult> {
                format!("rate_limit `max_events` must be an integer number of requests, got {t}")
                    .into()
            })?;
            if n <= 0 {
                return Err(
                    format!("rate_limit `max_events` must be a positive integer, got {n}").into(),
                );
            }
            let n = n as u64;
            if n > MAX_MAX_EVENTS {
                return Err(format!(
                    "rate_limit `max_events` must be at most {MAX_MAX_EVENTS}, got {n}"
                )
                .into());
            }
            n
        }
    };

    let window_secs = match window {
        None => return Err("rate_limit requires `window`".into()),
        Some(value) => {
            // The range check rejects NaN and the infinities along with
            // everything else outside it, so it is the only check needed.
            let n = as_number(value, "rate_limit", "window")?;
            if !(MIN_WINDOW_SECS..=MAX_WINDOW_SECS).contains(&n) {
                return Err(format!(
                    "rate_limit `window` must be between {MIN_WINDOW_SECS} and \
                     {MAX_WINDOW_SECS} seconds, got {n}"
                )
                .into());
            }
            n
        }
    };

    Ok(RateLimitSettings {
        max_events,
        window_secs,
    })
}

// l[impl service.http.headers]
pub(super) fn parse_headers(mut map: Map) -> Result<HeaderSettings, Box<EvalAltResult>> {
    let request = map.remove("request");
    let response = map.remove("response");

    // Before the emptiness check, so that a `requests:` typo is reported as
    // the invented key it is rather than as a config naming no direction.
    reject_unknown(&map, "headers")?;

    if request.is_none() && response.is_none() {
        return Err("headers requires `request`, `response`, or both".into());
    }

    Ok(HeaderSettings {
        request: parse_direction(request, "request")?,
        response: parse_direction(response, "response")?,
    })
}

fn parse_direction(
    value: Option<Dynamic>,
    direction: &str,
) -> Result<HeaderRules, Box<EvalAltResult>> {
    let Some(value) = value else {
        return Ok(HeaderRules::default());
    };
    let type_name = value.type_name();
    let Some(mut map) = value.try_cast::<Map>() else {
        return Err(
            format!("headers `{direction}` must be a map of operations, got {type_name}").into(),
        );
    };

    let replace = map.remove("replace");
    let add = map.remove("add");
    let remove = map.remove("remove");

    reject_unknown(&map, &format!("headers {direction}"))?;

    if replace.is_none() && add.is_none() && remove.is_none() {
        return Err(format!("headers `{direction}` requires `replace`, `add`, or `remove`").into());
    }

    let mut rules = HeaderRules::default();
    if let Some(value) = replace {
        collect_valued(value, direction, "replace", HeaderOp::Replace, &mut rules)?;
    }
    if let Some(value) = add {
        collect_valued(value, direction, "add", HeaderOp::Add, &mut rules)?;
    }
    if let Some(value) = remove {
        collect_removals(value, direction, &mut rules)?;
    }
    Ok(rules)
}

fn collect_valued(
    value: Dynamic,
    direction: &str,
    op: &str,
    build: fn(Vec<String>) -> HeaderOp,
    into: &mut HeaderRules,
) -> Result<(), Box<EvalAltResult>> {
    let type_name = value.type_name();
    let Some(map) = value.try_cast::<Map>() else {
        return Err(format!(
            "headers `{direction}.{op}` must be a map of header name to value, got {type_name}"
        )
        .into());
    };
    for (name, value) in map {
        let name = HeaderName::parse(&name)?;
        let values = parse_values(value, direction, op, &name)?;
        insert_unique(into, name, build(values), direction)?;
    }
    Ok(())
}

/// A value is a string, or an array of strings where the header is to carry
/// several. `Set-Cookie` is the case that requires the array form, since
/// several cookies cannot be folded into one header.
// l[impl service.http.headers.fields]
fn parse_values(
    value: Dynamic,
    direction: &str,
    op: &str,
    name: &HeaderName,
) -> Result<Vec<String>, Box<EvalAltResult>> {
    let at = format!("headers `{direction}.{op}` value for `{}`", name.as_str());
    let type_name = value.type_name();

    // take: dispatch — string first, then array, then throw. Neither arm
    // accepts a value of the other shape, so a script type error still fails
    // where the script that made it can be named.
    if let Ok(single) = value.clone().into_string() {
        return Ok(vec![header_value(single, &at)?]);
    }

    if let Some(array) = value.try_cast::<rhai::Array>() {
        if array.is_empty() {
            return Err(format!(
                "{at} must not be an empty array; use `remove` to discard a header"
            )
            .into());
        }
        let mut out = Vec::with_capacity(array.len());
        for entry in array {
            let entry = entry.into_string().map_err(|t| -> Box<EvalAltResult> {
                format!("{at} must be an array of strings, got {t}").into()
            })?;
            out.push(header_value(entry, &at)?);
        }
        return Ok(out);
    }

    Err(format!(
        "{at} must be a string, or an array of strings where the header carries \
         several values; got {type_name}"
    )
    .into())
}

/// The proxy writes a value onto the wire as given, so one carrying CR or LF
/// would end the header and begin another of the declaration's choosing.
// l[impl service.http.headers.fields]
fn header_value(value: String, at: &str) -> Result<String, Box<EvalAltResult>> {
    if value.contains(['\r', '\n']) {
        return Err(format!("{at} must not contain a carriage return or line feed").into());
    }
    Ok(value)
}

fn collect_removals(
    value: Dynamic,
    direction: &str,
    into: &mut HeaderRules,
) -> Result<(), Box<EvalAltResult>> {
    let type_name = value.type_name();
    let Some(array) = value.try_cast::<rhai::Array>() else {
        return Err(format!(
            "headers `{direction}.remove` must be an array of header names, got {type_name}"
        )
        .into());
    };
    if array.is_empty() {
        return Err(format!("headers `{direction}.remove` must not be empty").into());
    }
    for entry in array {
        let name = entry.into_string().map_err(|t| -> Box<EvalAltResult> {
            format!("headers `{direction}.remove` must be an array of header names, got {t}").into()
        })?;
        let name = HeaderName::parse(&name)?;
        insert_unique(into, name, HeaderOp::Remove, direction)?;
    }
    Ok(())
}

/// A name takes one operation per direction. Two contradict each other and
/// resolution has no order to settle them by, so the declaration is refused
/// rather than one of them silently winning.
// l[impl service.http.headers.fields]
fn insert_unique(
    into: &mut HeaderRules,
    name: HeaderName,
    op: HeaderOp,
    direction: &str,
) -> Result<(), Box<EvalAltResult>> {
    if into.0.contains_key(&name) {
        return Err(format!(
            "headers `{direction}` gives `{}` more than one operation; \
             a header takes one operation per direction, and names are \
             compared without regard to case",
            name.as_str()
        )
        .into());
    }
    into.0.insert(name, op);
    Ok(())
}

fn take_seconds(map: &mut Map, key: &str) -> Result<Option<f64>, Box<EvalAltResult>> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    let n = as_number(value, "balance", key)?;
    if n < 0.0 {
        return Err(format!("balance `{key}` must not be negative, got {n}").into());
    }
    if !n.is_finite() {
        return Err(format!("balance `{key}` must be a finite number of seconds").into());
    }
    Ok(Some(n))
}

/// Accepts both `10` and `10.0`: a whole number of seconds is the common case
/// and rhai types that as an integer.
fn as_number(value: Dynamic, what: &str, key: &str) -> Result<f64, Box<EvalAltResult>> {
    if let Some(f) = value.clone().try_cast::<f64>() {
        return Ok(f);
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return Ok(i as f64);
    }
    Err(format!(
        "{what} `{key}` must be a number of seconds, got {}",
        value.type_name()
    )
    .into())
}

fn reject_unknown(map: &Map, what: &str) -> Result<(), Box<EvalAltResult>> {
    if map.is_empty() {
        return Ok(());
    }
    let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    Err(format!("unknown `{what}` field(s): {}", keys.join(", ")).into())
}

#[cfg(test)]
mod tests;
