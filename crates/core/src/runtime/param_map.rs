//! The param map an action or shell closure receives, in the form it is
//! queued, persisted, logged and replayed in.

use std::collections::BTreeMap;

use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::defs::{
    param_def::ParsedValue,
    version::{SemVer, Version},
};

/// The param map: each value as JSON, with a version parser result held as
/// its text and its type kept beside the map.
///
/// The types are kept apart from the values because an undeclared param is
/// passed as supplied, so no shape of JSON value can be reserved to mark a
/// version.
// spec: ACN#param-map
// spec: OPS#persistence
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamMap {
    values: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    versions: BTreeMap<String, VersionType>,
}

/// Which version parser result a param's text stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionType {
    Version,
    SemVer,
}

impl ParamMap {
    /// A map of plain JSON values, holding no version.
    pub fn from_values(values: serde_json::Map<String, Value>) -> Self {
        Self {
            values,
            versions: BTreeMap::new(),
        }
    }

    /// A map of `values`, of which those `versions` names hold a version's
    /// text.
    pub fn with_versions(
        values: serde_json::Map<String, Value>,
        versions: BTreeMap<String, VersionType>,
    ) -> Self {
        Self { values, versions }
    }

    /// The values, with each version as its text.
    pub fn values(&self) -> &serde_json::Map<String, Value> {
        &self.values
    }

    /// The type of each param holding a version's text.
    pub fn versions(&self) -> &BTreeMap<String, VersionType> {
        &self.versions
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Set `key` to a plain JSON value.
    pub fn insert(&mut self, key: String, value: Value) {
        self.versions.remove(&key);
        self.values.insert(key, value);
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.versions.remove(key);
        self.values.remove(key)
    }

    /// Set `key` to a kind's parser result.
    // spec: ACN#param-delivery
    pub fn insert_parsed(&mut self, key: String, parsed: ParsedValue) {
        let (value, version) = match parsed {
            ParsedValue::Value(text) => (Value::String(text), None),
            ParsedValue::Email { local, domain } => (
                serde_json::json!({ "local": local, "domain": domain }),
                None,
            ),
            ParsedValue::Bool(b) => (Value::Bool(b), None),
            ParsedValue::Int(n) => (Value::from(n), None),
            ParsedValue::Version(v) => (Value::String(v.to_string()), Some(VersionType::Version)),
            ParsedValue::SemVer(v) => (Value::String(v.to_string()), Some(VersionType::SemVer)),
        };
        self.insert(key.clone(), value);
        if let Some(version) = version {
            self.versions.insert(key, version);
        }
    }

    /// The map as the closure receives it, with each version restored from
    /// its text.
    ///
    /// Restoring depends on the text alone, so it fails only for a map that
    /// was not built by [`Self::insert_parsed`]; that is reported rather than
    /// delivering the text in the version's place.
    // spec: OPS#persistence
    pub fn to_rhai(&self) -> Result<rhai::Map, String> {
        let mut map = rhai::Map::new();
        for (key, value) in &self.values {
            let value = match self.versions.get(key) {
                Some(version) => restore_version(key, *version, value)?,
                None => json_value_to_rhai_dynamic(value),
            };
            map.insert(key.as_str().into(), value);
        }
        if let Some(key) = self.versions.keys().find(|k| !self.values.contains_key(*k)) {
            return Err(format!("param {key:?} is recorded as a version but has no value"));
        }
        Ok(map)
    }
}

// spec: OPS#persistence
fn restore_version(key: &str, version: VersionType, value: &Value) -> Result<Dynamic, String> {
    let Value::String(text) = value else {
        return Err(format!("param {key:?} is recorded as a version but is not text"));
    };
    let restored = match version {
        VersionType::Version => Version::parse(text).map(Dynamic::from),
        VersionType::SemVer => SemVer::parse(text).map(Dynamic::from),
    };
    restored.map_err(|e| format!("param {key:?} cannot be restored as a version: {e}"))
}

pub fn json_value_to_rhai_dynamic(v: &Value) -> Dynamic {
    match v {
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Bool(b) => Dynamic::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                Dynamic::UNIT
            }
        }
        Value::Null => Dynamic::UNIT,
        Value::Object(map) => {
            let rhai_map: rhai::Map = map
                .iter()
                .map(|(k, v)| (k.as_str().into(), json_value_to_rhai_dynamic(v)))
                .collect();
            Dynamic::from(rhai_map)
        }
        Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(json_value_to_rhai_dynamic).collect();
            Dynamic::from(rhai_arr)
        }
    }
}
