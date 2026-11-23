//! The values the `version` and `version-semver` kinds' parsers return.

use std::{cmp::Ordering, fmt};

use rhai::{CustomType, Dynamic, EvalAltResult, INT, TypeBuilder};

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
        if !text.bytes().any(|b| b.is_ascii_digit()) {
            return Err(format!("{text:?} is not a version: it holds no digit"));
        }
        let segments = split_segments(ordering_text(text))?;
        Ok(Self {
            text: text.to_owned(),
            segments,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
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
/// version into, every other character separating them.
// spec: VER#generic-versions
fn split_segments(text: &str) -> Result<Vec<Segment>, String> {
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
        segments.push(if digits {
            Segment::Number(run.parse().map_err(|_| {
                format!("the number {run} in a version is too large for an integer")
            })?)
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
        libversion::version_compare2(ordering_text(&self.text), ordering_text(&other.text))
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

    pub fn as_str(&self) -> &str {
        &self.text
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
    for op in ["==", "!=", "<", "<=", ">", ">="] {
        builder
            .with_fn(
                op,
                move |_: &mut T, b: Dynamic| -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, name, type_name(&b)))
                },
            )
            .with_fn(
                op,
                move |a: Dynamic, _: T| -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, type_name(&a), name))
                },
            );
    }

    // spec: VER#text-of-a-version
    builder
        .on_print(|this: &mut T| this.to_string())
        .on_debug(|this: &mut T| this.to_string());
}

// spec: VER#mixing-version-types
fn mixed(op: &str, left: &str, right: &str) -> Box<EvalAltResult> {
    format!(
        "cannot compare {left} {op} {right}: a version compares only with a version of its own type"
    )
    .into()
}

/// The name a script knows `value`'s type by.
fn type_name(value: &Dynamic) -> &str {
    if value.is::<Version>() {
        Version::NAME
    } else if value.is::<SemVer>() {
        SemVer::NAME
    } else {
        value.type_name()
    }
}
