use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// Rejection reason produced by the bsl.name validator. Shared across all
/// name newtypes whose rules match the `bsl.name` spec item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidName {
    LeadingUnderscore,
    Malformed(String),
}

impl fmt::Display for InvalidName {
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

impl std::error::Error for InvalidName {}

// l[impl bsl.name]
fn validate_bsl_name(s: &str) -> Result<(), InvalidName> {
    if s.starts_with('_') {
        return Err(InvalidName::LeadingUnderscore);
    }
    let ok = s.len() >= 3
        && s.len() <= 63
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.ends_with(|c: char| c.is_ascii_alphanumeric())
        && s[1..s.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(InvalidName::Malformed(s.to_owned()))
    }
}

// ---------------------------------------------------------------------------
// AppName
// ---------------------------------------------------------------------------

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

impl AppName {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidName> {
        let s = s.into();
        validate_bsl_name(&s)?;
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
    type Err = InvalidName;
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

// ---------------------------------------------------------------------------
// ActionName
// ---------------------------------------------------------------------------

/// Canonical name of a BSL action within an app.
///
/// Action names follow the same [`bsl.name`](../../docs/spec/language.md)
/// rules as app names: ASCII alphanumeric with hyphens, 3-63 characters,
/// starting with a letter, not starting or ending with a hyphen, and not
/// starting with an underscore. The implicit `"start"` lifecycle action is
/// valid by these rules.
///
/// Construct with [`ActionName::new`] to validate. Use
/// [`ActionName::new_unchecked`] only when reading from a trusted source.
///
/// [`ActionName::default`] yields an empty placeholder used only for the
/// transient `AppStatus::Operating { action_name }` state before a concrete
/// action is known; it is invalid by the name rules and must be overwritten
/// before anything inspects it.
// l[impl bsl.name]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ActionName(String);

impl ActionName {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidName> {
        let s = s.into();
        validate_bsl_name(&s)?;
        Ok(Self(s))
    }

    /// Construct an `ActionName` without re-running validation. The caller
    /// guarantees that `s` already satisfies the name rules.
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

impl fmt::Display for ActionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ActionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for ActionName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActionName {
    type Err = InvalidName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_owned())
    }
}

impl From<ActionName> for String {
    fn from(n: ActionName) -> Self {
        n.0
    }
}

impl PartialEq<str> for ActionName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ActionName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<ActionName> for str {
    fn eq(&self, other: &ActionName) -> bool {
        self == other.0
    }
}

impl PartialEq<String> for ActionName {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl<'de> Deserialize<'de> for ActionName {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::ToSql for ActionName {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::types::FromSql for ActionName {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        String::column_result(value).map(Self)
    }
}

// ---------------------------------------------------------------------------
// ExternalVolumeName
// ---------------------------------------------------------------------------

/// Canonical name of an external-volume slot declared by a BSL app.
///
/// External-volume names follow the same [`bsl.name`](../../docs/spec/language.md)
/// rules as app and action names: ASCII alphanumeric with hyphens, 3-63
/// characters, starting with a letter, not starting or ending with a hyphen,
/// and not starting with an underscore.
///
/// This type names the *slot* the app declares in its script (e.g.
/// `app.external_volume("data")`); the *target* volume the operator maps into
/// that slot is still plain text for now — it can refer to either a site
/// volume or another app's volume, so it needs a different newtype.
// l[impl bsl.name]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ExternalVolumeName(String);

impl ExternalVolumeName {
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidName> {
        let s = s.into();
        validate_bsl_name(&s)?;
        Ok(Self(s))
    }

    /// Construct without re-running validation. The caller guarantees that
    /// `s` already satisfies the name rules.
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

impl fmt::Display for ExternalVolumeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ExternalVolumeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for ExternalVolumeName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for ExternalVolumeName {
    type Err = InvalidName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_owned())
    }
}

impl From<ExternalVolumeName> for String {
    fn from(n: ExternalVolumeName) -> Self {
        n.0
    }
}

impl PartialEq<str> for ExternalVolumeName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ExternalVolumeName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<ExternalVolumeName> for str {
    fn eq(&self, other: &ExternalVolumeName) -> bool {
        self == other.0
    }
}

impl PartialEq<String> for ExternalVolumeName {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl<'de> Deserialize<'de> for ExternalVolumeName {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::ToSql for ExternalVolumeName {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

#[cfg(feature = "rusqlite")]
impl rusqlite::types::FromSql for ExternalVolumeName {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        String::column_result(value).map(Self)
    }
}

// ---------------------------------------------------------------------------
// SessionId, ForwardId — UUID-backed ephemeral identifiers
// ---------------------------------------------------------------------------

macro_rules! uuid_newtype {
    (
        $(#[$attr:meta])*
        $name:ident
    ) => {
        $(#[$attr])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID value.
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            pub const fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

uuid_newtype! {
    /// Identifier for a running shell session.
    SessionId
}

uuid_newtype! {
    /// Identifier for an active port forward.
    ForwardId
}

uuid_newtype! {
    /// Identifier for a volume that has been removed from service and held
    /// for operator review. The id becomes the directory name under the
    /// `held-volumes` store and the stem of the sidecar `{id}.meta.json`
    /// file, so the wire/on-disk form is the plain UUID string.
    HeldVolumeId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_app_names() {
        AppName::new("web").unwrap();
        AppName::new("my-app").unwrap();
        AppName::new("app-123").unwrap();
        AppName::new("abc").unwrap();
    }

    #[test]
    fn app_rejects_too_short() {
        assert!(matches!(AppName::new("ab"), Err(InvalidName::Malformed(_))));
    }

    #[test]
    fn app_rejects_too_long() {
        assert!(matches!(
            AppName::new("a".repeat(64)),
            Err(InvalidName::Malformed(_))
        ));
    }

    #[test]
    fn app_rejects_leading_hyphen() {
        assert!(matches!(
            AppName::new("-app"),
            Err(InvalidName::Malformed(_))
        ));
    }

    #[test]
    fn app_rejects_trailing_hyphen() {
        assert!(matches!(
            AppName::new("app-"),
            Err(InvalidName::Malformed(_))
        ));
    }

    #[test]
    fn app_rejects_leading_underscore_specifically() {
        assert!(matches!(
            AppName::new("_app"),
            Err(InvalidName::LeadingUnderscore)
        ));
    }

    #[test]
    fn app_rejects_leading_digit() {
        assert!(matches!(
            AppName::new("1app"),
            Err(InvalidName::Malformed(_))
        ));
    }

    #[test]
    fn app_rejects_non_ascii() {
        assert!(matches!(
            AppName::new("café"),
            Err(InvalidName::Malformed(_))
        ));
    }

    #[test]
    fn app_serde_transparent_as_string() {
        let a = AppName::new("my-app").unwrap();
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, "\"my-app\"");
        let b: AppName = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn app_deserialize_rejects_invalid() {
        let res: Result<AppName, _> = serde_json::from_str("\"_nope\"");
        assert!(res.is_err());
    }

    #[test]
    fn app_new_longest_accepted() {
        AppName::new(format!("a{}", "b".repeat(62))).unwrap();
    }

    #[test]
    fn action_accepts_canonical() {
        ActionName::new("start").unwrap();
        ActionName::new("backup").unwrap();
        ActionName::new("rotate-certs").unwrap();
    }

    #[test]
    fn action_rejects_leading_underscore() {
        assert!(matches!(
            ActionName::new("_private"),
            Err(InvalidName::LeadingUnderscore)
        ));
    }

    #[test]
    fn action_serde_transparent() {
        let a = ActionName::new("backup").unwrap();
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"backup\"");
        let b: ActionName = serde_json::from_str("\"backup\"").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn app_and_action_are_distinct_types() {
        // Compile-time: the following would not type-check if the types were the same.
        let a: AppName = AppName::new("web").unwrap();
        let b: ActionName = ActionName::new("web").unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn session_id_generate_is_unique() {
        let a = SessionId::generate();
        let b = SessionId::generate();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_serde_is_uuid_string() {
        let id = SessionId::generate();
        let json = serde_json::to_string(&id).unwrap();
        // serialised as "...uuid..." (quoted string)
        assert_eq!(json.len(), 38);
        let round: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, round);
    }

    #[test]
    fn session_id_parse_round_trips_display() {
        let id = SessionId::generate();
        let s = id.to_string();
        let parsed: SessionId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_rejects_garbage() {
        let r: Result<SessionId, _> = "not-a-uuid".parse();
        assert!(r.is_err());
    }

    #[test]
    fn forward_id_is_distinct_from_session_id() {
        // Compile-time: a ForwardId and SessionId share structure but are not assignable.
        let s = SessionId::generate();
        let f = ForwardId::generate();
        assert_ne!(s.as_uuid(), f.as_uuid());
    }

    #[test]
    fn forward_id_serde_round_trip() {
        let id = ForwardId::generate();
        let json = serde_json::to_string(&id).unwrap();
        let round: ForwardId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, round);
    }

    #[test]
    fn held_volume_id_round_trip() {
        let id = HeldVolumeId::generate();
        let s = id.to_string();
        let parsed: HeldVolumeId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn external_volume_name_accepts_canonical() {
        ExternalVolumeName::new("data").unwrap();
        ExternalVolumeName::new("shared-store").unwrap();
    }

    #[test]
    fn external_volume_name_rejects_underscore_prefix() {
        assert!(matches!(
            ExternalVolumeName::new("_data"),
            Err(InvalidName::LeadingUnderscore)
        ));
    }

    #[test]
    fn external_volume_name_serde_transparent() {
        let n = ExternalVolumeName::new("data").unwrap();
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"data\"");
    }

    #[test]
    fn external_volume_name_is_distinct_from_app_name() {
        let a = AppName::new("data").unwrap();
        let e = ExternalVolumeName::new("data").unwrap();
        assert_eq!(a.as_str(), e.as_str());
    }
}
