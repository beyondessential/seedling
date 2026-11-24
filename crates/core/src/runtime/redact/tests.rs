use super::*;

fn holding(values: &[&str]) -> SensitiveValues {
    let mut sensitive = SensitiveValues::default();
    for value in values {
        sensitive.insert(value);
    }
    sensitive
}

// spec: SEC#withheld-values
#[test]
fn text_with_no_sensitive_value_is_borrowed() {
    let masked = holding(&["hunter2"]).mask("nothing to see");
    assert!(matches!(masked, Cow::Borrowed("nothing to see")));
}

// spec: SEC#withheld-values
#[test]
fn each_occurrence_is_masked_in_place() {
    assert_eq!(
        holding(&["hunter2"]).mask("a=hunter2 b=hunter2;"),
        "a=<redacted> b=<redacted>;"
    );
}

// spec: SEC#withheld-values
#[test]
fn a_value_equal_to_a_sensitive_value_is_masked_whole() {
    assert_eq!(holding(&["hunter2"]).mask("hunter2"), REDACTED);
}

// spec: SEC#withheld-values
#[test]
fn overlapping_and_adjacent_occurrences_mask_as_one_stretch() {
    let sensitive = holding(&["abcd", "cdef"]);
    assert_eq!(sensitive.mask("xabcdefx"), "x<redacted>x");
    assert_eq!(sensitive.mask("xabcdcdefx"), "x<redacted>x");
}

// spec: SEC#withheld-values
#[test]
fn a_value_among_multibyte_text_is_masked_on_character_boundaries() {
    let sensitive = holding(&["mōtā"]);
    assert_eq!(
        sensitive.mask("kia ora mōtā, tēnā"),
        "kia ora <redacted>, tēnā"
    );
}

// spec: SEC#withheld-values
#[test]
fn masking_masked_text_again_changes_nothing() {
    let sensitive = holding(&["e", "hunter2"]);
    let once = sensitive.mask("see hunter2").into_owned();
    assert_eq!(once, "s<redacted> <redacted>");
    assert!(matches!(sensitive.mask(&once), Cow::Borrowed(_)));
    assert!(!sensitive.found_in(REDACTED));
}

// spec: SEC#withheld-values
#[test]
fn a_value_overlapping_a_marker_is_masked_with_it() {
    let sensitive = holding(&["x<redacted>y", "ed> a"]);
    assert_eq!(sensitive.mask("q x<redacted>y q"), "q <redacted> q");
    assert_eq!(sensitive.mask("<redacted> ab"), "<redacted>b");
}

// spec: SEC#withheld-values
#[test]
fn a_marker_forming_a_value_with_its_neighbours_is_masked_again() {
    let sensitive = holding(&["pw", "> z"]);
    let masked = sensitive.mask("pw z");
    assert_eq!(masked, "<redacted>");
    assert!(!sensitive.found_in(&masked));
}

// spec: SEC#sensitive-value
#[test]
fn an_empty_value_is_not_sensitive() {
    let sensitive = holding(&[""]);
    assert!(sensitive.is_empty());
    assert!(!sensitive.found_in("anything"));
}

#[test]
fn extending_a_set_holds_both() {
    let mut sensitive = holding(&["one"]);
    sensitive.extend(&holding(&["two"]));
    assert!(sensitive.found_in("one") && sensitive.found_in("two"));
    assert_eq!(sensitive.len(), 2);
}

// spec: SEC#matching
#[test]
fn a_scope_holds_its_apps_values_and_its_own() {
    let scope = Scope::new(holding(&["app-secret"]));
    scope.insert("op-secret");
    assert_eq!(
        scope.values().mask("app-secret op-secret other"),
        "<redacted> <redacted> other"
    );
}

// spec: SEC#sensitive-value
#[test]
fn a_scope_holds_secret_params_as_text() {
    let scope = Scope::empty();
    let params: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
        serde_json::json!({ "token": "t0ken", "pin": 4321, "label": "plain" }),
    )
    .unwrap();
    scope.insert_secret_params(&params, |name| name != "label");
    let values = scope.values();
    assert!(values.found_in("t0ken") && values.found_in("4321"));
    assert!(!values.found_in("plain"));
}

// spec: SEC#responses-and-events
#[test]
fn a_document_masks_each_strings_value_except_identifiers() {
    let mut document = serde_json::json!({
        "name": "web-hunter2",
        "note": "built with hunter2",
        "items": [{ "detail": "hunter2 again", "instance_id": "hunter2-1" }],
        "elsewhere": { "app": "other", "detail": "hunter2 kept", "secret": "Kea-7" },
    });
    let mut asked = Vec::new();
    mask_document(&mut document, Some("demo"), &mut |app: &str| {
        asked.push(app.to_owned());
        Ok::<_, ()>(match app {
            "demo" => holding(&["hunter2"]),
            _ => holding(&["Kea-7"]),
        })
    })
    .unwrap();
    assert_eq!(
        document,
        serde_json::json!({
            "name": "web-hunter2",
            "note": "built with <redacted>",
            "items": [{ "detail": "<redacted> again", "instance_id": "hunter2-1" }],
            "elsewhere": { "app": "other", "detail": "hunter2 kept", "secret": "<redacted>" },
        })
    );
    assert!(asked.iter().any(|a| a == "other"));
}

// spec: SEC#responses-and-events
#[test]
fn a_document_masks_a_member_name_holding_a_value() {
    let mut document = serde_json::json!({ "env": { "TOKEN_hunter2": "set", "PLAIN": "x" } });
    mask_document(&mut document, Some("demo"), &mut |_: &str| {
        Ok::<_, ()>(holding(&["hunter2"]))
    })
    .unwrap();
    assert_eq!(
        document,
        serde_json::json!({ "env": { "TOKEN_<redacted>": "set", "PLAIN": "x" } })
    );
}

// spec: SEC#responses-and-events
#[test]
fn a_document_about_no_app_is_left_as_it_is() {
    let mut document = serde_json::json!({ "note": "hunter2" });
    mask_document(&mut document, None, &mut |_: &str| {
        Ok::<_, ()>(holding(&["hunter2"]))
    })
    .unwrap();
    assert_eq!(document, serde_json::json!({ "note": "hunter2" }));
}

// spec: SEC#redaction
#[test]
fn a_value_is_found_wherever_it_sits() {
    let mut values = SensitiveValues::default();
    values.insert(b"\0secret");
    values.insert(b"x");
    for text in [&b"\0secret"[..], b"a\0secret", b"\0secretb", b"..x.."] {
        assert!(values.found_in_bytes(text), "{text:?}");
    }
    for text in [&b"secret"[..], b"\0secre", b"\0\0secre", b"", b"yz"] {
        assert!(!values.found_in_bytes(text), "{text:?}");
    }
}

// spec: SEC#redaction
#[test]
fn a_long_value_is_found_in_a_long_text_in_one_pass() {
    let form: Vec<u8> = (0..1 << 19).map(|i: u32| (i * 7 % 251) as u8).collect();
    let mut text = vec![b'.'; 1 << 19];
    text.extend(&form);
    let mut values = SensitiveValues::default();
    values.insert(&form);
    assert!(values.found_in_bytes(&text));
    text.pop();
    assert!(!values.found_in_bytes(&text));
}
