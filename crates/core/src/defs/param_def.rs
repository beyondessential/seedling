use std::{
    collections::BTreeSet,
    fmt,
    ops::{Range, RangeInclusive},
};

use rhai::{Dynamic, EvalAltResult, INT};
use seedling_protocol::names::SiteVolumeName;
use strum::VariantArray;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use super::{
    purl::{Purl, QualifierValue},
    quantity::{format_quantity, parse_quantity},
    take::{reject_unknown_keys, take_map, take_string, take_string_array},
    url::{Url, UrlOrigin, parse_hostname},
    version::{SemVer, Version},
};

// spec: ACN#param-schemas
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub kind: ParamKind,
    pub required: bool,
    pub default_value: Option<SuppliedValue>,
    pub description: Option<String>,
    /// What the script declared for `secret`, or `None` when it said nothing,
    /// in which case the kind decides. An explicit `false` is a declaration, so
    /// it is not the same as unset: it is how a password kind opts out.
    // spec: PARAM#secret-parameters
    pub secret: Option<bool>,
}

impl ParamDef {
    // spec: PARAM#secret-parameters
    pub fn is_secret(&self) -> bool {
        self.secret.unwrap_or(matches!(
            self.kind,
            ParamKind::Password | ParamKind::WeakPassword
        ))
    }

    /// Check that the param is not secret where its kind lists every value it
    /// can take.
    // spec: PARAM#secret-parameters
    pub fn check_secret(&self) -> Result<(), String> {
        match &self.kind {
            ParamKind::Boolean | ParamKind::Choice { .. } if self.is_secret() => Err(format!(
                "a {} param cannot be secret, since the definition lists every value it can take",
                self.kind.as_str()
            )),
            _ => Ok(()),
        }
    }

    /// The declared default as the text a param takes, or `None` without one.
    pub fn default_text(&self) -> Option<String> {
        self.default_value.as_ref().map(SuppliedValue::canonical)
    }

    /// Check the declared default against the kind, giving the reason the
    /// kind rejects it.
    // spec: ACN#param-schemas
    // spec: PARAM#kind-checks
    pub fn check_default(&self) -> Result<(), String> {
        match &self.default_value {
            Some(default) => self.kind.parse_supplied(default).map(drop),
            None => Ok(()),
        }
    }
}

/// The name of a param kind, without the arguments some kinds take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, VariantArray)]
pub enum ParamKindName {
    Text,
    Multiline,
    Email,
    Password,
    WeakPassword,
    Random,
    Boolean,
    Integer,
    Quantity,
    Choice,
    Version,
    VersionSemver,
    Purl,
    Url,
    UrlOrigin,
    Hostname,
    Volume,
}

impl ParamKindName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Multiline => "multiline",
            Self::Email => "email",
            Self::Password => "password",
            Self::WeakPassword => "weak-password",
            Self::Random => "random",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Quantity => "quantity",
            Self::Choice => "choice",
            Self::Version => "version",
            Self::VersionSemver => "version-semver",
            Self::Purl => "purl",
            Self::Url => "url",
            Self::UrlOrigin => "url-origin",
            Self::Hostname => "hostname",
            Self::Volume => "volume",
        }
    }

    /// The arguments a kind map naming this kind may carry.
    // spec: ACN#param-kinds
    fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::Integer | Self::Quantity => &["range"],
            Self::Choice => &["options"],
            _ => &[],
        }
    }
}

impl std::str::FromStr for ParamKindName {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::VARIANTS
            .iter()
            .copied()
            .find(|name| name.as_str() == s)
            .ok_or(())
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub enum ParamKind {
    // spec: ACN#param-kinds
    #[default]
    Text,
    // spec: ACN#param-kinds
    Multiline,
    // spec: ACN#param-kinds
    Email,
    // spec: ACN#param-kinds
    Password,
    // spec: ACN#param-kinds
    WeakPassword,
    // spec: ACN#param-kinds
    Random,
    // spec: ACN#param-kinds
    Boolean,
    // spec: ACN#param-kinds
    Integer {
        range: Option<IntRange>,
    },
    // spec: ACN#param-kinds
    Quantity {
        range: Option<IntRange>,
    },
    // spec: ACN#param-kinds
    Choice {
        options: Vec<String>,
    },
    // spec: ACN#param-kinds
    Version,
    // spec: ACN#param-kinds
    VersionSemver,
    // spec: ACN#param-kinds
    Purl,
    // spec: ACN#param-kinds
    Url,
    // spec: ACN#param-kinds
    UrlOrigin,
    // spec: ACN#param-kinds
    Hostname,
    /// A reference to a site volume; only valid in action and shell param
    /// schemas. The runtime resolves the operator-supplied volume reference
    /// to an operation-scoped binding before invoking the closure.
    // spec: ACN#volume-params
    Volume,
}

impl ParamKind {
    pub fn name(&self) -> ParamKindName {
        match self {
            Self::Text => ParamKindName::Text,
            Self::Multiline => ParamKindName::Multiline,
            Self::Email => ParamKindName::Email,
            Self::Password => ParamKindName::Password,
            Self::WeakPassword => ParamKindName::WeakPassword,
            Self::Random => ParamKindName::Random,
            Self::Boolean => ParamKindName::Boolean,
            Self::Integer { .. } => ParamKindName::Integer,
            Self::Quantity { .. } => ParamKindName::Quantity,
            Self::Choice { .. } => ParamKindName::Choice,
            Self::Version => ParamKindName::Version,
            Self::VersionSemver => ParamKindName::VersionSemver,
            Self::Purl => ParamKindName::Purl,
            Self::Url => ParamKindName::Url,
            Self::UrlOrigin => ParamKindName::UrlOrigin,
            Self::Hostname => ParamKindName::Hostname,
            Self::Volume => ParamKindName::Volume,
        }
    }

    pub fn as_str(&self) -> &'static str {
        self.name().as_str()
    }

    /// The kind `name` stands for given no arguments, or `None` for a kind
    /// with a required argument.
    // spec: ACN#kind-arguments
    pub fn without_arguments(name: ParamKindName) -> Option<Self> {
        (name != ParamKindName::Choice).then(|| kind_with(name, None, None))
    }

    /// Whether this kind may appear in static param schemas (`app.param`).
    /// `Volume` is rejected there because its bindings are operation-scoped —
    /// the only sensible static equivalent is a declared external_volume
    /// mapping.
    pub fn allowed_static(&self) -> bool {
        !matches!(self, Self::Volume)
    }

    /// Parse `value` as a value of this kind, or give the reason it is not
    /// one. This is the whole of the kind's check on a value, both for the
    /// runtime and for the kind's `std` helper.
    // spec: ACN#kind-parsers
    pub fn parse(&self, value: &str) -> Result<ParsedValue, String> {
        if self.is_single_line() {
            reject_control_characters(value)?;
        }
        match self {
            Self::Text | Self::Multiline | Self::WeakPassword | Self::Random => {}
            // spec: ACN#kind-parsers
            Self::Email => {
                let Some((local, domain)) = split_email(value) else {
                    return Err("invalid email address".to_owned());
                };
                return Ok(ParsedValue::Email {
                    local: local.to_owned(),
                    domain: domain.to_owned(),
                });
            }
            // spec: ACN#kind-parsers
            Self::Password => {
                if !is_strong_password(value) {
                    return Err("password is too weak".to_owned());
                }
            }
            // spec: ACN#kind-parsers
            Self::Boolean => {
                return match value {
                    "true" => Ok(ParsedValue::Bool(true)),
                    "false" => Ok(ParsedValue::Bool(false)),
                    _ => Err("must be true or false".to_owned()),
                };
            }
            // spec: ACN#kind-parsers
            Self::Integer { range } => {
                let n = parse_decimal(value)?;
                if let Some(range) = range
                    && !range.contains(n)
                {
                    return Err(format!("must be {range}"));
                }
                return Ok(ParsedValue::Int(n));
            }
            // spec: ACN#quantities
            Self::Quantity { range } => {
                let n = parse_quantity(value)?;
                if let Some(range) = range
                    && !range.contains(n)
                {
                    return Err(format!("must be {}", range.describe(format_quantity)));
                }
                return Ok(ParsedValue::Int(n));
            }
            // spec: ACN#kind-parsers
            Self::Choice { options } => {
                if !options.iter().any(|option| option == value) {
                    return Err(format!(
                        "must be one of the options {}",
                        options
                            .iter()
                            .map(|o| format!("{o:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            Self::Version => return Version::parse(value).map(ParsedValue::Version),
            Self::VersionSemver => return SemVer::parse(value).map(ParsedValue::SemVer),
            Self::Purl => return Purl::parse(value).map(ParsedValue::Purl),
            Self::Url => return Url::parse(value).map(ParsedValue::Url),
            Self::UrlOrigin => return UrlOrigin::parse(value).map(ParsedValue::UrlOrigin),
            Self::Hostname => return parse_hostname(value).map(ParsedValue::Value),
            // spec: ACN#kind-parsers
            Self::Volume => {
                SiteVolumeName::new(value).map_err(|e| e.to_string())?;
            }
        }
        Ok(ParsedValue::Value(value.to_owned()))
    }

    /// Check that a supplied boolean or integer is for the kind that takes
    /// one; a string is for every kind.
    // spec: ACN#param-schemas
    pub fn check_type(&self, value: &SuppliedValue) -> Result<(), String> {
        let given = match (self, value) {
            (_, SuppliedValue::Text(_))
            | (Self::Boolean, SuppliedValue::Bool(_))
            | (Self::Integer { .. } | Self::Quantity { .. }, SuppliedValue::Int(_)) => {
                return Ok(());
            }
            (_, SuppliedValue::Bool(_)) => "a boolean",
            (_, SuppliedValue::Int(_)) => "an integer",
        };
        Err(format!(
            "{given} is given for a param of kind {}",
            self.as_str()
        ))
    }

    /// Parse a supplied value: a boolean or integer, for the kind that takes
    /// one, as its canonical form, and a string as [`Self::parse`] does.
    // spec: ACN#param-schemas
    pub fn parse_supplied(&self, value: &SuppliedValue) -> Result<ParsedValue, String> {
        self.check_type(value)?;
        self.parse(&value.canonical())
    }

    // spec: ACN#kind-parsers
    fn is_single_line(&self) -> bool {
        !matches!(self, Self::Multiline)
    }
}

/// Read a kind from a script: a kind name, or a kind map of `name` and the
/// kind's arguments.
// spec: ACN#kind-arguments
pub fn take_kind(what: &str, value: Dynamic) -> Result<ParamKind, Box<EvalAltResult>> {
    // take: dispatch
    if value.is_string() {
        let name = take_kind_name(what, value)?;
        return ParamKind::without_arguments(name).ok_or_else(|| {
            format!("`{what}`: the choice kind takes `options`, so it is given as a map").into()
        });
    }
    let map = take_map(what, value).map_err(|_| -> Box<EvalAltResult> {
        format!("`{what}` must be a kind name or a kind map").into()
    })?;
    let Some(name) = map.get("name") else {
        return Err(format!("`{what}` is a kind map without a `name`").into());
    };
    let name = take_kind_name(&format!("{what}.name"), name.clone())?;
    let mut allowed = vec!["name"];
    allowed.extend_from_slice(name.arguments());
    reject_unknown_keys(what, &map, &allowed)?;
    let range = map
        .get("range")
        .map(|range| take_range(&format!("{what}.range"), range.clone()))
        .transpose()?;
    let options = map
        .get("options")
        .map(|options| take_options(&format!("{what}.options"), options.clone()))
        .transpose()?;
    if name == ParamKindName::Choice && options.is_none() {
        return Err(format!("`{what}`: the choice kind takes `options`").into());
    }
    Ok(kind_with(name, range, options))
}

fn kind_with(
    name: ParamKindName,
    range: Option<IntRange>,
    options: Option<Vec<String>>,
) -> ParamKind {
    match name {
        ParamKindName::Text => ParamKind::Text,
        ParamKindName::Multiline => ParamKind::Multiline,
        ParamKindName::Email => ParamKind::Email,
        ParamKindName::Password => ParamKind::Password,
        ParamKindName::WeakPassword => ParamKind::WeakPassword,
        ParamKindName::Random => ParamKind::Random,
        ParamKindName::Boolean => ParamKind::Boolean,
        ParamKindName::Integer => ParamKind::Integer { range },
        ParamKindName::Quantity => ParamKind::Quantity { range },
        ParamKindName::Choice => ParamKind::Choice {
            options: options.unwrap_or_default(),
        },
        ParamKindName::Version => ParamKind::Version,
        ParamKindName::VersionSemver => ParamKind::VersionSemver,
        ParamKindName::Purl => ParamKind::Purl,
        ParamKindName::Url => ParamKind::Url,
        ParamKindName::UrlOrigin => ParamKind::UrlOrigin,
        ParamKindName::Hostname => ParamKind::Hostname,
        ParamKindName::Volume => ParamKind::Volume,
    }
}

fn take_kind_name(what: &str, value: Dynamic) -> Result<ParamKindName, Box<EvalAltResult>> {
    let name = take_string(what, value)?;
    name.parse()
        .map_err(|()| format!("`{what}`: unknown param kind {name:?}").into())
}

/// Read the `range` of an `integer` or `quantity` kind.
// spec: ACN#kind-arguments
pub fn take_range(what: &str, value: Dynamic) -> Result<IntRange, Box<EvalAltResult>> {
    let type_name = value.type_name();
    let range = if let Some(range) = value.clone().try_cast::<Range<INT>>() {
        IntRange::new(range.start, range.end, false)
    } else if let Some(range) = value.try_cast::<RangeInclusive<INT>>() {
        IntRange::new(*range.start(), *range.end(), true)
    } else {
        return Err(format!("`{what}` must be an integer range, got {type_name}").into());
    };
    range.ok_or_else(|| format!("`{what}` holds no integer").into())
}

/// Read a `choice` kind's `options`.
// spec: ACN#kind-arguments
pub fn take_options(what: &str, value: Dynamic) -> Result<Vec<String>, Box<EvalAltResult>> {
    let options = take_string_array(what, value)?;
    if options.is_empty() {
        return Err(format!("`{what}` must hold at least one option").into());
    }
    let mut seen = BTreeSet::new();
    for option in &options {
        if option.is_empty() {
            return Err(format!("`{what}` holds an empty option").into());
        }
        ParamKind::Text
            .parse(option)
            .map_err(|e| -> Box<EvalAltResult> {
                format!("`{what}` option {option:?} {e}").into()
            })?;
        if !seen.insert(option.as_str()) {
            return Err(format!("`{what}` holds {option:?} more than once").into());
        }
    }
    Ok(options)
}

/// The range of an `integer` or `quantity` kind, kept with the end the script wrote so that an
/// exclusive range running to the largest integer still reads as unbounded.
// spec: ACN#kind-arguments
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRange {
    start: i64,
    end: i64,
    inclusive: bool,
}

impl IntRange {
    /// The range, or `None` when it holds no integer.
    pub fn new(start: i64, end: i64, inclusive: bool) -> Option<Self> {
        let holds_one = if inclusive { start <= end } else { start < end };
        holds_one.then_some(Self {
            start,
            end,
            inclusive,
        })
    }

    pub fn contains(&self, n: i64) -> bool {
        n >= self.start && (n < self.end || (self.inclusive && n == self.end))
    }

    /// The least value, or `None` where the range starts at the smallest
    /// integer.
    pub fn min(&self) -> Option<i64> {
        (self.start != i64::MIN).then_some(self.start)
    }

    /// The greatest value, or `None` where the range ends at the largest
    /// integer, inclusive or not.
    pub fn max(&self) -> Option<i64> {
        (self.end != i64::MAX).then(|| self.greatest())
    }

    fn greatest(&self) -> i64 {
        if self.inclusive {
            self.end
        } else {
            self.end - 1
        }
    }
}

impl IntRange {
    /// The range as its least and greatest values, each written by `bound`.
    pub fn describe(&self, bound: impl Fn(i64) -> String) -> String {
        format!(
            "between {} and {}",
            bound(self.start),
            bound(self.greatest())
        )
    }
}

impl fmt::Display for IntRange {
    // spec: ACN#kind-parsers
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe(|n| n.to_string()))
    }
}

/// A value as supplied for a param: a string, or a boolean or integer that
/// stands for its canonical form.
// spec: ACN#param-schemas
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuppliedValue {
    Text(String),
    Bool(bool),
    Int(i64),
}

impl SuppliedValue {
    /// The value's text: a string as it is, and a boolean or integer in its
    /// canonical form.
    // spec: ACN#kind-parsers
    pub fn canonical(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
        }
    }

    /// Whether the value is the empty string, which stands for no value.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Text(text) if text.is_empty())
    }

    /// The JSON string, boolean or integer, or `None` for any other JSON
    /// value. A JSON integer is a number with no fraction or exponent that
    /// fits a signed 64-bit integer.
    // spec: ACI#action-parameters
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        match value {
            serde_json::Value::String(text) => Some(Self::Text(text.clone())),
            serde_json::Value::Bool(b) => Some(Self::Bool(*b)),
            serde_json::Value::Number(n) => n.as_i64().map(Self::Int),
            _ => None,
        }
    }

    /// Read a value a script gives: a string, a boolean or an integer.
    pub fn take(what: &str, value: Dynamic) -> Result<Self, Box<EvalAltResult>> {
        // take: dispatch
        if let Ok(b) = value.as_bool() {
            return Ok(Self::Bool(b));
        }
        // take: dispatch
        if let Ok(n) = value.as_int() {
            return Ok(Self::Int(n));
        }
        let type_name = value.type_name();
        value.into_string().map(Self::Text).map_err(|_| {
            format!("`{what}` must be a string, a boolean or an integer, got {type_name}").into()
        })
    }
}

/// An integer in its decimal form: digits with no leading zero, after a `-`
/// for a negative one.
// spec: ACN#kind-parsers
fn parse_decimal(value: &str) -> Result<i64, String> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let decimal = is_decimal_number(digits) && value != "-0";
    if !decimal {
        return Err("must be an integer in decimal form".to_owned());
    }
    value
        .parse()
        .map_err(|_| "must be an integer within the signed 64-bit range".to_owned())
}

/// Whether `digits` is a number in ASCII decimal digits with no leading
/// zero, zero being written `0`.
// spec: ACN#kind-parsers
// spec: IP#decimal-number
pub(crate) fn is_decimal_number(digits: &str) -> bool {
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
}

/// What a kind's parser makes of a value it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedValue {
    /// The value itself, for a kind with no parts.
    Value(String),
    Email {
        local: String,
        domain: String,
    },
    Bool(bool),
    Int(i64),
    Version(Version),
    SemVer(SemVer),
    Purl(Purl),
    Url(Url),
    UrlOrigin(UrlOrigin),
}

impl ParsedValue {
    /// The value a script receives.
    // spec: ACN#param-kinds
    pub fn into_dynamic(self) -> Dynamic {
        match self {
            Self::Value(value) => value.into(),
            Self::Email { local, domain } => {
                let mut parts = rhai::Map::new();
                parts.insert("local".into(), local.into());
                parts.insert("domain".into(), domain.into());
                parts.into()
            }
            Self::Bool(b) => b.into(),
            Self::Int(n) => n.into(),
            Self::Version(version) => Dynamic::from(version),
            Self::SemVer(version) => Dynamic::from(version),
            Self::Purl(purl) => purl_map(purl),
            Self::Url(url) => url_map(url),
            Self::UrlOrigin(origin) => url_origin_map(origin),
        }
    }
}

/// A Package URL's components as a map, leaving out those it does not have.
// spec: PURL#components
fn purl_map(purl: Purl) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("type".into(), purl.ty.into());
    if let Some(namespace) = purl.namespace {
        map.insert("namespace".into(), namespace.into());
    }
    map.insert("name".into(), purl.name.into());
    if let Some(version) = purl.version {
        map.insert("version".into(), Dynamic::from(version));
    }
    if !purl.qualifiers.is_empty() {
        let qualifiers: rhai::Map = purl
            .qualifiers
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    QualifierValue::Text(text) => text.into(),
                    QualifierValue::Checksums(entries) => entries
                        .into_iter()
                        .map(Dynamic::from)
                        .collect::<rhai::Array>()
                        .into(),
                };
                (key.into(), value)
            })
            .collect();
        map.insert("qualifiers".into(), qualifiers.into());
    }
    if let Some(subpath) = purl.subpath {
        map.insert("subpath".into(), subpath.into());
    }
    map.into()
}

/// A URL's components as a map, leaving out those it does not have.
// spec: URL#components
fn url_map(url: Url) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("scheme".into(), url.scheme.into());
    let optional = [
        ("host", url.host),
        ("query", url.query),
        ("fragment", url.fragment),
        ("username", url.username),
        ("password", url.password),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            map.insert(key.into(), value.into());
        }
    }
    if let Some(port) = url.port {
        map.insert("port".into(), INT::from(port).into());
    }
    map.insert("path".into(), url.path.into());
    map.into()
}

// spec: URL#url-origins
fn url_origin_map(origin: UrlOrigin) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("scheme".into(), origin.scheme.into());
    map.insert("host".into(), origin.host.into());
    map.insert("port".into(), INT::from(origin.port).into());
    map.into()
}

// spec: ACN#kind-parsers
fn reject_control_characters(value: &str) -> Result<(), String> {
    match line_breaking_character(value) {
        Some(c) => Err(format!(
            "contains U+{:04X}, which a single-line value cannot hold",
            u32::from(c)
        )),
        None => Ok(()),
    }
}

/// The first character in `value` whose general category is Control, Line
/// Separator or Paragraph Separator.
pub(super) fn line_breaking_character(value: &str) -> Option<char> {
    value.chars().find(|c| {
        matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
    })
}

/// The local part and domain of an email address, split at the first `@`.
// spec: ACN#kind-parsers
fn split_email(email: &str) -> Option<(&str, &str)> {
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.');
    valid.then_some((local, domain))
}

// spec: ACN#kind-parsers
fn is_strong_password(password: &str) -> bool {
    zxcvbn::zxcvbn(password, &[]).score() >= zxcvbn::Score::Three
}
