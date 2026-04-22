use std::{fmt, str::FromStr};

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

/// Environment variable names that are unsafe to set from BSL because they
/// influence the loader or change the container's runtime search paths.
// l[impl container.env.validation]
pub const FORBIDDEN_ENV_NAMES: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "LD_PROFILE",
];

/// Rejection reason produced when constructing an [`EnvironmentVarName`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidEnvName {
    Empty,
    LeadingDigit,
    InvalidCharacter(char),
    Forbidden,
}

impl fmt::Display for InvalidEnvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("environment variable name must not be empty"),
            Self::LeadingDigit => {
                f.write_str("environment variable name must not start with a digit")
            }
            Self::InvalidCharacter(c) => write!(
                f,
                "environment variable name contains invalid character {c:?}: \
                 must use only ASCII letters, digits, and underscores"
            ),
            Self::Forbidden => {
                f.write_str("environment variable name is forbidden (loader-sensitive)")
            }
        }
    }
}

impl std::error::Error for InvalidEnvName {}

// l[impl container.env.validation]
fn validate_env_name(s: &str) -> Result<(), InvalidEnvName> {
    if s.is_empty() {
        return Err(InvalidEnvName::Empty);
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(InvalidEnvName::LeadingDigit);
    }
    if let Some(c) = s.chars().find(|c| !c.is_ascii_alphanumeric() && *c != '_') {
        return Err(InvalidEnvName::InvalidCharacter(c));
    }
    if FORBIDDEN_ENV_NAMES.contains(&s) {
        return Err(InvalidEnvName::Forbidden);
    }
    Ok(())
}

/// Name of an environment variable passed to a container.
///
/// Unlike most name newtypes in this crate, environment variable names do not
/// follow the `bsl.name` rules — they follow the POSIX-style
/// letter/digit/underscore convention, must not start with a digit, and must
/// not clobber loader-sensitive names (see [`FORBIDDEN_ENV_NAMES`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct EnvironmentVarName(CompactString);

impl EnvironmentVarName {
    pub fn new(s: impl AsRef<str>) -> Result<Self, InvalidEnvName> {
        let s = s.as_ref();
        validate_env_name(s)?;
        Ok(Self(CompactString::from(s)))
    }

    /// Wrap `s` without re-running validation. The caller guarantees `s`
    /// already satisfies the rules.
    pub fn new_unchecked(s: impl Into<CompactString>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn into_string(self) -> String {
        self.0.into_string()
    }
}

impl fmt::Display for EnvironmentVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl AsRef<str> for EnvironmentVarName {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl std::borrow::Borrow<str> for EnvironmentVarName {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for EnvironmentVarName {
    type Err = InvalidEnvName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl PartialEq<str> for EnvironmentVarName {
    fn eq(&self, other: &str) -> bool {
        self.0.as_str() == other
    }
}

impl PartialEq<&str> for EnvironmentVarName {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_str() == *other
    }
}

impl<'de> Deserialize<'de> for EnvironmentVarName {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = CompactString::deserialize(d)?;
        validate_env_name(s.as_str()).map_err(serde::de::Error::custom)?;
        Ok(Self(s))
    }
}

/// Rejection reason for [`EnvVar::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidEnvVar {
    Name(InvalidEnvName),
    ValueContainsNull,
}

impl fmt::Display for InvalidEnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(e) => e.fmt(f),
            Self::ValueContainsNull => {
                f.write_str("environment variable value must not contain null bytes")
            }
        }
    }
}

impl std::error::Error for InvalidEnvVar {}

impl From<InvalidEnvName> for InvalidEnvVar {
    fn from(e: InvalidEnvName) -> Self {
        Self::Name(e)
    }
}

/// A `NAME=value` environment-variable pair as passed to a container.
///
/// The name is a validated [`EnvironmentVarName`]; the value is a free-form
/// string subject only to the POSIX "no null byte" constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: EnvironmentVarName,
    pub value: String,
}

impl EnvVar {
    /// Construct a validated pair. The name is parsed through
    /// [`EnvironmentVarName::new`] and the value is checked for null bytes.
    // l[impl container.env.validation]
    pub fn new(name: impl AsRef<str>, value: impl Into<String>) -> Result<Self, InvalidEnvVar> {
        let name = EnvironmentVarName::new(name)?;
        let value = value.into();
        if value.contains('\0') {
            return Err(InvalidEnvVar::ValueContainsNull);
        }
        Ok(Self { name, value })
    }
}

impl fmt::Display for EnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_conventional_env_names() {
        EnvironmentVarName::new("FOO").unwrap();
        EnvironmentVarName::new("APP_VERSION").unwrap();
        EnvironmentVarName::new("_PRIVATE").unwrap();
        EnvironmentVarName::new("x9").unwrap();
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(EnvironmentVarName::new(""), Err(InvalidEnvName::Empty));
    }

    #[test]
    fn rejects_leading_digit() {
        assert_eq!(
            EnvironmentVarName::new("9LIVES"),
            Err(InvalidEnvName::LeadingDigit)
        );
    }

    #[test]
    fn rejects_invalid_character() {
        assert!(matches!(
            EnvironmentVarName::new("APP-VERSION"),
            Err(InvalidEnvName::InvalidCharacter('-'))
        ));
    }

    #[test]
    fn rejects_forbidden_name() {
        assert_eq!(
            EnvironmentVarName::new("PATH"),
            Err(InvalidEnvName::Forbidden)
        );
        assert_eq!(
            EnvironmentVarName::new("LD_PRELOAD"),
            Err(InvalidEnvName::Forbidden)
        );
    }

    #[test]
    fn env_var_pair_validates_both_sides() {
        let ok = EnvVar::new("RUST_LOG", "debug").unwrap();
        assert_eq!(ok.to_string(), "RUST_LOG=debug");

        assert!(matches!(
            EnvVar::new("PATH", "anything"),
            Err(InvalidEnvVar::Name(InvalidEnvName::Forbidden))
        ));
        assert!(matches!(
            EnvVar::new("OK", "a\0b"),
            Err(InvalidEnvVar::ValueContainsNull)
        ));
    }

    #[test]
    fn serde_transparent_for_name() {
        let n = EnvironmentVarName::new("FOO").unwrap();
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"FOO\"");
        let back: EnvironmentVarName = serde_json::from_str("\"FOO\"").unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn deserialize_rejects_invalid_name() {
        let r: Result<EnvironmentVarName, _> = serde_json::from_str("\"9BAD\"");
        assert!(r.is_err());
    }

    #[test]
    fn env_var_round_trips() {
        let v = EnvVar::new("FOO", "bar").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: EnvVar = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
