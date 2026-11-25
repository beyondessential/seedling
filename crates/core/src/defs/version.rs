//! The values the `version` and `version-semver` kinds' parsers return.

use std::{cmp::Ordering, fmt};

use rhai::{CustomType, Dynamic, EvalAltResult, INT, TypeBuilder};

use super::compare::refuse_mixed_comparisons;
use crate::runtime::definition::VersionRequirement;

/// A version of any scheme, ordered as libversion orders it.
// spec: VER#generic-versions
#[derive(Debug, Clone)]
pub struct Version {
    text: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Number(INT),
    Word(String),
}

impl Version {
    const NAME: &str = "Version";

    // spec: VER#generic-versions
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(c) = text.chars().find(|c| !c.is_ascii_graphic()) {
            return Err(format!(
                "a version holds only printable ASCII characters other than space, \
                 and {c:?} is not one"
            ));
        }
        if text.is_empty() {
            return Err("a version cannot be empty".to_owned());
        }
        let segments = split_segments(ordering_text(text), is_numbered(text))?;
        Ok(Self {
            text: text.to_owned(),
            segments,
        })
    }

    fn is_numbered(&self) -> bool {
        is_numbered(&self.text)
    }
}

/// Whether `text` begins with a digit, or with a `v` or `V` directly before
/// one.
// spec: VER#numbered-version
fn is_numbered(text: &str) -> bool {
    ordering_text(text).starts_with(|c: char| c.is_ascii_digit())
}

/// The text a version is ordered and split by: the value without a leading
/// `v` or `V` that a digit follows.
// spec: VER#generic-versions
fn ordering_text(text: &str) -> &str {
    match text.as_bytes() {
        [b'v' | b'V', next, ..] if next.is_ascii_digit() => &text[1..],
        _ => text,
    }
}

/// The runs of ASCII letters and of ASCII digits that libversion splits a
/// version into, every other character separating them. A digit run too
/// large for an integer is refused in a numbered version and kept as a word
/// in any other.
// spec: VER#generic-versions
fn split_segments(text: &str, numbered: bool) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphanumeric()) {
        rest = &rest[start..];
        let digits = rest.starts_with(|c: char| c.is_ascii_digit());
        let end = rest
            .find(|c: char| {
                if digits {
                    !c.is_ascii_digit()
                } else {
                    !c.is_ascii_alphabetic()
                }
            })
            .unwrap_or(rest.len());
        let run = &rest[..end];
        segments.push(if !digits {
            Segment::Word(run.to_owned())
        } else if let Ok(n) = run.parse() {
            Segment::Number(n)
        } else if numbered {
            return Err(format!(
                "the number {run} in a version is too large for an integer"
            ));
        } else {
            Segment::Word(run.to_owned())
        });
        rest = &rest[end..];
    }
    Ok(segments)
}

impl Ord for Version {
    // spec: VER#generic-versions
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_numbered(), other.is_numbered()) {
            (true, true) => {
                libversion::version_compare2(ordering_text(&self.text), ordering_text(&other.text))
            }
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => self.text.as_bytes().cmp(other.text.as_bytes()),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Version {}

// spec: VER#text-of-a-version
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl CustomType for Version {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: VER#generic-versions
        builder.with_fn("segments", |this: &mut Self| -> rhai::Array {
            this.segments
                .iter()
                .map(|segment| match segment {
                    Segment::Number(n) => Dynamic::from_int(*n),
                    Segment::Word(word) => word.clone().into(),
                })
                .collect()
        });

        register_comparisons(&mut builder, Self::NAME);
    }
}

/// A Semantic Versioning 2.0.0 version, ordered by precedence.
// spec: VER#semver-versions
#[derive(Debug, Clone)]
pub struct SemVer {
    text: String,
    version: semver::Version,
}

impl SemVer {
    const NAME: &str = "SemVer";

    // spec: VER#semver-versions
    pub fn parse(text: &str) -> Result<Self, String> {
        let bare = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let version = semver::Version::parse(bare)
            .map_err(|e| format!("{text:?} is not a SemVer version: {e}"))?;
        for (part, number) in [
            ("major", version.major),
            ("minor", version.minor),
            ("patch", version.patch),
        ] {
            if INT::try_from(number).is_err() {
                return Err(format!(
                    "the {part} number of {text:?} is too large for an integer"
                ));
            }
        }
        Ok(Self {
            text: text.to_owned(),
            version,
        })
    }
}

impl Ord for SemVer {
    // spec: VER#semver-versions
    fn cmp(&self, other: &Self) -> Ordering {
        self.version.cmp_precedence(&other.version)
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SemVer {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for SemVer {}

// spec: VER#text-of-a-version
impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A number `parse` has already checked fits an integer.
fn as_int(number: u64) -> INT {
    INT::try_from(number).expect("parse checks every number fits an integer")
}

impl CustomType for SemVer {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: VER#semver-versions
        builder.with_fn("major", |this: &mut Self| as_int(this.version.major));
        // spec: VER#semver-versions
        builder.with_fn("minor", |this: &mut Self| as_int(this.version.minor));
        // spec: VER#semver-versions
        builder.with_fn("patch", |this: &mut Self| as_int(this.version.patch));
        // spec: VER#semver-versions
        builder.with_fn("pre_release", |this: &mut Self| {
            this.version.pre.to_string()
        });
        // spec: VER#semver-versions
        builder.with_fn("build", |this: &mut Self| this.version.build.to_string());

        // spec: VER#semver-versions
        builder.with_fn(
            "matches",
            |this: &mut Self, requirement: Dynamic| -> Result<bool, Box<EvalAltResult>> {
                let requirement = crate::defs::take::take_string("requirement", requirement)?;
                let requirement = VersionRequirement::parse(&requirement)?;
                Ok(requirement.matches(&this.version))
            },
        );

        register_comparisons(&mut builder, Self::NAME);
    }
}

/// Register the comparison operators between two values of `T`, the
/// operators that refuse a value of any other type on either side, and how
/// `T` prints.
fn register_comparisons<T>(builder: &mut TypeBuilder<T>, name: &'static str)
where
    T: CustomType + Ord + Clone + fmt::Display + Send + Sync + 'static,
{
    // spec: VER#generic-versions
    // spec: VER#semver-versions
    builder
        .with_fn("==", |a: &mut T, b: T| *a == b)
        .with_fn("!=", |a: &mut T, b: T| *a != b)
        .with_fn("<", |a: &mut T, b: T| *a < b)
        .with_fn("<=", |a: &mut T, b: T| *a <= b)
        .with_fn(">", |a: &mut T, b: T| *a > b)
        .with_fn(">=", |a: &mut T, b: T| *a >= b);

    // spec: VER#mixing-version-types
    refuse_mixed_comparisons(
        builder,
        name,
        "a version compares only with a version of its own type",
    );

    // spec: VER#text-of-a-version
    builder
        .on_print(|this: &mut T| this.to_string())
        .on_debug(|this: &mut T| this.to_string());
}
