use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Canonical name of a seedling application.
///
/// App names follow the [`bsl.name`](../../docs/spec/language.md) rules:
/// ASCII alphanumeric with hyphens, 3-63 characters, starting with a letter,
/// not starting or ending with a hyphen, and not starting with an underscore.
///
/// Construct with [`AppName::new`] to validate. Use [`AppName::new_unchecked`]
/// only when reading from a trusted source (e.g. a SQLite row written after a
/// prior validation).
///
/// [`AppName::default`] yields an empty placeholder used only as a pre-script
/// seed for [`AppDef::default`]; it is invalid by the name rules and must be
/// overwritten by the BSL `app.name(...)` call before anything inspects it.
// l[impl bsl.name]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct AppName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidAppName {
    LeadingUnderscore,
    Malformed(String),
}

impl AppName {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidAppName> {
        let s = s.into();
        if s.starts_with('_') {
            return Err(InvalidAppName::LeadingUnderscore);
        }
        if !is_valid_name(&s) {
            return Err(InvalidAppName::Malformed(s));
        }
        Ok(Self(s))
    }

    /// Construct an `AppName` without re-running validation. The caller
    /// guarantees that `s` already satisfies the name rules — typically
    /// because it was read from the database or another component that
    /// validated it on the way in.
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AppName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for AppName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for AppName {
    type Err = InvalidAppName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_owned())
    }
}

impl From<AppName> for String {
    fn from(n: AppName) -> Self {
        n.0
    }
}

impl PartialEq<str> for AppName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for AppName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<AppName> for str {
    fn eq(&self, other: &AppName) -> bool {
        self == other.0
    }
}

impl PartialEq<String> for AppName {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl<'de> Deserialize<'de> for AppName {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::ToSql for AppName {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::types::FromSql for AppName {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        // Values in SQLite were written after validation on the way in, so
        // bypass re-validation here (matches the contract of `new_unchecked`).
        String::column_result(value).map(Self)
    }
}

impl fmt::Display for InvalidAppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeadingUnderscore => f.write_str("name must not start with an underscore"),
            Self::Malformed(n) => write!(
                f,
                "invalid name '{n}': must match ^[a-zA-Z][a-zA-Z0-9-]{{1,60}}[a-zA-Z0-9]$"
            ),
        }
    }
}

impl std::error::Error for InvalidAppName {}

fn is_valid_name(name: &str) -> bool {
    name.len() >= 3
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
        && name[1..name.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_names() {
        AppName::new("web").unwrap();
        AppName::new("my-app").unwrap();
        AppName::new("app-123").unwrap();
        AppName::new("abc").unwrap();
    }

    #[test]
    fn rejects_too_short() {
        assert!(matches!(
            AppName::new("ab"),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn rejects_too_long() {
        assert!(matches!(
            AppName::new("a".repeat(64)),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn rejects_leading_hyphen() {
        assert!(matches!(
            AppName::new("-app"),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn rejects_trailing_hyphen() {
        assert!(matches!(
            AppName::new("app-"),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn rejects_leading_underscore_specifically() {
        assert!(matches!(
            AppName::new("_app"),
            Err(InvalidAppName::LeadingUnderscore)
        ));
    }

    #[test]
    fn rejects_leading_digit() {
        assert!(matches!(
            AppName::new("1app"),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn rejects_non_ascii() {
        assert!(matches!(
            AppName::new("café"),
            Err(InvalidAppName::Malformed(_))
        ));
    }

    #[test]
    fn serde_transparent_as_string() {
        let a = AppName::new("my-app").unwrap();
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, "\"my-app\"");
        let b: AppName = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let res: Result<AppName, _> = serde_json::from_str("\"_nope\"");
        assert!(res.is_err());
    }

    #[test]
    fn new_longest_accepted() {
        AppName::new(format!("a{}", "b".repeat(62))).unwrap();
    }
}
