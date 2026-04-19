use std::collections::BTreeMap;

use crate::defs::install::{InstallRequirementDef, InstallRequirementKind};

use super::{is_strong_password, is_valid_email, validate_requirements};

// i[verify action.invoke.install.validation]
#[test]
fn valid_email_basic() {
    assert!(is_valid_email("user@example.com"));
    assert!(is_valid_email("a@b.co"));
    assert!(is_valid_email("user+tag@sub.example.org"));
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_no_at() {
    assert!(!is_valid_email("notanemail"));
    assert!(!is_valid_email(""));
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_no_dot_in_domain() {
    assert!(!is_valid_email("user@nodot"));
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_empty_local() {
    assert!(!is_valid_email("@example.com"));
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_domain_starts_with_dot() {
    assert!(!is_valid_email("user@.example.com"));
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_domain_ends_with_dot() {
    assert!(!is_valid_email("user@example.com."));
}

// i[verify action.invoke.install.validation]
#[test]
fn strong_password_accepted() {
    assert!(is_strong_password("correct-horse-battery-staple-42!"));
    assert!(is_strong_password("Tr0ub4dor&3xtraL0ng"));
}

// i[verify action.invoke.install.validation]
#[test]
fn weak_password_rejected() {
    assert!(!is_strong_password("password"));
    assert!(!is_strong_password("123456"));
    assert!(!is_strong_password("abc"));
}

fn schema(fields: &[(&str, InstallRequirementDef)]) -> BTreeMap<String, InstallRequirementDef> {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn req(
    kind: InstallRequirementKind,
    required: bool,
    default: Option<&str>,
) -> InstallRequirementDef {
    InstallRequirementDef {
        kind,
        required,
        default_value: default.map(|s| s.to_owned()),
        description: None,
    }
}

// i[verify action.invoke.install.validation]
#[test]
fn empty_schema_empty_submitted_ok() {
    let result = validate_requirements(&BTreeMap::new(), &BTreeMap::new());
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

// i[verify action.invoke.install.validation]
#[test]
fn required_field_missing_returns_error() {
    let s = schema(&[("email", req(InstallRequirementKind::Text, true, None))]);
    let result = validate_requirements(&s, &BTreeMap::new());
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("email"));
}

// i[verify action.invoke.install.validation]
#[test]
fn required_field_with_default_filled_in() {
    let s = schema(&[(
        "site",
        req(InstallRequirementKind::Text, true, Some("default-site")),
    )]);
    let result = validate_requirements(&s, &BTreeMap::new());
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap().get("site").map(String::as_str),
        Some("default-site")
    );
}

// i[verify action.invoke.install.validation]
#[test]
fn optional_field_absent_is_ok() {
    let s = schema(&[("note", req(InstallRequirementKind::Text, false, None))]);
    let result = validate_requirements(&s, &BTreeMap::new());
    assert!(result.is_ok());
}

// i[verify action.invoke.install.validation]
#[test]
fn invalid_email_field_returns_error() {
    let s = schema(&[("email", req(InstallRequirementKind::Email, true, None))]);
    let mut submitted = BTreeMap::new();
    submitted.insert("email".to_owned(), "notanemail".to_owned());
    let result = validate_requirements(&s, &submitted);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("email"));
}

// i[verify action.invoke.install.validation]
#[test]
fn valid_email_field_passes() {
    let s = schema(&[("email", req(InstallRequirementKind::Email, true, None))]);
    let mut submitted = BTreeMap::new();
    submitted.insert("email".to_owned(), "user@example.com".to_owned());
    let result = validate_requirements(&s, &submitted);
    assert!(result.is_ok());
}

// i[verify action.invoke.install.validation]
#[test]
fn weak_password_field_returns_error() {
    let s = schema(&[("pw", req(InstallRequirementKind::Password, true, None))]);
    let mut submitted = BTreeMap::new();
    submitted.insert("pw".to_owned(), "password".to_owned());
    let result = validate_requirements(&s, &submitted);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("pw"));
}

// i[verify action.invoke.install.validation]
#[test]
fn strong_password_field_passes() {
    let s = schema(&[("pw", req(InstallRequirementKind::Password, true, None))]);
    let mut submitted = BTreeMap::new();
    submitted.insert(
        "pw".to_owned(),
        "correct-horse-battery-staple-42!".to_owned(),
    );
    let result = validate_requirements(&s, &submitted);
    assert!(result.is_ok());
}

// i[verify action.invoke.install.validation]
#[test]
fn weak_password_kind_always_passes() {
    let s = schema(&[("pw", req(InstallRequirementKind::WeakPassword, true, None))]);
    let mut submitted = BTreeMap::new();
    submitted.insert("pw".to_owned(), "password".to_owned());
    let result = validate_requirements(&s, &submitted);
    assert!(result.is_ok());
}

// i[verify action.invoke.install.validation]
#[test]
fn multiple_errors_collected() {
    let s = schema(&[
        ("email", req(InstallRequirementKind::Email, true, None)),
        ("name", req(InstallRequirementKind::Text, true, None)),
    ]);
    let result = validate_requirements(&s, &BTreeMap::new());
    assert!(result.is_err());
    let msg = result.unwrap_err().message;
    assert!(msg.contains("email") || msg.contains("name"));
}
