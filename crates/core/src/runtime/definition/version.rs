use std::fmt;

use semver::{Comparator, Op, Version, VersionReq};

/// A Seedling version requirement: comparator sets joined by `||`, satisfied
/// when any one set is.
///
/// `semver::VersionReq` parses a single comma-joined set, so the alternatives
/// are split here and each is parsed on its own.
// l[impl bsl.bundle.seedling-versions]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRequirement {
    source: String,
    sets: Vec<VersionReq>,
}

impl VersionRequirement {
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut sets = Vec::new();
        for alternative in source.split("||") {
            let alternative = alternative.trim();
            if alternative.is_empty() {
                return Err(format!(
                    "version requirement {source:?} has an empty alternative"
                ));
            }
            let set = VersionReq::parse(alternative)
                .map_err(|e| format!("version requirement {source:?} is invalid: {e}"))?;
            sets.push(set);
        }
        Ok(Self {
            source: source.trim().to_owned(),
            sets,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn matches(&self, version: &Version) -> bool {
        self.sets.iter().any(|set| set.matches(version))
    }

    /// The lowest version satisfying the requirement, or `None` when no
    /// alternative can be satisfied at all.
    // i[impl definition.fetch.select]
    pub fn minimum(&self) -> Option<Version> {
        self.sets.iter().filter_map(set_minimum).min()
    }
}

impl fmt::Display for VersionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// The lowest version satisfying every comparator of one set: the greatest of
/// the comparators' lower bounds, checked against the set so that an
/// unsatisfiable set (`>=2, <1`) has none.
fn set_minimum(set: &VersionReq) -> Option<Version> {
    let floor = set
        .comparators
        .iter()
        .map(lower_bound)
        .max()
        .unwrap_or(Version::new(0, 0, 0));
    set.matches(&floor).then_some(floor)
}

fn lower_bound(c: &Comparator) -> Version {
    let minor = c.minor.unwrap_or(0);
    let patch = c.patch.unwrap_or(0);
    let exact = Version {
        major: c.major,
        minor,
        patch,
        pre: c.pre.clone(),
        build: Default::default(),
    };
    match c.op {
        Op::Exact | Op::GreaterEq | Op::Tilde | Op::Caret | Op::Wildcard => exact,
        // The next version past the one named, at the precision it was named.
        Op::Greater => match (c.minor, c.patch) {
            (None, _) => Version::new(c.major + 1, 0, 0),
            (Some(minor), None) => Version::new(c.major, minor + 1, 0),
            (Some(minor), Some(patch)) => Version::new(c.major, minor, patch + 1),
        },
        Op::Less | Op::LessEq => Version::new(0, 0, 0),
        _ => exact,
    }
}

/// The running Seedling's version, as the workspace declares it.
pub fn running() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is valid semver")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    // l[verify bsl.bundle.seedling-versions]
    #[test]
    fn alternatives_match_when_any_set_does() {
        let req = VersionRequirement::parse(">=0.13, <0.15 || ^1.2").unwrap();
        assert!(req.matches(&v("0.13.0")));
        assert!(req.matches(&v("0.14.9")));
        assert!(!req.matches(&v("0.15.0")));
        assert!(!req.matches(&v("0.12.9")));
        assert!(req.matches(&v("1.4.0")));
        assert!(!req.matches(&v("2.0.0")));
    }

    // l[verify bsl.bundle.seedling-versions]
    #[test]
    fn malformed_requirements_are_rejected() {
        assert!(VersionRequirement::parse("").is_err());
        assert!(VersionRequirement::parse(">=0.13 ||").is_err());
        assert!(VersionRequirement::parse("not a version").is_err());
    }

    // i[verify definition.fetch.select]
    // l[verify bsl.bundle.seedling-versions]
    #[test]
    fn minimum_is_lowest_across_alternatives() {
        let req = VersionRequirement::parse(">=0.14 || >=0.12, <0.13").unwrap();
        assert_eq!(req.minimum(), Some(v("0.12.0")));
        assert_eq!(
            VersionRequirement::parse(">0.12").unwrap().minimum(),
            Some(v("0.13.0"))
        );
        assert_eq!(
            VersionRequirement::parse(">0.12.3").unwrap().minimum(),
            Some(v("0.12.4"))
        );
        assert_eq!(
            VersionRequirement::parse("<0.12").unwrap().minimum(),
            Some(v("0.0.0"))
        );
        assert_eq!(
            VersionRequirement::parse(">=2, <1").unwrap().minimum(),
            None
        );
    }
}
