# Duration parameter kind

Add the `duration` kind with its `Duration` value type, then move every BSL option that takes a time span from a number of seconds to a duration argument (a `Duration` or a duration string).
Specs: Durations (DUR), with the kind rows in Actions (ACN) and Parameters (PARAM), and the option specs for containers, jobs, deployments, gates, HTTP services and `rt`.

## Notes

- **Parsing.** Pre-check the text, then hand it to jiff's friendly-only `SpanParser` (which already refuses ISO 8601), then convert the `Span` with days as 24 hours. The pre-checks cover what the friendly grammar allows but the spec refuses: a leading `+`/`-`, a trailing `ago`, and any year or month designator, including a zero one such as `0mo` (the span's fields can't tell `0mo 5d` from `5d`). Control characters are already refused by the common single-line check, which leaves space as the only whitespace. Finish by checking the total against 2⁶³−1 ns.
- **Text form.** Write our own printer rather than configuring jiff's: the spec fixes days as the largest unit and `us` rather than `µs`, and jiff's duration printer stops at hours. Days, then h, m, s, ms, us, ns; `0s` for zero.
- **Internal units stay.** Each option converts the `Duration` to the unit it already stores (`u32`/`u64` seconds, `f64` seconds for the proxy), so summaries, the wire (`*_secs`, `try_duration`, `window`), translation to the container engine and Caddy, and replay records are unchanged.
- **Script-only `Duration` in defaults.** `SuppliedValue` gains a `Duration` case for `default_value` and action schema defaults only. `SuppliedValue::from_json` stays as it is, because a JSON client supplies durations as strings.

## Duration type and parser

- [x] Add `crates/core/src/defs/duration.rs`: `Duration` (a `u64` of nanoseconds, at most `i64::MAX`), its parser following the notes above, and its text form; register `defs::duration` in `defs.rs`
- [x] `CustomType` for `Duration`: `as_seconds`, `as_millis` (throwing when not whole), `as_seconds_f`, `as_millis_f`, `to_string`/`to_debug`, comparisons, and a script error on comparison with any other type, following `defs/version.rs`
- [x] Arithmetic operators: `+`, `-` (throwing when negative), `*` by an integer in both orders, `/` by an integer (rounding toward zero; throwing on zero or a negative); every overflow throws
- [x] `take_duration(what, value)` in `defs/take.rs`: a `Duration`, or a string parsed with the parser's error naming `what`; anything else, including a number, throws. Add `take_whole_seconds(what, value, range)`, built on it, for the whole-seconds options

## The kind

- [x] `ParamKindName::Duration` and `ParamKind::Duration { min, max, precision }` in `defs/param_def.rs`, with `arguments()` returning `min`, `max` and `precision`, and `kind_with`, `name()` and `parse()` arms
- [x] `DurationPrecision` (`d h m s ms us ns`), with `take_kind` reading the three arguments through `take_duration` and a precision taker, and refusing a kind whose `min` exceeds its `max` or whose bounds hold no whole multiple of the precision unit
- [x] `ParsedValue::Duration`, with `into_dynamic` giving the `Duration`; out-of-bounds errors state the bound
- [x] `std::parse_duration(value)` and `std::parse_duration(value, args: map)` in `defs/stdlib/kinds.rs`, where the map holds only `min`, `max` and `precision`, with the same validity checks as `take_kind`
- [x] `SuppliedValue::Duration`: `take` accepts one, `canonical` gives its text form, and `check_type` lets it through only for the `duration` kind (`defs/param_def.rs`, `defs/app/param.rs`, action schema reading in `defs/param.rs`)

## Wire and interfaces

- [x] `oi/handler/appdef_json.rs`: a `duration` kind's bounds go in `range` as text-form strings, plus a new `precision` member on every param schema; the same in the templates handler output
- [x] Web frontend: `precision` and the string-bounds `range` variant in `lib/types.ts`; a `duration` param keeps the single-line input, with its bounds shown in the helper text (`components/ParamFields.tsx`, `KindInput.tsx` as needed)

## Duration options

- [x] `container.stop_timeout`: take a whole-seconds duration from 1 s to 4294967295 s (`defs/container.rs`)
- [x] Health check `interval`, `timeout` and `start_period` through the whole-seconds taker, replacing `take_secs` (`defs/container.rs`)
- [x] `job.deadline`: a whole-seconds duration of at least 1 s (`defs/job.rs`)
- [x] Gate check `interval`: a whole-seconds duration of at least 1 s (`defs/gate.rs`)
- [x] Balance `try_duration` and `interval` through `take_duration`, kept as `f64` seconds, replacing `take_seconds` and `as_number` (`defs/service/proxy.rs`)
- [x] Rate-limit `window`: a duration from 1 ms to 1 h (`defs/service/proxy.rs`)
- [x] Barrier `scheduled`/`running`/`ready`/`terminated(deadline)` and `rt.stop(selection, deadline)` take a `Dynamic` read as a whole-seconds duration; a zero deadline throws, and `resolve_deadline` stops mapping 0 to the default (`runtime/barrier/runtime.rs`)
- [x] Remove `take_number` from `defs/take.rs` if no caller remains

## Fixtures, docs and tests

- [x] Convert numeric arguments to strings in `apps/postgres.seed.rhai`, `apps/tamanu-central.seed.rhai` and `apps/tamanu-facility.seed.rhai`
- [x] Convert the BSL in Rust tests: `tests/service.rs`, `tests/gate.rs`, `tests/barrier.rs`, `tests/runtime.rs`, `tests/job.rs`, `tests/container.rs`, `defs/service/proxy/tests.rs`, `defs/container/tests.rs`, `system/caddy/tests.rs`, `runtime/gates/**/tests.rs`, `runtime/desired/tests.rs`, and any other script fixture a test run flags
- [x] `tests/duration.rs`: syntax accepted and refused (sign, `ago`, ISO 8601, years and months including zero ones, overflow, control characters), text form round trips, getters, comparisons, arithmetic edges
- [x] `tests/param.rs` and `oi/handler/params/tests.rs`: the kind with each argument, invalid kind maps, `default_value` given a `Duration` (and refused on another kind), bounds and precision rejections through the params handler
- [x] Option tests: a string, a `Duration`, a number (refused), a fraction of a second on each whole-seconds option (refused), and a zero `rt` deadline (refused)
- [x] `appdef_json` and templates tests for the `range` and `precision` members; a frontend test for the duration helper text
- [x] `docs/bsl-scripting.md`: the duration kind and its arguments, `Duration` methods, and every option example moved to strings
- [x] `// spec: DUR#...` citations on the new code; run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh` and `etc/ci/check-defs-coercion.sh`
