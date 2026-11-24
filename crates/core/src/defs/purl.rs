//! The value the `purl` kind's parser returns.

use std::{collections::HashSet, str::FromStr};

use purl::{GenericPurl, ParseError, PurlField};

use super::{param_def::line_breaking_character, version::Version};

/// The components of a Package URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Purl {
    pub ty: String,
    pub namespace: Option<String>,
    pub name: String,
    pub version: Option<Version>,
    pub qualifiers: Vec<(String, QualifierValue)>,
    pub subpath: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualifierValue {
    Text(String),
    Checksums(Vec<String>),
}

const SCHEME: &str = "pkg:";
const CHECKSUM: &str = "checksum";

impl Purl {
    pub fn parse(text: &str) -> Result<Self, String> {
        // spec: PURL#accepted-values
        if !text.starts_with(SCHEME) {
            return Err(format!("a Package URL starts with {SCHEME:?}"));
        }
        check_qualifiers(text)?;
        let purl = GenericPurl::<String>::from_str(text).map_err(describe)?;

        // spec: PURL#accepted-values
        let ty = purl.package_type().clone();
        if !is_type(&ty) {
            return Err(format!(
                "the type {ty:?} is not a letter followed by letters, digits, `.` and `-`"
            ));
        }

        let mut qualifiers = Vec::new();
        for (key, value) in purl.qualifiers().iter() {
            let key = key.as_str();
            check_component(&format!("qualifier {key}"), value)?;
            // spec: PURL#components
            let value = if key == CHECKSUM {
                QualifierValue::Checksums(checksums(value)?)
            } else {
                QualifierValue::Text(value.to_owned())
            };
            qualifiers.push((key.to_owned(), value));
        }

        let namespace = purl.namespace().map(str::to_owned);
        if let Some(namespace) = &namespace {
            check_component("namespace", namespace)?;
        }
        let name = purl.name().to_owned();
        check_component("name", &name)?;
        let subpath = purl.subpath().map(str::to_owned);
        if let Some(subpath) = &subpath {
            check_component("subpath", subpath)?;
        }

        // spec: PURL#accepted-values
        let version = purl
            .version()
            .map(|version| {
                Version::parse(version).map_err(|e| format!("the version {version:?}: {e}"))
            })
            .transpose()?;

        Ok(Self {
            ty,
            namespace,
            name,
            version,
            qualifiers,
            subpath,
        })
    }
}

/// Check the qualifiers the parsing procedure would split off `text`, before
/// they are decoded: each is a `key=value` pair whose key matches the lenient
/// grammar, and no key appears twice once in lower case.
fn check_qualifiers(text: &str) -> Result<(), String> {
    let rest = text.rsplit_once('#').map_or(text, |(rest, _)| rest);
    let Some((_, qualifiers)) = rest.rsplit_once('?') else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    for qualifier in qualifiers.split('&') {
        // spec: PURL#accepted-values
        if qualifier.is_empty() {
            return Err("a Package URL holds an empty qualifier".to_owned());
        }
        let Some((key, _)) = qualifier.split_once('=') else {
            return Err(format!("the qualifier {qualifier:?} has no `=`"));
        };
        if !is_qualifier_key(key) {
            return Err(format!(
                "the qualifier key {key:?} is not a letter followed by letters, digits, `.`, `-` and `_`"
            ));
        }
        // spec: PURL#accepted-values
        if !seen.insert(key.to_ascii_lowercase()) {
            return Err(format!(
                "the qualifier {:?} appears more than once",
                key.to_ascii_lowercase()
            ));
        }
    }
    Ok(())
}

/// The reason for an error the `purl` crate gives, once `check_qualifiers`
/// has ruled out every qualifier problem but a checksum's.
fn describe(error: ParseError) -> String {
    match error {
        ParseError::UnsupportedUrlScheme => format!("a Package URL starts with {SCHEME:?}"),
        ParseError::MissingRequiredField(PurlField::Name) => {
            "a Package URL has a non-empty name, with no `/` after it".to_owned()
        }
        ParseError::MissingRequiredField(field) => format!("a Package URL has a {field}"),
        ParseError::InvalidPackageType => {
            "a Package URL's type is a letter followed by letters, digits, `.` and `-`".to_owned()
        }
        ParseError::InvalidQualifier => format!(
            "a Package URL's {CHECKSUM} is a list of `algorithm:digits` entries separated by `,`, \
             each with an even number of hexadecimal digits and an algorithm named once"
        ),
        ParseError::InvalidEscape => "a Package URL's components are valid UTF-8 once \
             percent-decoded, and its namespace and subpath segments hold no `/`"
            .to_owned(),
    }
}

/// The entries of a `checksum` qualifier the `purl` crate has validated, each
/// checked to name an algorithm and hold digits, in lower case and in the
/// byte order of their algorithm names.
// spec: PURL#accepted-values
// spec: PURL#components
fn checksums(value: &str) -> Result<Vec<String>, String> {
    let mut entries = value
        .split(',')
        .map(|entry| match entry.rsplit_once(':') {
            Some((algorithm, digits)) if !algorithm.is_empty() && !digits.is_empty() => {
                Ok((algorithm.to_ascii_lowercase(), digits.to_ascii_lowercase()))
            }
            _ => Err(format!(
                "the {CHECKSUM} entry {entry:?} does not name an algorithm and hold its digits"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries
        .into_iter()
        .map(|(algorithm, digits)| format!("{algorithm}:{digits}"))
        .collect())
}

// spec: PURL#accepted-values
fn check_component(component: &str, value: &str) -> Result<(), String> {
    match line_breaking_character(value) {
        Some(c) => Err(format!(
            "the {component} contains U+{:04X}, which a Package URL component cannot hold",
            u32::from(c)
        )),
        None => Ok(()),
    }
}

/// Whether `ty` matches the lenient grammar's `ALPHA *( ALPHA / DIGIT / "." / "-" )`.
fn is_type(ty: &str) -> bool {
    ty.starts_with(|c: char| c.is_ascii_alphabetic())
        && ty
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// Whether `key` matches the lenient grammar's
/// `ALPHA *( ALPHA / DIGIT / "." / "-" / "_" )`.
fn is_qualifier_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_alphabetic())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}
