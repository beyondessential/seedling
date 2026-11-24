# E7: Package URL parameter kind

Adds the `purl` kind and `std::parse_purl` (Package URLs spec, PURL), and changes the generic `Version` so floating names and digests parse and sort after numbered versions (Versions spec, VER).
Both land in `crates/core/src/defs/`, next to the version types A7 added; the web frontend needs no code change, since every kind but `multiline` already gets a single-line input.

## Tech notes

- **Own parser, not the `packageurl` crate.** `packageurl` 0.7.1 applies type-specific rules in its constructor (lower-casing `github` and `pypi` names, rewriting `_` to `-` for `pypi`), which PURL#components forbids.
- **Shape of the parser.** The spec fixes component boundaries by the purl-spec v1.1.1 parsing procedure, which splits right to left (`#`, then `?`, then `@`, then the last `/`). The parser follows that procedure step for step with string splits, then checks the lenient grammar's character classes for the type and qualifier keys.
- **Percent-decoding** uses the `percent-encoding` crate (already in the lock at 2.3.2, added as a direct dependency of `seedling-core`), decoding each component to bytes and then to UTF-8 so invalid UTF-8 is a rejection.
- **Result type.** `Purl` is a plain Rust struct in `defs/purl.rs`, converted to a Rhai object map in `stdlib/kinds.rs`, so the map rule in STD#results holds and absent keys are simply not inserted. `version` goes in as a `Dynamic::from(Version)`.
- **Version ordering.** `Version::cmp` branches on whether each side is a numbered version: both numbered uses libversion as now; one numbered sorts below; neither compares `text` bytewise. Equality follows from `cmp`, as today.

## Versions

- [ ] `crates/core/src/defs/version.rs`: drop the digit requirement from `Version::parse`, and reject the empty string.
- [ ] Add `Version::is_numbered` (text starts with a digit, or `v`/`V` then a digit), and make `Ord` branch on it as the tech notes describe.
- [ ] `split_segments`: in a non-numbered version, keep a digit run too large for `INT` as `Segment::Word` holding the digits; still reject it in a numbered version.
- [ ] Update the `// spec:` citations on the changed code (`VER#generic-versions`, `VER#terminology` for `is_numbered` where it names the term).
- [ ] `crates/core/src/tests/version.rs`: replace the "holds no digit" refusals with acceptance of `latest`, `main`, `v`, `sha256:…`; keep `""` refused.
- [ ] Add ordering cases: numbered below `latest`; `latest` vs `main` by bytes; `Latest` vs `latest` unequal; `v2.0 == 2.0` still holds; a digest with a 20-digit run parses and its `segments()` gives that run as a string; `1.99999999999999999999` is refused.
- [ ] `crates/core/src/runtime/action_params.rs` (`a_version_param_is_checked_by_its_kind`): replace `latest` as the `version` refusal with a value that is still refused (one containing a space).

## Purl parser

- [ ] `cargo add -p seedling-core percent-encoding@2.3.2`.
- [ ] `crates/core/src/defs/purl.rs`: `Purl` struct (`ty`, `namespace: Option<String>`, `name`, `version: Option<Version>`, `qualifiers: Vec<(String, QualifierValue)>` in input order, `subpath: Option<String>`), with `QualifierValue::{Text(String), Checksums(Vec<String>)}`.
- [ ] `Purl::parse` following the parsing procedure:
  - [ ] split off the subpath on the last `#`; decode each segment, drop empty, `.` and `..`, reject a decoded `/`;
  - [ ] split off the qualifiers on the last `?`; split on `&`, split each pair on the first `=`, check the key against `ALPHA *(ALPHA / DIGIT / "." / "-" / "_")`, lower-case it, decode the value, drop empty values, reject a repeated key among the kept ones, split a `checksum` value on `,` after decoding and drop empty entries;
  - [ ] split the scheme on the first `:`, require it to equal `pkg` ignoring case, strip leading `/`;
  - [ ] split the type on the first `/`, check it against `ALPHA *(ALPHA / DIGIT / "." / "-")`, lower-case it;
  - [ ] split the version on the last `@`; when non-empty after decoding, parse it with `Version::parse` and carry its error;
  - [ ] strip trailing `/`, split the name on the last `/`, decode it, reject it when empty;
  - [ ] split the rest on `/`, drop empty segments, decode each, reject a decoded `/`, join with `/`, leave it out when nothing remains.
- [ ] Reject any decoded component holding a Cc, Zl or Zp character, reusing the category check `param_def.rs` already has for single-line kinds (lift it into a shared function rather than copying it).
- [ ] Cite `PURL#accepted-values` and `PURL#components` at the statements implementing each rule.
- [ ] `crates/core/src/defs.rs`: declare `pub mod purl;`.

## Wiring the kind

- [ ] `crates/core/src/defs/param_def.rs`: add `ParamKind::Purl` (`"purl"` both ways in `as_str` and `FromStr`), `ParsedValue::Purl(Purl)`, and the `Self::Purl` arm in `parse`.
- [ ] `crates/core/src/defs/stdlib/kinds.rs`: convert `ParsedValue::Purl` to the object map of PURL#components, with `checksum` as an array.
- [ ] Check the OI param description and CLI paths pick the new kind up through `ParamKind` without a separate list; add it wherever one turns up.

## Tests

- [ ] `crates/core/src/tests/purl.rs` (registered in `tests.rs`): BSL-level cases through `std::parse_purl`:
  - [ ] the full map for `pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=sha1:a,,sha256:b#lib/util`;
  - [ ] absent keys for `pkg:cargo/serde`, and for `pkg:npm/foo@` (no `version`);
  - [ ] lenient input accepted as the procedure reads it: `PKG://NPM/foo`, `pkg:npm//ns//foo/`, `?arch=&arch=x64`, `?FOO&bar=1`, a subpath with `.`, `..` and empty segments;
  - [ ] a type the purl spec does not define, and type-specific normalisation not applied (`pkg:pypi/Foo_Bar` keeps its name);
  - [ ] refusals, each naming its reason: no scheme, wrong scheme, bad type character, empty name (`pkg:npm/@1.0`), `%2F` in a namespace and in a subpath segment, invalid UTF-8 (`%FF`), `%0A` in a name, repeated key (`arch=x64&ARCH=arm64`), a version `parse_version` refuses (`%20` in it);
  - [ ] `version` is a `Version` that compares with `std::parse_version` results.
- [ ] Port the parse cases of purl-spec v1.1.1's `tests/spec/specification-test.json` into that file as Rust cases, adjusting any whose expected output relies on type-specific normalisation.
- [ ] `crates/core/src/tests/stdlib.rs`: add `parse_purl` to the single-line control-character test and to the helper list.
- [ ] `crates/core/src/tests/param.rs`: `"purl"` in the kind round-trip table and the action-schema case.
- [ ] `crates/core/src/oi/handler/params/tests.rs`: a stored `purl` parameter refused with the parser's reason and accepted unchanged.
- [ ] `crates/web/frontend/src/components/ParamFields.test.tsx`: add `"purl"` to the single-line input test.

## Docs and demos

- [ ] `docs/bsl-scripting.md`: add `purl` to the kind comment and the parser list, rewrite the `Version` paragraph for floating names and their ordering, and add a short purl section with an example map.
- [ ] Run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`, and the frontend test for `ParamFields`.
