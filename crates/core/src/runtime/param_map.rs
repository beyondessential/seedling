//! The param map an action or shell closure receives, in the form it is
//! queued, persisted, logged and replayed in.

use std::collections::BTreeMap;

use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::defs::{
    duration::Duration,
    ip::{IpAddress, IpRange, IpSubnet},
    param_def::ParsedValue,
    regex::{self, Budget},
    version::{SemVer, Version},
};

/// The param map: each value as JSON, with a parser result of a BSL type of
/// its own held as its text, and that type kept beside the map. A purl's
/// components are held as a map whose version is likewise its text.
///
/// The types are kept apart from the values because an undeclared param is
/// passed as supplied, so no shape of JSON value can be reserved to mark a
/// typed one.
// spec: ACN#param-map
// spec: OPS#persistence
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamMap {
    values: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    types: BTreeMap<String, ParsedType>,
}

/// Which parser result of a BSL type of its own a param's value stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParsedType {
    /// A `Version`, held as its text.
    Version,
    /// A `SemVer`, held as its text.
    SemVer,
    /// A purl's components, whose `version`, where it has one, is a
    /// `Version` held as its text.
    Purl,
    /// An `IpAddress`, held as its canonical text.
    IpAddress,
    /// An `IpSubnet`, held as its canonical text.
    IpSubnet,
    /// An `IpRange`, held as its canonical text.
    IpRange,
    /// A `Duration`, held as its text form.
    Duration,
    /// A `Regex`, held as its pattern.
    Regex,
}

impl ParamMap {
    /// A map of plain JSON values, holding no typed value.
    pub fn from_values(values: serde_json::Map<String, Value>) -> Self {
        Self {
            values,
            types: BTreeMap::new(),
        }
    }

    /// A map of `values`, of which those `types` names hold a typed value's
    /// text.
    pub fn with_types(
        values: serde_json::Map<String, Value>,
        types: BTreeMap<String, ParsedType>,
    ) -> Self {
        Self { values, types }
    }

    /// The values, with each typed value as its text.
    pub fn values(&self) -> &serde_json::Map<String, Value> {
        &self.values
    }

    /// The type of each param holding a typed value's text.
    pub fn types(&self) -> &BTreeMap<String, ParsedType> {
        &self.types
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Set `key` to a plain JSON value.
    pub fn insert(&mut self, key: String, value: Value) {
        self.types.remove(&key);
        self.values.insert(key, value);
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.types.remove(key);
        self.values.remove(key)
    }

    /// Set `key` to a kind's parser result.
    // spec: ACN#param-delivery
    pub fn insert_parsed(&mut self, key: String, parsed: ParsedValue) {
        let (value, ty) = match parsed {
            ParsedValue::Value(text) => (Value::String(text), None),
            ParsedValue::Email { local, domain } => (
                serde_json::json!({ "local": local, "domain": domain }),
                None,
            ),
            ParsedValue::Bool(b) => (Value::Bool(b), None),
            ParsedValue::Int(n) => (Value::from(n), None),
            ParsedValue::Version(v) => (Value::String(v.to_string()), Some(ParsedType::Version)),
            ParsedValue::SemVer(v) => (Value::String(v.to_string()), Some(ParsedType::SemVer)),
            ParsedValue::IpAddress(v) => {
                (Value::String(v.to_string()), Some(ParsedType::IpAddress))
            }
            ParsedValue::IpSubnet(v) => (Value::String(v.to_string()), Some(ParsedType::IpSubnet)),
            ParsedValue::IpRange(v) => (Value::String(v.to_string()), Some(ParsedType::IpRange)),
            ParsedValue::Duration(d) => (Value::String(d.to_string()), Some(ParsedType::Duration)),
            url @ (ParsedValue::Url(_) | ParsedValue::UrlOrigin(_)) => (
                crate::defs::json::from_dynamic(url.into_dynamic())
                    .expect("a URL's components are strings and an integer"),
                None,
            ),
            purl @ ParsedValue::Purl(_) => (
                crate::defs::json::from_dynamic(purl.into_dynamic())
                    .expect("a purl's components are strings, arrays, maps and a Version"),
                Some(ParsedType::Purl),
            ),
            ParsedValue::Regex(regex) => (
                Value::String(regex.pattern().to_owned()),
                Some(ParsedType::Regex),
            ),
        };
        self.insert(key.clone(), value);
        if let Some(ty) = ty {
            self.types.insert(key, ty);
        }
    }

    /// The map as the closure receives it, with each typed value restored
    /// from its text.
    ///
    /// Restoring depends on the text alone, so it fails only for a map that
    /// was not built by [`Self::insert_parsed`]; that is reported rather than
    /// delivering the text in the typed value's place.
    // spec: OPS#persistence
    pub fn to_rhai(&self) -> Result<rhai::Map, String> {
        let mut map = rhai::Map::new();
        for (key, value) in &self.values {
            let value = match self.types.get(key) {
                Some(ty) => restore(key, *ty, value)?,
                None => crate::defs::json::to_dynamic(value),
            };
            map.insert(key.as_str().into(), value);
        }
        if let Some(key) = self.types.keys().find(|k| !self.values.contains_key(*k)) {
            return Err(format!(
                "param {key:?} is recorded as a typed value but has no value"
            ));
        }
        Ok(map)
    }
}

// spec: OPS#persistence
fn restore(key: &str, ty: ParsedType, value: &Value) -> Result<Dynamic, String> {
    match ty {
        ParsedType::Version => restore_text(key, value, Version::parse),
        ParsedType::SemVer => restore_text(key, value, SemVer::parse),
        ParsedType::IpAddress => restore_text(key, value, IpAddress::parse),
        ParsedType::IpSubnet => restore_text(key, value, IpSubnet::parse),
        ParsedType::IpRange => restore_text(key, value, IpRange::parse),
        ParsedType::Duration => restore_text(key, value, Duration::parse),
        // spec: OPS#persistence
        ParsedType::Regex => restore_text(key, value, |pattern| {
            regex::compile(pattern, &mut Budget::unlimited())
        }),
        ParsedType::Purl => {
            let Value::Object(components) = value else {
                return Err(format!(
                    "param {key:?} is recorded as a purl but is not a map"
                ));
            };
            let mut purl = crate::defs::json::to_map(components);
            if let Some(text) = components.get("version") {
                purl.insert("version".into(), restore_text(key, text, Version::parse)?);
            }
            Ok(purl.into())
        }
    }
}

/// Restore a typed value from the text `value` holds.
fn restore_text<T: Clone + Send + Sync + 'static>(
    key: &str,
    value: &Value,
    parse: fn(&str) -> Result<T, String>,
) -> Result<Dynamic, String> {
    let Value::String(text) = value else {
        return Err(format!(
            "param {key:?} is recorded as a typed value but is not text"
        ));
    };
    parse(text)
        .map(Dynamic::from)
        .map_err(|e| format!("param {key:?} cannot be restored as its type: {e}"))
}
