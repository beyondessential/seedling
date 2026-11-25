//! Shared param-schema validation for action invocations.
//!
//! Both the operator-driven OI invocation path
//! (`oi/handler/actions.rs:invoke_action`) and the script-driven
//! [`Action.call`](crate::defs::action::Action) path apply the same
//! rules to action params: reject reserved keys, apply defaults,
//! enforce required fields, and resolve `kind: "volume"` references
//! against the live site-volume table. The two paths surface errors
//! through different channels (`OiError` vs `EvalAltResult`), so the
//! shared logic returns a tagged [`ParamValidationError`] that each
//! caller maps to its own error type.
//!
//! # Spec
//! - spec: ACN#param-delivery
//! - spec: ACN#volume-params
//! - spec: OPS#sub-action-parameters
//! - spec: ACN#param-delivery

use std::collections::BTreeMap;

use seedling_protocol::names::{ParamName, SiteVolumeName};

use crate::{
    defs::{
        param_def::{ParamDef, ParamKind, ParsedValue, SuppliedValue},
        regex::Budget,
    },
    runtime::param_map::ParamMap,
};

/// Validation outcome carrying enough structure for the caller to
/// either reject the call or surface a precise error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamValidationError {
    /// A param key uses one of the reserved suffixes (`_volume`,
    /// `_filename`).
    ReservedKey { key: String },
    /// One or more schema fields failed the requirements check
    /// (missing-required / invalid-email / weak-password / ...).
    Requirements { messages: Vec<String> },
    /// A schema-declared param was supplied as a type its kind does not
    /// take: anything but a string, or a boolean or integer for a kind that
    /// is not one.
    ///
    /// It is an error rather than an absence, so the value is never silently
    /// replaced by the field's default or reported as a missing required
    /// field.
    WrongType { key: String, reason: String },
    /// A `kind: "volume"` param's value was not a string.
    VolumeNotString { key: String },
    /// A required `kind: "volume"` param was absent.
    VolumeMissing { key: String },
    /// A `kind: "volume"` value did not resolve to an existing site
    /// volume.
    VolumeNotFound { key: String, value: String },
}

impl std::fmt::Display for ParamValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReservedKey { key } => write!(
                f,
                "param key {key:?} is reserved (keys ending in _volume or _filename are reserved)"
            ),
            Self::Requirements { messages } => write!(f, "{}", messages.join("; ")),
            Self::WrongType { key, reason } => write!(f, "param {key:?}: {reason}"),
            Self::VolumeNotString { key } => {
                write!(f, "param {key:?}: volume reference must be a string")
            }
            Self::VolumeMissing { key } => {
                write!(f, "param {key:?}: required volume reference is missing")
            }
            Self::VolumeNotFound { key, value } => {
                write!(f, "param {key:?}: site volume {value:?} not found")
            }
        }
    }
}

impl std::error::Error for ParamValidationError {}

// spec: ACN#param-delivery
/// Reject any param key ending in a reserved suffix.
pub fn reject_reserved_keys(
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), ParamValidationError> {
    reject_reserved(params.keys())
}

/// Refuse the first of `keys` that is reserved for the runtime.
// spec: ACN#param-delivery
pub fn reject_reserved<'a>(
    mut keys: impl Iterator<Item = &'a String>,
) -> Result<(), ParamValidationError> {
    match keys.find(|key| key.ends_with("_volume") || key.ends_with("_filename")) {
        Some(key) => Err(ParamValidationError::ReservedKey { key: key.clone() }),
        None => Ok(()),
    }
}

/// Validate supplied params against `schema` and give the param map the
/// closure receives: each declared param that has a value as its kind's
/// parser result, each declared param without one left out, and every
/// undeclared param as supplied.
// spec: ACN#param-schemas
// spec: ACN#param-delivery
pub fn apply_schema(
    schema: &BTreeMap<ParamName, ParamDef>,
    mut params: serde_json::Map<String, serde_json::Value>,
    regex_limit: usize,
) -> Result<ParamMap, ParamValidationError> {
    let mut supplied = BTreeMap::new();
    for (name, def) in schema {
        let key = name.as_str();
        let Some(value) = params.get(key) else {
            continue;
        };
        let wrong_type = |reason| ParamValidationError::WrongType {
            key: key.to_owned(),
            reason,
        };
        let value = SuppliedValue::from_json(value).ok_or_else(|| {
            wrong_type(format!(
                "{} is not a value a param takes",
                json_type_name(value)
            ))
        })?;
        def.kind.check_type(&value).map_err(wrong_type)?;
        supplied.insert(key.to_owned(), value);
    }

    let parsed = run_requirements(schema, &supplied, regex_limit)?;
    for name in schema.keys() {
        params.remove(name.as_str());
    }
    let mut map = ParamMap::from_values(params);
    for (key, value) in parsed {
        map.insert_parsed(key, value);
    }
    Ok(map)
}

/// What a JSON value is, for an error refusing it.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number that is not a signed 64-bit integer",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

// spec: ACN#volume-params
/// Resolve every `kind: "volume"` param against `lookup`, ensuring
/// each declared volume name is present (when required) and points at
/// a real site volume.
pub fn validate_volume_params<L: VolumeLookup>(
    schema: &BTreeMap<ParamName, ParamDef>,
    params: &ParamMap,
    lookup: &L,
) -> Result<(), ParamValidationError> {
    let volume_names: Vec<&ParamName> = schema
        .iter()
        .filter_map(|(name, def)| (def.kind == ParamKind::Volume).then_some(name))
        .collect();
    if volume_names.is_empty() {
        return Ok(());
    }

    for name in volume_names {
        let key = name.as_str();
        let val = match params.get(key) {
            // An empty value is an absent one, whether it was left empty or
            // is the empty string a form submitted for a param it left blank.
            Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
            Some(serde_json::Value::String(_)) | None => {
                let def = schema.get(name).expect("schema entry");
                if def.required {
                    return Err(ParamValidationError::VolumeMissing {
                        key: key.to_owned(),
                    });
                }
                continue;
            }
            Some(_) => {
                return Err(ParamValidationError::VolumeNotString {
                    key: key.to_owned(),
                });
            }
        };
        // A name the kind's parser refuses is reported as that parser
        // reports it, so the refusal reads the same whichever check found it.
        let site_name =
            SiteVolumeName::new(&val).map_err(|e| ParamValidationError::Requirements {
                messages: vec![format!("{key}: {e}")],
            })?;
        if !lookup.site_volume_exists(&site_name) {
            return Err(ParamValidationError::VolumeNotFound {
                key: key.to_owned(),
                value: val,
            });
        }
    }
    Ok(())
}

/// Adapter trait so the validation can run against either the live
/// OI state or a test double without dragging the whole [`OiState`]
/// into the runtime layer.
pub trait VolumeLookup {
    fn site_volume_exists(&self, name: &SiteVolumeName) -> bool;
}

/// Fill in defaults, enforce required params, and parse each declared
/// param that has a value with its kind, giving the parser results.
// spec: ACN#param-schemas
// spec: OPS#sub-action-parameters
pub fn run_requirements(
    schema: &BTreeMap<ParamName, ParamDef>,
    supplied: &BTreeMap<String, SuppliedValue>,
    regex_limit: usize,
) -> Result<BTreeMap<String, ParsedValue>, ParamValidationError> {
    let mut parsed = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();

    for (field, req_def) in schema {
        let field_str = field.as_str();
        let value = match supplied.get(field_str).filter(|v| !v.is_empty()) {
            Some(value) => value.clone(),
            None => match &req_def.default_value {
                Some(default) => default.clone(),
                None => {
                    if req_def.required {
                        errors.push(format!("{field_str}: required field is missing"));
                    }
                    continue;
                }
            },
        };

        // spec: ACN#kind-parsers
        match req_def
            .kind
            .parse_supplied(&value, &mut Budget::fresh(regex_limit))
        {
            Ok(value) => {
                parsed.insert(field_str.to_owned(), value);
            }
            Err(reason) => errors.push(format!("{field_str}: {reason}")),
        }
    }

    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(ParamValidationError::Requirements { messages: errors })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use seedling_protocol::names::ParamName;
    use serde_json::json;

    use super::{ParamValidationError, VolumeLookup, validate_volume_params};
    use crate::{
        defs::param_def::{IntRange, ParamDef, ParamKind, SuppliedValue},
        runtime::param_map::ParamMap,
    };

    struct NoVolumes;

    impl VolumeLookup for NoVolumes {
        fn site_volume_exists(&self, _: &seedling_protocol::names::SiteVolumeName) -> bool {
            false
        }
    }

    fn text_with_default(default: &str) -> ParamDef {
        ParamDef {
            default_value: Some(SuppliedValue::Text(default.to_owned())),
            ..of_kind(ParamKind::Text)
        }
    }

    fn of_kind(kind: ParamKind) -> ParamDef {
        ParamDef {
            kind,
            required: false,
            default_value: None,
            description: None,
            secret: None,
        }
    }

    fn schema(name: &str, def: ParamDef) -> BTreeMap<ParamName, ParamDef> {
        BTreeMap::from([(ParamName::new(name).expect("param name"), def)])
    }

    /// Apply `schema` under the default limits.
    fn apply_schema(
        schema: &BTreeMap<ParamName, ParamDef>,
        params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<ParamMap, ParamValidationError> {
        super::apply_schema(
            schema,
            params,
            crate::ScriptLimits::default().max_regex_compiled_size,
        )
    }

    fn supplied(
        name: &str,
        value: serde_json::Value,
    ) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::from_iter([(name.to_owned(), value)])
    }

    fn integer(range: Option<IntRange>) -> ParamKind {
        ParamKind::Integer { range }
    }

    // spec: ACN#param-schemas
    // A non-string used to be skipped as absent, so a submitted value was
    // silently replaced by the field's default — the caller believed the
    // value they sent had been taken.
    #[test]
    fn a_value_of_a_type_its_kind_does_not_take_is_refused_rather_than_defaulted() {
        let sch = schema("count", text_with_default("fallback"));
        for (value, reason) in [
            (json!(42), "an integer is given for a param of kind text"),
            (json!(true), "a boolean is given for a param of kind text"),
            (json!(["a"]), "an array is not a value a param takes"),
        ] {
            let err = apply_schema(&sch, supplied("count", value.clone())).expect_err("wrong type");
            assert_eq!(
                err,
                ParamValidationError::WrongType {
                    key: "count".to_owned(),
                    reason: reason.to_owned(),
                },
                "{value}"
            );
        }
    }

    // spec: ACN#param-schemas
    // spec: ACI#action-parameters
    #[test]
    fn a_boolean_or_integer_param_takes_its_native_json_type() {
        let flag = schema("flag", of_kind(ParamKind::Boolean));
        assert_eq!(
            apply_schema(&flag, supplied("flag", json!(true)))
                .unwrap()
                .get("flag"),
            Some(&json!(true))
        );
        assert_eq!(
            apply_schema(&flag, supplied("flag", json!("false")))
                .unwrap()
                .get("flag"),
            Some(&json!(false))
        );

        let count = schema("count", of_kind(integer(None)));
        assert_eq!(
            apply_schema(&count, supplied("count", json!(-7)))
                .unwrap()
                .get("count"),
            Some(&json!(-7))
        );
        assert_eq!(
            apply_schema(&count, supplied("count", json!("12")))
                .unwrap()
                .get("count"),
            Some(&json!(12))
        );
    }

    // spec: ACN#param-schemas
    // spec: ACI#action-parameters
    #[test]
    fn a_quantity_param_takes_a_json_integer_or_a_quantity() {
        let size = schema(
            "size",
            of_kind(ParamKind::Quantity {
                range: IntRange::new(1, 1 << 30, true),
            }),
        );
        for (value, amount) in [(json!(1024), 1024), (json!("512Mi"), 512 << 20)] {
            assert_eq!(
                apply_schema(&size, supplied("size", value.clone()))
                    .unwrap()
                    .get("size"),
                Some(&json!(amount)),
                "{value}"
            );
        }
        let err = apply_schema(&size, supplied("size", json!(0))).expect_err("out of range");
        assert!(format!("{err:?}").contains("between 1 and 1Gi"), "{err:?}");
    }

    // spec: ACI#action-parameters
    #[test]
    fn a_json_number_that_is_not_a_signed_64_bit_integer_is_refused() {
        let count = schema("count", of_kind(integer(None)));
        for text in ["1.0", "1e3", "9223372036854775808", "-9223372036854775809"] {
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            let err = apply_schema(&count, supplied("count", value)).expect_err(text);
            assert!(
                matches!(err, ParamValidationError::WrongType { .. }),
                "{text}: {err:?}"
            );
        }
    }

    // spec: ACN#param-delivery
    #[test]
    fn declared_params_reach_the_closure_as_parser_results() {
        let mut sch = schema("contact", of_kind(ParamKind::Email));
        sch.extend(schema("size", of_kind(integer(IntRange::new(1, 10, true)))));
        sch.extend(schema(
            "tier",
            of_kind(ParamKind::Choice {
                options: vec!["a".into(), "b".into()],
            }),
        ));
        sch.extend(schema("pinned", of_kind(ParamKind::Version)));
        let mut params = supplied("contact", json!("ops@example.org"));
        params.insert("size".into(), json!("3"));
        params.insert("tier".into(), json!("b"));
        params.insert("pinned".into(), json!("v16.4"));
        params.insert("extra".into(), json!(["as", "supplied"]));

        let map = apply_schema(&sch, params).unwrap();
        assert_eq!(
            map.get("contact"),
            Some(&json!({ "local": "ops", "domain": "example.org" }))
        );
        assert_eq!(map.get("size"), Some(&json!(3)));
        assert_eq!(map.get("tier"), Some(&json!("b")));
        assert_eq!(map.get("extra"), Some(&json!(["as", "supplied"])));

        let delivered = map.to_rhai().unwrap();
        let pinned = delivered["pinned"].clone();
        let pinned = pinned
            .try_cast::<crate::defs::version::Version>()
            .expect("a Version");
        assert_eq!(pinned.to_string(), "v16.4");
    }

    // spec: ACN#param-schemas
    #[test]
    fn an_absent_field_still_takes_its_default() {
        let sch = schema("name", text_with_default("fallback"));
        let map = apply_schema(&sch, serde_json::Map::new()).expect("a default fills it");
        assert_eq!(map.get("name"), Some(&json!("fallback")));

        let flag = schema(
            "flag",
            ParamDef {
                default_value: Some(SuppliedValue::Bool(true)),
                ..of_kind(ParamKind::Boolean)
            },
        );
        let map = apply_schema(&flag, supplied("flag", json!(""))).expect("a default fills it");
        assert_eq!(map.get("flag"), Some(&json!(true)));
    }

    // spec: ACN#param-delivery
    #[test]
    fn a_declared_param_without_a_value_is_absent() {
        for value in [None, Some(json!(""))] {
            let params = value.map(|v| supplied("note", v)).unwrap_or_default();
            let map = apply_schema(&schema("note", of_kind(ParamKind::Text)), params).unwrap();
            assert_eq!(map, ParamMap::default());
        }
    }

    // spec: ACN#volume-params
    // spec: ACN#param-schemas
    // An empty optional volume param is an absent one: it named a volume
    // called "", which is not a volume name, and the invocation was refused.
    #[test]
    fn an_empty_optional_volume_param_is_absent() {
        let def = |required| ParamDef {
            required,
            ..of_kind(ParamKind::Volume)
        };
        let params = ParamMap::from_values(supplied("data", json!("")));

        validate_volume_params(&schema("data", def(false)), &params, &NoVolumes)
            .expect("empty and optional is absent");
        assert_eq!(
            validate_volume_params(&schema("data", def(true)), &params, &NoVolumes),
            Err(ParamValidationError::VolumeMissing {
                key: "data".to_owned()
            }),
            "empty and required is missing"
        );
    }

    // spec: ACN#kind-parsers
    #[test]
    fn a_text_param_with_a_line_break_is_refused_naming_the_field() {
        let params = supplied("note", json!("one\ntwo"));

        let err = apply_schema(&schema("note", of_kind(ParamKind::Text)), params.clone())
            .expect_err("text is a single line");
        assert_eq!(
            err,
            ParamValidationError::Requirements {
                messages: vec![
                    "note: contains U+000A, which a single-line value cannot hold".to_owned()
                ]
            }
        );

        apply_schema(&schema("note", of_kind(ParamKind::Multiline)), params)
            .expect("multiline takes line breaks");
    }

    // spec: ACN#param-schemas
    // spec: OPS#persistence
    #[test]
    fn a_regex_param_survives_persistence_as_a_regex() {
        let map = apply_schema(
            &schema("pattern", of_kind(ParamKind::Regex)),
            supplied("pattern", json!("a(b)?")),
        )
        .expect("a valid pattern");
        let persisted: ParamMap =
            serde_json::from_str(&serde_json::to_string(&map).unwrap()).unwrap();
        let restored = persisted.to_rhai().expect("restores");
        let regex = restored["pattern"]
            .clone()
            .try_cast::<crate::defs::regex::Regex>()
            .expect("restored as a Regex");
        assert_eq!(regex.pattern(), "a(b)?");
    }

    // spec: ACN#kind-parsers
    #[test]
    fn an_empty_param_is_absent_rather_than_refused_by_its_kind() {
        apply_schema(
            &schema("contact", of_kind(ParamKind::Email)),
            supplied("contact", json!("")),
        )
        .expect("an empty optional email is absent");
    }

    // spec: ACN#kind-parsers
    // spec: VER#semver-versions
    // spec: PURL#accepted-values
    // spec: URL#hostnames
    // spec: URL#components
    // spec: URL#url-origins
    // spec: REX#compiling-a-pattern
    #[test]
    fn a_version_purl_or_url_param_is_checked_by_its_kind() {
        for (kind, refused, accepted, delivered) in [
            (ParamKind::Version, "16.4 bookworm", "16.4", json!("16.4")),
            (
                ParamKind::VersionSemver,
                "2.71",
                "v2.71.3",
                json!("v2.71.3"),
            ),
            (
                ParamKind::Purl,
                "pkg:npm/foo/",
                "pkg://NPM/foo@1.0",
                json!({ "type": "npm", "name": "foo", "version": "1.0" }),
            ),
            (
                ParamKind::Url,
                "https://h:0/",
                "postgres://u:p%40ss@DB.Internal/app",
                json!({
                    "scheme": "postgres",
                    "host": "db.internal",
                    "path": "/app",
                    "username": "u",
                    "password": "p@ss",
                }),
            ),
            (
                ParamKind::UrlOrigin,
                "https://example.org/a",
                "https://Example.org",
                json!({ "scheme": "https", "host": "example.org", "port": 443 }),
            ),
            (
                ParamKind::Hostname,
                "10.0.0.5",
                "Example.org",
                json!("example.org"),
            ),
            (ParamKind::Regex, "a(b", "a(b)?", json!("a(b)?")),
        ] {
            let err = apply_schema(
                &schema("target", of_kind(kind.clone())),
                supplied("target", json!(refused)),
            )
            .expect_err(refused);
            let ParamValidationError::Requirements { messages } = err else {
                panic!("{refused}: expected a requirement failure, got {err:?}");
            };
            assert!(
                messages.len() == 1 && messages[0].starts_with("target: "),
                "{refused}: {messages:?}"
            );

            let map = apply_schema(
                &schema("target", of_kind(kind)),
                supplied("target", json!(accepted)),
            )
            .expect(accepted);
            assert_eq!(map.get("target"), Some(&delivered));
        }
    }

    // spec: ACN#param-delivery
    // spec: IP#family-argument
    // spec: OPS#persistence
    #[test]
    fn an_ip_param_is_checked_by_its_kind_and_delivered_as_its_type() {
        use crate::defs::ip::{Family, IpAddress, IpRange, IpSubnet};

        let mut sch = schema("proxy", of_kind(ParamKind::IpAddress { family: None }));
        sch.extend(schema(
            "allowed",
            of_kind(ParamKind::IpSubnet { family: None }),
        ));
        sch.extend(schema(
            "pool",
            of_kind(ParamKind::IpRange {
                family: Some(Family::V4),
            }),
        ));
        let mut params = supplied("proxy", json!("2001:DB8::1"));
        params.insert("allowed".into(), json!("10.0.0.7"));
        params.insert("pool".into(), json!("10.0.0.0/30"));
        let map = apply_schema(&sch, params).expect("each value is of its kind");
        assert_eq!(map.get("proxy"), Some(&json!("2001:db8::1")));
        assert_eq!(map.get("allowed"), Some(&json!("10.0.0.7/32")));
        assert_eq!(map.get("pool"), Some(&json!("10.0.0.0-10.0.0.3")));

        let persisted: ParamMap =
            serde_json::from_value(serde_json::to_value(&map).unwrap()).unwrap();
        let delivered = persisted.to_rhai().expect("restores");
        let proxy = delivered["proxy"].clone().try_cast::<IpAddress>();
        assert_eq!(proxy.expect("an IpAddress").to_string(), "2001:db8::1");
        let allowed = delivered["allowed"].clone().try_cast::<IpSubnet>();
        assert_eq!(allowed.expect("an IpSubnet").to_string(), "10.0.0.7/32");
        let pool = delivered["pool"].clone().try_cast::<IpRange>();
        assert_eq!(pool.expect("an IpRange").to_string(), "10.0.0.0-10.0.0.3");

        for (key, refused, reason) in [
            ("proxy", "::ffff:10.0.0.1", "IPv4-mapped"),
            ("allowed", "10.0.0.5/24", "10.0.0.0/24"),
            (
                "pool",
                "2001:db8::/64",
                "must be an IPv4 range, not an IPv6 one",
            ),
        ] {
            let mut params = supplied("proxy", json!("10.0.0.1"));
            params.insert("allowed".into(), json!("10.0.0.0/24"));
            params.insert("pool".into(), json!("10.0.0.1-10.0.0.9"));
            params.insert(key.into(), json!(refused));
            let err = apply_schema(&sch, params).expect_err(refused);
            let ParamValidationError::Requirements { messages } = err else {
                panic!("{refused}: expected a requirement failure, got {err:?}");
            };
            assert!(
                messages.len() == 1
                    && messages[0].starts_with(&format!("{key}: "))
                    && messages[0].contains(reason),
                "{refused}: {messages:?}"
            );
        }
    }
}
