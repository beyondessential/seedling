//! JSON values handed to a script as BSL values.

use rhai::Dynamic;
use serde_json::Value;

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
