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
/// compression is off for that route.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRouteProxy {
    pub compress: Option<ResolvedCompress>,
    pub balance: ResolvedBalance,
}

impl Default for ResolvedRouteProxy {
    fn default() -> Self {
        resolve(&ProxySettings::default(), None)
    }
}

// l[impl service.http.proxy-settings.resolution]
/// Resolve one route's settings. Every field is taken from the route if the
/// route named it, otherwise from the service if the service named it,
/// otherwise from the field's default.
///
/// Passing `None` for `route` resolves the service's own values, which is what
/// the synthesised `/` route of a service with no HTTP route bindings takes.
pub fn resolve(service: &ProxySettings, route: Option<&ProxySettings>) -> ResolvedRouteProxy {
    let route_balance = route.map(|r| &r.balance);
    let route_compress = route.and_then(|r| r.compress.as_ref());

    ResolvedRouteProxy {
        compress: resolve_compress(service.compress.as_ref(), route_compress),
        balance: resolve_balance(&service.balance, route_balance),
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

// l[impl service.http.balance]
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

fn take_seconds(map: &mut Map, key: &str) -> Result<Option<f64>, Box<EvalAltResult>> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    let n = as_number(value, key)?;
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
fn as_number(value: Dynamic, key: &str) -> Result<f64, Box<EvalAltResult>> {
    if let Some(f) = value.clone().try_cast::<f64>() {
        return Ok(f);
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return Ok(i as f64);
    }
    Err(format!(
        "balance `{key}` must be a number of seconds, got {}",
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
