# A7: Version and SemVer parameter kinds

Adds the `version` and `version-semver` param kinds, their `std` parsers, and the `Version` and `SemVer` script types they return, as the Versions spec (VER) sets out.

## Notes

- Generic ordering comes from the `libversion` crate, Repology's own Rust port of libversion, compared with no flags.
  Its components are not public, so `segments()` does its own split into runs of ASCII letters and ASCII digits, the same split libversion makes.
- A `Version` keeps the text it was parsed from, for printing, and orders on that text with one leading `v` or `V` before a digit removed.
- A `SemVer` keeps its text and a `semver::Version` parsed after removing one leading `v` or `V`.
  It compares with `cmp_precedence`, not the crate's `Ord`, which breaks ties on build metadata.
  `matches` parses the requirement with the existing `VersionRequirement` in `crates/core/src/runtime/definition/version.rs`, which already applies Cargo's pre-release rule.
- BSL integers are `i64`, so a digit run in a `Version`, or a SemVer major, minor or patch number above `i64::MAX`, is a parse error.
- Operators and methods have to be reachable unqualified, unlike the `std::` parsers.
  Comparing with another type must throw, where Rhai's default for `==` and `!=` across types is `false`.
  Check that Rhai picks a `(Version, Version)` overload over a `(Version, Dynamic)` one before relying on throwing catch-all overloads.

## Types and kinds

- [x] `cargo add -p seedling-core libversion`
- [x] `crates/core/src/defs/version.rs`: `Version` with `parse`, libversion ordering, `segments()` and `Display` as written
- [x] Same file: `SemVer` with `parse`, the five getters, precedence ordering, `matches(requirement)` and `Display` as written
- [x] `crates/core/src/defs/param_def.rs`: `ParamKind::Version` and `ParamKind::VersionSemver` in `as_str`, `FromStr` and `parse`, allowed in static schemas; `ParsedValue::Version` and `ParsedValue::SemVer`
- [x] `crates/core/src/defs/stdlib/kinds.rs`: return the new `ParsedValue` variants as their script types
- [x] Register both types with the engine in `crates/core/src/defs.rs`: getters, `segments`, `matches`, the six comparison operators, `to_string` and `to_debug`, and throwing overloads for comparison with any other type
- [x] `// spec: VER#…` citations on the parsers, types, operators and printing

## Tests

- [x] `crates/core/src/tests/version.rs`, `std::parse_version`: accepts `16.4`, `1.0a`, `sha-3f2a1c`; rejects `latest`, a value with a space, non-ASCII, a control character, a digit run above `i64::MAX`
- [x] Same file, `Version` ordering: the libversion algorithm's example table in ascending order, `1.0 == 1.0.0`, `1.0alpha1 == 1.0a1`, `v2.0 == 2.0`, `v2.0 > 1.0`
- [x] Same file, `segments()`: `v1.0a1` gives `1`, `0`, `"a"`, `1`; leading zeros read as the integer
- [x] Same file, `std::parse_version_semver`: accepts `1.2.3`, `v1.2.3`, `V1.2.3-rc.1+b.5`; rejects `2.71`, `vv1.2.3`, a number above `i64::MAX`; the five getters, empty strings for absent pre-release and build
- [x] Same file, `SemVer` ordering: precedence across pre-releases, `1.0.0+a == 1.0.0+b`
- [x] Same file, `matches`: `~2.71`, a `||` requirement, `2.71.4-rc.1` against `>=2.71.4-rc.0` and `~2.71`, an invalid requirement throws
- [x] Same file: both types print as written, in `to_string()` and interpolation; every operator throws for `Version` against `SemVer`, a string and an integer
- [x] `crates/core/src/tests/param.rs`: `app.param(…).kind("version")` and `"version-semver"` accepted, and an action schema accepts both
- [x] `crates/core/src/runtime/action_params.rs`: an action param of each version kind is checked by its parser
- [x] `crates/core/src/oi/handler/params/tests.rs`: a stored value the kind rejects is refused with the parser's reason, for each new kind
- [x] `crates/web/frontend/src/components/ParamFields.test.tsx`: both kinds render a single-line text input

## Demo definitions

- [x] `apps/postgres.seed.rhai`: `version` param `.kind("version")`; replace the `parse_major` closure with `std::parse_version(v).segments()[0]` at its two call sites
- [x] `apps/tamanu-facility.seed.rhai` and `apps/tamanu-central.seed.rhai`: `version` param `.kind("version-semver")`, and the same kind on the `target-version` param of `warm-images`
- [x] Run `crates/core/src/tests/definitions.rs`, which evaluates the demos

## Docs

- [x] `docs/bsl-scripting.md`: the kind list in the Parameter block, the parser list in the `std` section, and a short section on `Version` and `SemVer` values

## Explicitly rejected

- A single SemVer-only `version` kind: `version` is generic, and SemVer is the `version-semver` opt-in.
- Requirements or ranges on a generic `Version`, including VERS range strings: the Seedling version requirement on `SemVer` is the only requirement syntax.
- Partial versions (`2.71`) as `version-semver` values.
- A `version-requirement` kind: requirements come from scripts, not operators.
