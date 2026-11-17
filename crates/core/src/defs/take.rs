//! Convert-or-throw helpers for values crossing the BSL boundary.
//!
//! Every value a script hands to Rust arrives as a [`rhai::Dynamic`], and the
//! defs layer had two ways of consuming one. The checked style — `take_retries`
//! in [`super::container`], `Port::new`, `validate_scale` — converts, names the
//! argument and the actual type in the error, and throws, so rhai reports the
//! script line that is wrong. The coercing style — `into_string().unwrap_or_default()`,
//! `filter_map(try_cast)`, bare `as` casts — turns the same mistake into a
//! *different meaning*: an empty argv element, a dropped `select` criterion
//! that then matches every resource in the app, a `pids_limit` that wraps to 1.
//!
//! These are the checked style, extracted, so the path of least resistance for
//! the next builder is `take_string(...)?` rather than a silent default.
//!
//! Conventions, matching the sites this was extracted from: the message
//! backtick-quotes the argument name as the script author typed it, states what
//! was expected, and reports what arrived — the type for a type mismatch, the
//! value for a range failure. Array elements report their index.

use std::ops::RangeInclusive;

use rhai::{Dynamic, EvalAltResult, FnPtr};

// spec: BSL#arguments — every helper below converts or throws; none
// coerces, defaults, or drops. Per-builder rules keep their own annotations
// at the call sites.

/// Convert to a string, or throw naming `what` and the actual type.
pub fn take_string(what: &str, value: Dynamic) -> Result<String, Box<EvalAltResult>> {
    value
        .into_string()
        .map_err(|t| -> Box<EvalAltResult> { format!("`{what}` must be a string, got {t}").into() })
}

/// Convert to a boolean, or throw naming `what` and the actual type.
pub fn take_bool(what: &str, value: Dynamic) -> Result<bool, Box<EvalAltResult>> {
    value.as_bool().map_err(|t| -> Box<EvalAltResult> {
        format!("`{what}` must be a boolean, got {t}").into()
    })
}

/// Convert to a map, or throw naming `what` and the actual type.
pub fn take_map(what: &str, value: Dynamic) -> Result<rhai::Map, Box<EvalAltResult>> {
    let type_name = value.type_name();
    value
        .try_cast::<rhai::Map>()
        .ok_or_else(|| -> Box<EvalAltResult> {
            format!("`{what}` must be a map, got {type_name}").into()
        })
}

/// Convert to an array, or throw naming `what` and the actual type.
pub fn take_array(what: &str, value: Dynamic) -> Result<rhai::Array, Box<EvalAltResult>> {
    let type_name = value.type_name();
    value
        .try_cast::<rhai::Array>()
        .ok_or_else(|| -> Box<EvalAltResult> {
            format!("`{what}` must be an array, got {type_name}").into()
        })
}

/// Convert to an array of strings, throwing on a non-array or any element
/// that is not a string.
///
/// The element index is in the message: a script author who wrote
/// `command(["nginx", 8080])` gets pointed at the `8080`, not at whatever the
/// container does later with an empty argv element.
pub fn take_string_array(what: &str, value: Dynamic) -> Result<Vec<String>, Box<EvalAltResult>> {
    let array = take_array(what, value)?;
    array
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            item.into_string().map_err(|t| -> Box<EvalAltResult> {
                format!("`{what}` must be an array of strings; element {index} is a {t}").into()
            })
        })
        .collect()
}

/// Convert to an array of a rhai-registered type, throwing on a non-array or
/// any element of the wrong type.
///
/// Used for enum criteria such as `select(#{ types: [ResourceType.Service] })`.
pub fn take_array_of<T: std::any::Any + Clone>(
    what: &str,
    value: Dynamic,
) -> Result<Vec<T>, Box<EvalAltResult>> {
    let array = take_array(what, value)?;
    array
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let type_name = item.type_name();
            item.try_cast::<T>().ok_or_else(|| -> Box<EvalAltResult> {
                format!(
                    "`{what}` must be an array of {}; element {index} is a {type_name}",
                    std::any::type_name::<T>()
                        .rsplit("::")
                        .next()
                        .unwrap_or("values")
                )
                .into()
            })
        })
        .collect()
}

/// Pass `value` through if it is an anonymous closure, or throw naming the
/// argument `arg` of `method` and the function supplied in its place.
///
/// A handler argument is typed as a closure; `Fn("name")` and a bare script
/// function name are named function pointers, not closures.
pub fn take_closure(method: &str, arg: &str, value: FnPtr) -> Result<FnPtr, Box<EvalAltResult>> {
    if value.is_anonymous() {
        return Ok(value);
    }
    Err(format!(
        "`{method}` argument `{arg}` must be a closure, got the function `{}`",
        value.fn_name()
    )
    .into())
}

/// Refuse a map carrying a key other than `allowed`, naming the keys it does
/// not take and the ones it does.
pub fn reject_unknown_keys(
    what: &str,
    map: &rhai::Map,
    allowed: &[&str],
) -> Result<(), Box<EvalAltResult>> {
    let mut unknown: Vec<&str> = map
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !allowed.contains(k))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort_unstable();
    Err(format!(
        "`{what}` has unknown key(s) {}; it takes {}",
        unknown.join(", "),
        allowed.join(", ")
    )
    .into())
}

/// Range-check an integer a builder already received as `i64`, or throw.
///
/// The builders' rhai signatures take `i64` because that is rhai's integer
/// type; the domain is narrower. Without the check, `pids_limit(4294967297)`
/// wraps to 1 and the container cannot start a workload.
pub fn take_int_in_range<T: TryFrom<i64>>(
    what: &str,
    n: i64,
    range: RangeInclusive<i64>,
) -> Result<T, Box<EvalAltResult>> {
    let out_of_range = || -> Box<EvalAltResult> {
        format!(
            "`{what}` must be between {} and {}, got {n}",
            range.start(),
            range.end()
        )
        .into()
    };
    if !range.contains(&n) {
        return Err(out_of_range());
    }
    T::try_from(n).map_err(|_| out_of_range())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_array_names_the_offending_element() {
        let value = Dynamic::from(vec![
            Dynamic::from("nginx".to_owned()),
            Dynamic::from(8080_i64),
        ]);
        let err = take_string_array("command", value).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("`command`"), "{message}");
        assert!(message.contains("element 1"), "{message}");
    }

    #[test]
    fn int_in_range_rejects_out_of_range() {
        let err =
            take_int_in_range::<u32>("pids_limit", i64::from(u32::MAX) + 1, 1..=4_294_967_295)
                .unwrap_err();
        assert!(err.to_string().contains("`pids_limit`"), "{err}");
        assert_eq!(
            take_int_in_range::<u32>("pids_limit", 64, 1..=4_294_967_295).unwrap(),
            64
        );
    }

    #[test]
    fn take_string_reports_the_actual_type() {
        let err = take_string("description", Dynamic::from(42_i64)).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("`description`"), "{message}");
        assert!(message.contains("must be a string"), "{message}");
    }

    // spec: BSL#arguments
    fn assert_names(message: &str, argument: &str, expected: &str, supplied: &str) {
        assert!(message.contains(&format!("`{argument}`")), "{message}");
        assert!(message.contains(expected), "{message}");
        assert!(message.contains(supplied), "{message}");
    }

    // spec: BSL#arguments
    #[test]
    fn scalar_helpers_name_argument_expected_and_supplied_types() {
        let err = take_string("name", Dynamic::from(42_i64)).unwrap_err();
        assert_names(&err.to_string(), "name", "must be a string", "i64");

        let err = take_bool("enabled", Dynamic::from("yes".to_owned())).unwrap_err();
        assert_names(&err.to_string(), "enabled", "must be a boolean", "string");

        let err = take_map("options", Dynamic::from(1_i64)).unwrap_err();
        assert_names(&err.to_string(), "options", "must be a map", "i64");

        let err = take_array("items", Dynamic::from(true)).unwrap_err();
        assert_names(&err.to_string(), "items", "must be an array", "bool");
    }

    // spec: BSL#arguments
    #[test]
    fn array_helpers_name_argument_expected_and_supplied_types() {
        let err = take_string_array("command", Dynamic::from(1_i64)).unwrap_err();
        assert_names(&err.to_string(), "command", "must be an array", "i64");

        let value = Dynamic::from(vec![Dynamic::from("a".to_owned()), Dynamic::from(2_i64)]);
        let err = take_string_array("command", value).unwrap_err();
        assert_names(
            &err.to_string(),
            "command",
            "must be an array of strings",
            "element 1 is a i64",
        );

        let err = take_array_of::<u8>("types", Dynamic::from("x".to_owned())).unwrap_err();
        assert_names(&err.to_string(), "types", "must be an array", "string");

        let value = Dynamic::from(vec![Dynamic::from("x".to_owned())]);
        let err = take_array_of::<u8>("types", value).unwrap_err();
        assert_names(
            &err.to_string(),
            "types",
            "must be an array of u8",
            "element 0 is a string",
        );
    }

    // spec: BSL#arguments
    #[test]
    fn range_errors_name_argument_range_and_value() {
        let err = take_int_in_range::<u8>("scale", 300, 1..=255).unwrap_err();
        assert_names(&err.to_string(), "scale", "between 1 and 255", "got 300");

        let err = take_int_in_range::<u8>("scale", 300, 0..=400).unwrap_err();
        assert_names(&err.to_string(), "scale", "between 0 and 400", "got 300");
    }

    // spec: BSL#arguments
    #[test]
    fn unknown_keys_name_argument_unknown_keys_and_accepted_keys() {
        let mut map = rhai::Map::new();
        map.insert("descripton".into(), Dynamic::from(1_i64));
        let err = reject_unknown_keys("exported options", &map, &["description"]).unwrap_err();
        assert_names(
            &err.to_string(),
            "exported options",
            "it takes description",
            "descripton",
        );
    }
}
