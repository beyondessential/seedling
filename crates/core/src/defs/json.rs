//! JSON values handed to a script as BSL values, and back.

use rhai::Dynamic;
use serde_json::Value;

use super::{
    duration::Duration,
    ip::{IpAddress, IpRange, IpSubnet},
    regex::Regex,
    version::{SemVer, Version},
};

/// Convert a JSON value to the BSL value a script reads, keeping integers and
/// floating-point numbers apart and reading `null` as the unit value.
pub fn to_dynamic(v: &Value) -> Dynamic {
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
        Value::Object(map) => Dynamic::from(to_map(map)),
        Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(to_dynamic).collect();
            Dynamic::from(rhai_arr)
        }
    }
}

/// Convert a JSON object to a BSL map.
pub fn to_map(map: &serde_json::Map<String, Value>) -> rhai::Map {
    map.iter()
        .map(|(k, v)| (k.as_str().into(), to_dynamic(v)))
        .collect()
}

/// Convert a BSL value to JSON, reading the unit value as `null`, taking a
/// `Version`, `SemVer` or `Duration` as its text, an `IpAddress`,
/// `IpSubnet` or `IpRange` as its canonical text, and a `Regex` as its
/// pattern.
pub fn from_dynamic(v: Dynamic) -> Result<Value, String> {
    if v.is_unit() {
        Ok(Value::Null)
    } else if v.is_bool() {
        Ok(Value::Bool(v.cast::<bool>()))
    } else if v.is_int() {
        Ok(Value::from(v.cast::<i64>()))
    } else if v.is_float() {
        // spec: BSL#arguments
        // A non-finite number has no JSON form; null would be a different
        // value than the script gave.
        let f = v.cast::<f64>();
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| format!("value {f} is not a finite number"))
    } else if v.is_string() {
        v.into_string()
            .map(Value::String)
            .map_err(|t| format!("value is of unsupported type {t}"))
    } else if let Some(version) = v.clone().try_cast::<Version>() {
        // spec: ACN#action-invocation
        Ok(Value::String(version.to_string()))
    } else if let Some(version) = v.clone().try_cast::<SemVer>() {
        // spec: ACN#action-invocation
        Ok(Value::String(version.to_string()))
    } else if let Some(addr) = v.clone().try_cast::<IpAddress>() {
        // spec: ACN#action-invocation
        Ok(Value::String(addr.to_string()))
    } else if let Some(subnet) = v.clone().try_cast::<IpSubnet>() {
        // spec: ACN#action-invocation
        Ok(Value::String(subnet.to_string()))
    } else if let Some(range) = v.clone().try_cast::<IpRange>() {
        // spec: ACN#action-invocation
        Ok(Value::String(range.to_string()))
    } else if let Some(duration) = v.clone().try_cast::<Duration>() {
        // spec: ACN#action-invocation
        Ok(Value::String(duration.to_string()))
    } else if let Some(regex) = v.clone().try_cast::<Regex>() {
        // spec: ACN#action-invocation
        Ok(Value::String(regex.pattern().to_owned()))
    } else if v.is_array() {
        v.cast::<rhai::Array>()
            .into_iter()
            .map(from_dynamic)
            .collect::<Result<_, _>>()
            .map(Value::Array)
    } else if v.is_map() {
        from_map(v.cast()).map(Value::Object)
    } else {
        Err(format!("value is of unsupported type {}", v.type_name()))
    }
}

/// Convert a BSL map to a JSON object, as [`from_dynamic`] converts its
/// values.
pub fn from_map(map: rhai::Map) -> Result<serde_json::Map<String, Value>, String> {
    map.into_iter()
        .map(|(k, v)| Ok((k.to_string(), from_dynamic(v)?)))
        .collect()
}
