# A7: Version and SemVer parameter kinds

## Generic versions

- [x] `std::parse_version` accepts `16.4`, `16.4-bookworm`, `1.0a`, `sha-3f2a1c`, `v2.71.3` and a Debian version, and prints each as written (verifies spec: VER)
- [x] `std::parse_version` refuses `latest`, `stable`, `v` and the empty string for holding no digit (verifies spec: VER)
- [x] `std::parse_version` refuses a space, non-ASCII and full-width digits (verifies spec: VER)
- [x] A control character or line break is refused naming the character, as for every single-line kind (verifies spec: ACN)
- [x] A digit run above the largest BSL integer is refused; one at the largest is a segment (verifies spec: VER)
- [x] The libversion algorithm's example versions order ascending through all six operators (verifies spec: VER)
- [x] `1.0 == 1.0.0`, `1.0alpha1 == 1.0a1`, `1.01 == 1.1`, `1-0 == 1.0`, and `v2.0` and `V2.0` equal `2.0` (verifies spec: VER)
- [x] `v2.0` sorts above `0.1` and `1.0`, while `ver2.0` keeps `ver` as a word and sorts below `0.1` (verifies spec: VER)
- [x] `segments()` of `v1.0a1` is `1`, `0`, `"a"`, `1`; leading zeros read as the integer; letters keep their case (verifies spec: VER)

## SemVer versions

- [x] `std::parse_version_semver` accepts `1.2.3`, `v1.2.3`, `V1.2.3-rc.1+b.5` and `0.0.0`, and prints each as written (verifies spec: VER)
- [x] `std::parse_version_semver` refuses `2.71`, `2`, `vv1.2.3`, `01.2.3`, `1.2.3.4`, `latest`, `v` and the empty string (verifies spec: VER)
- [x] A major, minor or patch number above the largest BSL integer is refused, naming the part (verifies spec: VER)
- [x] The five getters return the parts, with empty strings for an absent pre-release and build (verifies spec: VER)
- [x] The SemVer specification's precedence example orders ascending through all six operators (verifies spec: VER)
- [x] `1.0.0+a == 1.0.0+b`, and `v1.0.0 == 1.0.0` (verifies spec: VER)
- [x] `matches` handles `~2.71`, comma sets and `||` alternatives (verifies spec: VER, BND)
- [x] `2.71.4-rc.1` matches `>=2.71.4-rc.0` but not `~2.71` or `>=2.71.0` (verifies spec: BND)
- [x] `matches` raises for an invalid requirement and for a requirement that is not a string (verifies spec: VER)

## Both types

- [x] A version prints as written through `to_string()`, interpolation and string concatenation (verifies spec: VER)
- [x] Every comparison operator raises between a `Version` and a `SemVer`, a string, an integer or a unit, on either side (verifies spec: VER)
- [x] A list of versions sorts with `sort(|a, b| a <= b)` (verifies spec: VER)

## Parameters and actions

- [x] `app.param(…).kind("version")` and `.kind("version-semver")` are accepted and not secret (verifies spec: PARAM)
- [x] An action schema accepts both kinds (verifies spec: ACN)
- [x] An action param of each kind is refused by its parser and passed through as written when accepted (verifies spec: ACN)
- [x] A stored value each kind refuses is not stored, with the parser's reason; a valid value is stored as written (verifies spec: PARAM)
- [x] The console offers a single-line text input for both kinds (verifies spec: ACN)

## Demo definitions

- [x] The Postgres and Tamanu demos evaluate with their version parameters declared with the new kinds
