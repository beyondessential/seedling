//! Compression and balancing settings declared on an HTTP Service and on its
//! individual routes.
//!
//! Declared settings keep every field optional so that resolution can tell
//! "the app asked for this value" from "the app said nothing", which is what
//! [`l[service.http.proxy-settings.resolution]`] needs to fall back field by
//! field. Resolved settings have no optionality left except compression being
//! switched off entirely.

use rhai::{Dynamic, EvalAltResult, Map};

/// Caddy's own defaults, read from the source of the pinned proxy image so the
/// emitter can leave a field out whenever it still holds its default.
pub const DEFAULT_MINIMUM_LENGTH: u64 = 512;

/// Bounds on a declared rate limit.
///
/// These are sanity ceilings, not tuning. A limit is charged to the proxy
/// process every app on the host shares: the module preallocates a ring of
/// `max_events` timestamps per distinct client and holds it for the length of
/// the window, so an absurd declaration in one app is another app's memory.
/// The floor exists because the emitted window is a whole number of
/// nanoseconds — a smaller one would round to zero, which the module rejects
/// at provision, failing the entire proxy document rather than the one route.
pub const MIN_WINDOW_SECS: f64 = 0.001;
pub const MAX_WINDOW_SECS: f64 = 86_400.0;
pub const MAX_MAX_EVENTS: u64 = 1_000_000;
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
    /// Resolution is whole-unit, so the limit in force is exactly the one a
    /// level declared; there is no separate resolved form to convert to.
    pub rate_limit: Option<RateLimitSettings>,
}

impl Default for ResolvedRouteProxy {
    fn default() -> Self {
        resolve(&ProxySettings::default(), None)
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
/// Passing `None` for `route` resolves the service's own values, which is what
/// the synthesised `/` route of a service with no HTTP route bindings takes.
pub fn resolve(service: &ProxySettings, route: Option<&ProxySettings>) -> ResolvedRouteProxy {
    let route_balance = route.map(|r| &r.balance);
    let route_compress = route.and_then(|r| r.compress.as_ref());
    let route_rate_limit = route.and_then(|r| r.rate_limit.as_ref());

    ResolvedRouteProxy {
        compress: resolve_compress(service.compress.as_ref(), route_compress),
        balance: resolve_balance(&service.balance, route_balance),
        rate_limit: resolve_rate_limit(service.rate_limit.as_ref(), route_rate_limit),
    }
}

/// Rate limiting resolves as a whole rather than field by field: a route
/// declaring a limit replaces the service's outright, rather than merging with
/// it, because a count and a window only mean anything together. A route that
/// switched limiting off is not limited even where the service declared one.
fn resolve_rate_limit(
    service: Option<&RateLimitDecl>,
    route: Option<&RateLimitDecl>,
) -> Option<RateLimitSettings> {
    match route.or(service) {
        Some(RateLimitDecl::Enabled(s)) => Some(*s),
        Some(RateLimitDecl::Disabled) | None => None,
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
    let max_events = match map.remove("max_events") {
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

    let window_secs = match map.remove("window") {
        None => return Err("rate_limit requires `window`".into()),
        Some(value) => {
            let n = as_number(value, "rate_limit", "window")?;
            if !n.is_finite() || n <= 0.0 {
                return Err(format!(
                    "rate_limit `window` must be a positive, finite number of seconds, got {n}"
                )
                .into());
            }
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

    reject_unknown(&map, "rate_limit")?;

    Ok(RateLimitSettings {
        max_events,
        window_secs,
    })
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
