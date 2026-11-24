# Boolean, integer and choice parameter kinds

Implements the kind changes in the actions (ACN) and parameters (PARAM) specs and the wire specs that follow from them. A kind is now a name or a map carrying arguments, and scripts receive parser results instead of strings. Stored values and the strings operators type stay strings throughout.

## Design notes

- **`ParamKind` takes arguments, so it can no longer be `Copy` or `VariantArray`.** Split it in two:
  - `ParamKindName`: a fieldless, `Copy`, `VariantArray` enum, which owns `as_str`, `FromStr` and `helper_name`.
  - `ParamKind`: the enum that carries data, with `Integer { range: Option<IntRange> }` and `Choice { options: Vec<String> }`, plus `fn name(&self) -> ParamKindName`.
  - `kinds.rs` registers one helper per `ParamKindName`. It gives `parse_integer` both a one-argument and a two-argument overload, and `parse_choice` only the two-argument form.
- **`IntRange` keeps the end as the script gave it**: `{ start: i64, end: i64, inclusive: bool }`, not normalised to inclusive.
  - `/apps/show` reports `max` as `null` where `end == i64::MAX`, whether the end is inclusive or exclusive. Otherwise `max` is `end` for an inclusive range and `end - 1` for an exclusive one. `min` is `null` where `start == i64::MIN`.
  - Normalising first would make `1..` impossible to tell apart from `1..=MAX-1`.
  - The constructor rejects an empty range.
- **One reader for kinds.** A single `take_kind(what, Dynamic) -> Result<ParamKind>` in `defs::take` (or `param_def`) accepts a string or a kind map. It rejects unknown keys and argument keys the kind does not take, and checks `range` and `options`. Both `param.kind(...)` and `parse_param_defs` call it, and the two-argument `std::parse_*` helpers validate their argument through it too.
- **`ParsedValue` gains `Bool(bool)` and `Int(i64)`.** Two conversions sit beside it, so no call site builds the shapes itself:
  - `ParsedValue::into_dynamic` replaces the inline map building in `kinds.rs`. Validators and `parsed()` use it.
  - `ParsedValue::into_json` produces the action param map.
- **One type for a value as supplied.** `SuppliedValue { Text | Bool | Int }`, with `canonical()`, `from_json` and `take`, serves defaults, `/apps/params/set` values and invocation values alike. `ParamDef.default_value` is an `Option<SuppliedValue>`, and `ParamKind::check_type` and `parse_supplied` apply the native-type rule in one place.
  - After a stored parameter's evaluation, the end-of-evaluation check rejects a native default whose kind doesn't match, and runs the kind's parser on string defaults.
  - Action and shell schemas do the same check at declaration, in `parse_param_defs`.
  - An empty string clears the default, in both places.
- **Native values in `/apps/params/set` are only checked at validation.** `ParamChanges` is parsed before the definition is evaluated, so it can't check them against the kind. Its values are `SuppliedValue`s:
  - `check_kinds` in `runtime/apps.rs` rejects a native value whose kind doesn't match, or that is set for an unknown parameter.
  - Storage, history entries and `applied_to` all use `canonical()`.
  - `/apps/update` and `/apps/plan` share this parser, so they get the change too.
- **Action param maps carry parser results from validation onward.** `apply_schema` is the one place an invocation is validated: OI actions, shells, `invoke`, schedules, and now probes. It returns a `ParamMap` holding each parser result. Persistence, the queue, the action log and replay then hold parsed values without changing (OPS#persistence).
- **`ParamMap` keeps version results typed.** A `Version` or `SemVer` cannot be JSON, so the map holds its text and records its type beside the values. A purl's components are held as a map with its version as text, tagged `purl`, so its nested `Version` is restored too. Undeclared params pass as supplied, so no in-band JSON shape can mark a version. `to_rhai` restores each version from its text, failing loudly for a map not built by `insert_parsed`. `invoke` takes a `Version` or `SemVer` as its text, so a closure can pass on its param map. The persisted blob's format changed with this; Seedling is unshipped.
- **Scheduled runs deliver their validated params.** Schedules validated an empty invocation but dispatched an empty map, so defaults never reached the closure. The fire now carries the validated map.
- **JSON integers.** Accept a `Number` only when `as_i64()` is `Some`. serde_json reads `1.0` and `1e3` as floats, and numbers past `i64::MAX` as `u64` or `f64`, so this rule rejects all of them. The workspace doesn't enable `arbitrary_precision`.
- **The web console sends strings for every kind.** The spec accepts strings everywhere, and strings avoid JS losing precision above 2^53 on integers.

## Checklist

### Before starting

- [x] Run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`, and fix anything outstanding

### Kinds and parsers (`crates/core/src/defs/param_def.rs`)

- [x] Split `ParamKind` into `ParamKindName` and the data-carrying `ParamKind`, adding `Boolean`, `Integer { range }` and `Choice { options }`. Move `allowed_static`, `is_single_line` and `as_str` to whichever type fits
- [x] Add `IntRange`, with a fallible constructor from a Rhai `Range<i64>` or `RangeInclusive<i64>` and the `min`/`max` reporting from the design notes
- [x] Add the parsers for the new kinds to `ParamKind::parse`:
  - `boolean`: exactly `true` or `false`
  - `integer`: canonical decimal only, with an error that names the range when the value falls outside it
  - `choice`: an exact, case-sensitive match
  - all three still pass through the single-line check
- [x] Add a `canonical form` helper for bool and i64 values
- [x] Extend `ParsedValue` with `Bool` and `Int`, and add `into_dynamic` and `into_json`
- [x] Add `take_kind` for strings and kind maps, including choice options (non-empty, distinct, non-empty strings, each accepted by `parse_text`)
- [x] Add `SuppliedValue` and update `is_secret` and every `ParamDef` constructor, including the test helpers in `action_params.rs`, `schedules.rs`, `probe.rs` and `action_call.rs`

### BSL surface

- [x] `crates/core/src/defs/stdlib/kinds.rs`: register by `ParamKindName`, with `parse_integer` taking one or two arguments and `parse_choice` taking two. Validate the argument through `take_kind`, and convert results with `into_dynamic`
- [x] `crates/core/src/defs/param.rs`:
  - `kind` takes a `Dynamic` and calls `take_kind`, still rejecting `volume`
  - `default_value` takes a string, bool or int, and `""` clears the default
  - add `parsed()`, which uses the kind as declared at the moment it is called
- [x] `crates/core/src/defs/app/param.rs` `parse_param_defs`:
  - read `kind` through `take_kind`
  - accept a string, bool or int `default_value`, treating `""` as none
  - reject a native default whose kind doesn't match, and a non-empty default the kind's parser rejects
- [x] After evaluation completes, check each declared default against its kind and fail the evaluation, naming the parameter, if one is rejected. This goes in the evaluate path in `crates/core/src/runtime/apps.rs`, beside the existing validator capture

### Validation and validators (`crates/core/src/runtime/apps.rs`)

- [x] `check_kinds`: take `SuppliedValue`s, and reject a native value for a mismatched kind or an unknown parameter
- [x] Run no validators at all once `check_kinds` returns any rejection, replacing the per-parameter `retain`
- [x] `run_validators`: pass the validated parameter's parser result, and a map holding only declared, set parameters as parser results

### Parameter changes (`crates/core/src/oi/handler/params.rs`)

- [x] Change `ParamChanges` to hold `SuppliedValue`s. Accept a string, a bool, a JSON integer (`as_i64`) or `null`, and refuse anything else with `requirements_invalid`
- [x] Use `canonical()` in `applied_to`, `edits` and storage. Thread the supplied type through to `check_kinds` for `/apps/params/set`, `/apps/update` and `/apps/plan`

### Action and shell invocation (`crates/core/src/runtime/action_params.rs`)

- [x] `apply_schema`:
  - take a JSON bool for `boolean` and a JSON i64 for `integer`, converting each to its canonical form
  - reject any other non-string for declared params, and report an out-of-range or non-integer number as a validation failure
  - once validated, write each declared param's parser result into the map, leaving out params that have no value
- [x] Rename or reshape `ParamValidationError::NotAString` to cover "not a type this kind accepts"
- [x] Check that `validate_volume_params` still reads the volume value as a string after `apply_schema` has written parser results (the volume parser result is the string itself)
- [x] Check the callers deliver the rewritten map unchanged:
  - `oi/handler/actions.rs`
  - `oi/shells/session.rs`
  - `runtime/barrier/action_call.rs` (`invoke` and the action-log entry)
  - `runtime/schedules.rs`

### Image probes (`crates/core/src/runtime/probe.rs`)

- [x] Change `ProbeRequest.action_params` leaf values from `String` to `serde_json::Value`
- [x] Replace `resolve_params` with a two-step resolution:
  - build a JSON map from the supplied values, falling back to stored ones
  - run it through `apply_schema`
  - report a kind rejection as an error in strict mode or a skip in lenient mode, naming the parameter and the parser's error
- [x] Update the handler that parses the request in `oi/handler/images.rs`

### Wire reporting

- [x] Merge `serialize_param_schema` (`oi/handler/apps.rs`) and `param_schema_entry_json` (`oi/handler/appdef_json.rs`) into one schema serialiser, and have both call sites use it. Add `range` and `options` there, so `/apps/show` params, action and shell entries, and template previews all get them

### Web console (`crates/web/frontend`)

- [x] `src/lib/types.ts`: add `range: { min: number | null; max: number | null } | null` and `options: string[] | null` to the param schema and app param types
- [x] `src/components/ParamFields.tsx`:
  - a toggle for `boolean`, with a way to clear it when the param isn't required
  - a number input bounded by `range` for `integer`
  - a select over `options` for `choice`, in declared order, with an empty entry when the param isn't required
  - every value submitted as a string
- [x] `src/routes/AppDetail.tsx`: give the stored-parameter editor the same three controls. Extract a shared field component rather than duplicating `ParamFields`' branches
- [x] `src/lib/params.ts`: hold back an `integer` value outside its range in `paramsBlocker` (the inputs themselves live in `KindInput`)

### Scripts and docs

- [x] `apps/tamanu-central.seed.rhai`: declare `fhir-enabled` as `boolean`, and replace `fhir_enabled.value() == "true"` with `fhir_enabled.parsed()`. Check the other demo parameters for natural `integer` or `choice` kinds
- [x] Update every changed call site in `apps/*.rhai` that reads an `email` action param, which now arrives as a map
- [x] `docs/bsl-scripting.md`: cover kind maps, the three new kinds and their parsers, `parsed()`, typed param maps and typed validator arguments, and native defaults

### Tests

- [x] `crates/core/src/tests/param.rs`:
  - `kind` with a string and with a map
  - rejected kind maps: unknown key, argument on the wrong kind, empty range, empty `options` or an `""` option, duplicate options
  - `parsed()` for each new kind, plus the case where it throws when a later kind rejects the value
  - a default the kind rejects fails evaluation, as does a native default on a mismatched kind, and `default_value("")` clears
  - validators receive parser results, and none run while any kind check fails
- [x] `crates/core/src/tests/stdlib.rs`:
  - canonical and rejected spellings for each new parser
  - `parse_integer` with and without `range`, including the out-of-range error naming the range
  - `parse_choice` without `options` throws
- [x] `crates/core/src/tests/action.rs`:
  - closures receive a `bool`, an `int` and the email map
  - `invoke(#{ flag: true, n: 5 })` is accepted, a native value for a mismatched kind is rejected, and an unset optional param is absent
- [x] `action_params.rs` unit tests:
  - JSON bool and i64 accepted for the matching kinds
  - `1.0`, `1e3` and `9223372036854775808` rejected
  - parser results written back into the map
- [x] `oi/handler/params.rs` tests: native values parsed and canonicalised, and rejected for a mismatched kind or an unknown parameter
- [x] Probe tests: parser results reach the closure, and a kind rejection is an error in strict mode and a skip in lenient mode
- [x] Schema JSON tests: `range` with `null` ends for `1..`, `1..=` and `..=5`, and `options` in declared order
- [x] Frontend: `ParamFields.test.tsx` and `params.test.ts` for the toggle, the bounded number input and the select, including clearing an optional value
- [x] Run `cargo clippy`, `cargo fmt`, both spec checks, the core tests and the frontend tests at the end
