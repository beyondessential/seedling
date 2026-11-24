# E7: Package URL parameter kind

Adds the `purl` kind and `std::parse_purl` (Package URLs spec, PURL), and changes the generic `Version` so floating names and digests parse and sort after numbered versions (Versions spec, VER).
Both land in `crates/core/src/defs/`, next to the version types A7 added; the web frontend needs no code change, since every kind but `multiline` already gets a single-line input.

## Tech notes

- **Parsing builds on the `purl` crate** (0.1.6, phylum-dev, default features off, leaving `hex`, `percent-encoding` and `thiserror` as dependencies), through `GenericPurl<String>`, which parses any type without type-specific rules. The PURL spec's rejections were fitted to it: lower-case scheme only, bare and empty qualifiers rejected, a trailing `/` before the version rejected, and `checksum` validated and normalised.
- **Checks on top of the crate**, because it is more lenient than the spec in places:
  - before parsing, a scan of the raw qualifier string for any repeated key (lower-cased), since the crate rejects `arch=x64&arch=` but accepts `arch=&arch=x64`;
  - after parsing, the type against `ALPHA *(ALPHA / DIGIT / "." / "-")` (the crate also allows `+`), each qualifier key against `ALPHA *(ALPHA / DIGIT / "." / "-" / "_")` (the crate allows a leading digit), and each `checksum` entry for a non-empty algorithm and digits (the crate accepts `:ab` and `sha1:`);
  - after parsing, every decoded component for Cc, Zl and Zp characters, and the version through `Version::parse`.
- **Ruled out: `packageurl` 0.7.1**, which applies type-specific rules in its constructor (lower-casing `github` and `pypi` names, rewriting `_` to `-` for `pypi`) and keeps the last of two repeated qualifiers.
- **Result type.** `Purl` is a plain Rust struct in `defs/purl.rs`, converted to a Rhai object map in `stdlib/kinds.rs`, so the map rule in STD#results holds and absent keys are simply not inserted. `version` goes in as a `Dynamic::from(Version)`.
- **Version ordering.** `Version::cmp` branches on whether each side is a numbered version: both numbered uses libversion as now; one numbered sorts below; neither compares `text` bytewise. Equality follows from `cmp`, as today.

## Versions

- [x] `crates/core/src/defs/version.rs`: drop the digit requirement from `Version::parse`, and reject the empty string.
- [x] Add `Version::is_numbered` (text starts with a digit, or `v`/`V` then a digit), and make `Ord` branch on it as the tech notes describe.
- [x] `split_segments`: in a non-numbered version, keep a digit run too large for `INT` as `Segment::Word` holding the digits; still reject it in a numbered version.
- [x] Update the `// spec:` citations on the changed code (`VER#generic-versions`, `VER#terminology` for `is_numbered` where it names the term).
- [x] `crates/core/src/tests/version.rs`: replace the "holds no digit" refusals with acceptance of `latest`, `main`, `v`, `sha256:…`; keep `""` refused.
- [x] Add ordering cases: numbered below `latest`; `latest` vs `main` by bytes; `Latest` vs `latest` unequal; `v2.0 == 2.0` still holds; a digest with a 20-digit run parses and its `segments()` gives that run as a string; `1.99999999999999999999` is refused.
- [x] `crates/core/src/runtime/action_params.rs` (`a_version_param_is_checked_by_its_kind`): replace `latest` as the `version` refusal with a value that is still refused (one containing a space).

## Purl parser

- [x] `cargo add -p seedling-core purl@0.1.6 --no-default-features`.
- [x] `crates/core/src/defs/purl.rs`: `Purl` struct (`ty`, `namespace: Option<String>`, `name`, `version: Option<Version>`, `qualifiers: Vec<(String, QualifierValue)>`, `subpath: Option<String>`), with `QualifierValue::{Text(String), Checksums(Vec<String>)}`.
- [x] `Purl::parse`:
  - [x] take the qualifier string (split off `#` from the right, then `?` from the right) and reject a key repeated once lower-cased;
  - [x] parse with `purl::GenericPurl::<String>::from_str`, mapping its errors to messages naming the problem;
  - [x] check the type and qualifier keys against the lenient grammar's character classes;
  - [x] split the normalised `checksum` value on `,`, and reject an entry with an empty algorithm or empty digits;
  - [x] parse a present version with `Version::parse`, carrying its error.
- [x] Reject any decoded component holding a Cc, Zl or Zp character, reusing the category check `param_def.rs` already has for single-line kinds (lift it into a shared function rather than copying it).
- [x] Cite `PURL#accepted-values` and `PURL#components` at the statements implementing each rule.
- [x] `crates/core/src/defs.rs`: declare `pub mod purl;`.

## Wiring the kind

- [x] `crates/core/src/defs/param_def.rs`: add `ParamKind::Purl` (`"purl"` both ways in `as_str` and `FromStr`), `ParsedValue::Purl(Purl)`, and the `Self::Purl` arm in `parse`.
- [x] `crates/core/src/defs/stdlib/kinds.rs`: convert `ParsedValue::Purl` to the object map of PURL#components, with `checksum` as an array.
- [x] Check the OI param description and CLI paths pick the new kind up through `ParamKind` without a separate list; add it wherever one turns up.

## Tests

- [x] `crates/core/src/tests/purl.rs` (registered in `tests.rs`): BSL-level cases through `std::parse_purl`:
  - [x] the full map for `pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=SHA256:BB,sha1:aa#lib/util`, with `checksum` as `["sha1:aa", "sha256:bb"]`;
  - [x] absent keys for `pkg:cargo/serde`, and for `pkg:npm/foo@` (no `version`);
  - [x] lenient input accepted as the procedure reads it: `pkg://NPM/foo`, `pkg:npm//ns//foo`, `?arch=&bar=1` (no `arch`), `?FOO=1` (key `foo`), a subpath with `.`, `..`, `%2E%2E` and empty segments;
  - [x] a type the purl spec does not define, and type-specific normalisation not applied (`pkg:pypi/Foo_Bar` keeps its name);
  - [x] refusals, each naming its reason: no scheme, `PKG:` scheme, wrong scheme, `+` in the type, a qualifier key starting with a digit, empty name (`pkg:npm/@1.0`), a trailing `/` (`pkg:npm/foo/`, `pkg:npm/foo/@1.0`), `%2F` in a namespace and in a subpath segment, invalid UTF-8 (`%FF`), `%0A` in a name and in a qualifier value, `?FOO`, a trailing `?`, `&&`, repeated keys (`arch=x64&ARCH=arm64`, `arch=&arch=x64`, `arch=x64&arch=`), checksum entries that are odd-length, non-hex, missing the algorithm or digits, or naming an algorithm twice, a version `parse_version` refuses (`%20` in it);
  - [x] `version` is a `Version` that compares with `std::parse_version` results.
- [x] Port the parse cases of purl-spec v1.1.1's `tests/spec/specification-test.json` into that file as Rust cases, adjusting any whose expected output relies on type-specific normalisation or on input the PURL spec rejects beyond the procedure.
- [x] `crates/core/src/tests/stdlib.rs`: add `parse_purl` to the single-line control-character test and to the helper list.
- [x] `crates/core/src/tests/param.rs`: `"purl"` in the kind round-trip table and the action-schema case.
- [x] `crates/core/src/oi/handler/params/tests.rs`: a stored `purl` parameter refused with the parser's reason and accepted unchanged.
- [x] `crates/web/frontend/src/components/ParamFields.test.tsx`: add `"purl"` to the single-line input test.

## Docs and demos

- [x] `docs/bsl-scripting.md`: add `purl` to the kind comment and the parser list, rewrite the `Version` paragraph for floating names and their ordering, and add a short purl section with an example map.
- [x] Run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`, and the frontend test for `ParamFields`.
