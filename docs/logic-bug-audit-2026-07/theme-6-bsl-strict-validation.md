# Theme 6: Silent coercion in the BSL defs layer

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 6.

## The failure pattern

The defs layer (`crates/core/src/defs/`) is where BSL scripts hand values to Rust via rhai. Every value crossing that boundary arrives as a `rhai::Dynamic` (or a rhai-typed `i64`/`Array`/`Map`), and the layer has two coexisting styles for consuming it:

**The checked style** converts and throws. `take_retries` (`container.rs:241-255`) calls `as_int()`, maps the error to a message naming the field and the actual type ("healthcheck `retries` must be a positive integer, got {t}"), checks the sign, checks `n > u32::MAX as i64`, and only then casts. `Port::new` (`defs.rs:57-65`) uses `u16::try_from` and rejects zero. `validate_scale` (`deployment.rs:137-145`), `take_on_failure`, `parse_healthcheck`'s `kind` handling, and `EnvVar::new` all follow the same shape. Errors surface as `Box<EvalAltResult>`, which rhai turns into a script error at the exact call site — the script author sees the line that is wrong.

**The coercing style** swallows. `into_string().unwrap_or_default()` turns a non-string into `""` (`container.rs:468` for `command`, `:497` for `arg`, `:286` for healthcheck `cmd`, `:531-536` for `env` map fields). `try_cast`/`filter_map` drop values that fail the cast (`collection/selector.rs:20-44` drops whole criteria or individual elements; `container.rs:528` skips non-map `env` items). `into_string().ok()`/`as_bool().ok()` with a fallback default the field (`app/install.rs:56` defaults a malformed `kind` to text, `:74`/`:87` default `required`/`secret`, `app.rs:238` drops a non-string `description`). Unchecked `as` casts wrap (`container.rs:651` `pids_limit as u32`, `:690` `stop_timeout as u32`, `ingress.rs:198` redirect `code as u16` with no validation at all).

The coercing style is never asserted by a test or licensed by the spec. Its consequences are the worst kind: not a failure but a *different meaning*. `select(#{ types: ResourceType.Service })` — array brackets forgotten — drops the criterion and matches every resource in the app, so a follow-up `rt.stop` stops all workloads. `pids_limit(4294967297)` wraps to `1` and the container cannot start a workload. `command(["nginx", 8080])` produces an argv containing `""`. Each surfaces far from the cause, at reconcile or container-start time, with no pointer back to the script line.

## Affected findings

All findings for this theme are in [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) (core/defs).

| Finding | Section | Severity |
|---|---|---|
| Malformed `select` criteria silently invert to select-everything (or nothing) | [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) | medium |
| `pids_limit` and `stop_timeout` silently truncate i64 → u32 | [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) | medium |
| Redirect status code unvalidated and wrapped i64 → u16 | [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) | medium |
| Non-string `kind` and non-map entries in param schemas silently accepted | [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) | low |
| Non-string array elements silently become empty strings in command/arg/env/healthcheck cmd | [§2](../logic-bug-audit-2026-07.md#2-coredefs-bsl-app-definition-layer) | low |

Adjacent §2 findings fixed by the same discipline though not strictly coercion: the inverted `scale(5..2)` range (H20, a missing `min <= max` check in the otherwise-checked `validate_scale` path) and the digest "hex" check accepting `g`–`z` — both are validation gaps in the same functions the helper module would consolidate.

## Would a high-level change help?

**Yes, and this is the most mechanical theme in the report.** Every instance has the same shape (a `Dynamic` consumed without a throwing conversion), the same fix (convert-or-throw with a message naming the argument), and the correct pattern already exists in the same files — `take_retries` sits 30 lines above the unchecked `pids_limit` cast, and `take_on_failure`/`take_seconds`/`take_command_cmd` show the intended naming convention (`take_*`). Unlike themes 2–4, no architectural decision is needed: the layer already agrees errors are `Box<EvalAltResult>` and already has a pure-unit-test harness (`run_test_script_err` in `crates/core/src/tests.rs:39`). The work is extracting the checked style into shared helpers and pointing the ~17 coercing sites at them.

**The compatibility question.** Scripts that today "work" via silent coercion — a `select` with a scalar `types`, an `env` array containing a non-map, a `params` schema with `kind: 42` — will start throwing at evaluation time. That is exactly the desired outcome (the current behaviour is the bug), but it is a behaviour change: an `/apps/update` that previously succeeded can now fail, and it needs a release note. Two mitigating facts: first, any script relying on the coercion was already misbehaving (matching everything, running with empty argv elements, or installing password params as plain text); second, the failure mode of a throw is well-defined — *provided C1 is fixed first*. Until `AppRegistry::reload` stops swapping in a partially-evaluated def on script error (the report's critical finding), a newly-throwing script during `/apps/update` would trigger the destructive post-reload path. **Sequence the C1 fix before or with this change.**

## Proposed pattern

A conversion-helper module `crates/core/src/defs/take.rs`, continuing the existing `take_*` naming:

```rust
use std::ops::RangeInclusive;

use rhai::{Dynamic, EvalAltResult};

/// "`{what}` must be a string, got {type}"
pub fn take_string(what: &str, v: Dynamic) -> Result<String, Box<EvalAltResult>>;

/// Throws if `v` is not an array or any element is not a string.
/// "`{what}` must be an array of strings; element 2 is a {type}"
pub fn take_string_array(what: &str, v: Dynamic) -> Result<Vec<String>, Box<EvalAltResult>>;

/// "`{what}` must be a boolean, got {type}"
pub fn take_bool(what: &str, v: Dynamic) -> Result<bool, Box<EvalAltResult>>;

/// "`{what}` must be a map, got {type}"
pub fn take_map(what: &str, v: Dynamic) -> Result<rhai::Map, Box<EvalAltResult>>;

/// Typed array take for enum criteria, e.g. take_array_of::<ResourceKind>.
pub fn take_array_of<T: rhai::Variant + Clone>(
    what: &str,
    v: Dynamic,
) -> Result<Vec<T>, Box<EvalAltResult>>;

/// Range-checked integer conversion for builders whose rhai signature is
/// already i64. "`{what}` must be between {min} and {max}, got {n}"
pub fn take_int_in_range<T: TryFrom<i64>>(
    what: &str,
    n: i64,
    range: RangeInclusive<i64>,
) -> Result<T, Box<EvalAltResult>>;
```

Conventions, matching the existing checked sites: the message always backtick-quotes the argument name (`what` is the field or builder name as the script author typed it, e.g. `"pids_limit"`, `"healthcheck cmd"`, `"select types"`), states the expected type/range, and reports the actual — `Dynamic::type_name()` for type mismatches (what `into_string()`'s `Err` already carries), the value for range failures. Elements of arrays report their index.

Concrete conversions:

- `pids_limit`: `take_int_in_range::<u32>("pids_limit", limit, 1..=u32::MAX as i64)`; likewise `stop_timeout`. `redirect(port, code)` gains `take_int_in_range::<u16>("redirect code", code, 300..=399)` (the spec should pin the accepted range; today literally any i64 is stored).
- `command`/`arg` array overloads and `take_command_cmd`: `take_string_array`.
- The `env` array overload: `take_map("env entry", item)?` then `take_string("env name", ...)` — a non-map item or non-string field throws instead of being skipped or emptied.
- `parse_param_defs` (`app/install.rs`): non-map entries throw ("param `pw` must be a map, got string"); `kind`, `default_value`, `description` use `take_string`; `required`/`secret` use `take_bool`; the `params` value itself uses `take_map` instead of `unwrap_or_default`.
- **`Selector::from_map` becomes fallible**: `pub fn from_map(map: &Map) -> Result<Self, Box<EvalAltResult>>`, using `take_array_of::<ResourceKind>("select types", ...)` and `take_string_array` for `names`/`name_patterns`, and throwing on unknown keys ("unknown select criterion `typo`") — spec `l[collection.select]` says "All possible keys are defined in this spec". `Collection::select` (`collection.rs:75`) and its `with_fn` registration (`collection.rs:98`) become `Result`-returning, which rhai supports directly. A malformed criterion is then a script error at the `select` call, never a select-everything.

Explicitly out of scope: the `try_cast` chains that are genuine *type dispatch* — `col()`'s successive casts in `collection.rs:110-209` (spec-mandated coercion with a defined fallback) and the string-vs-array dispatch in `take_command_cmd` (which ends in an else-throw). Dispatch that terminates in a throw is the checked style.

## What it prevents — and what it does not

**Prevents**: the entire class where a script type mistake changes meaning instead of failing — inverted selections driving workload control, wrapped resource limits, empty argv/env values, silently downgraded password params, nonsense redirect codes. It also makes the next builder safe by default: the path of least resistance becomes `take_string(...)?` rather than `.into_string().unwrap_or_default()`.

**Does not prevent**: *value*-level validation gaps where the type is right but the domain check is missing — the inverted `scale(5..2)` range (H20) needs a `min <= max` check no helper supplies automatically; the digest pseudo-hex check needs `is_ascii_hexdigit()`. Nor does it touch the §2 semantic findings (frozen-reference semantics for `external_service`, the `col(action)` leftover branch, the `secret(false)` tri-state) — those are model bugs, not coercion. And it cannot catch mistakes that are type-valid, e.g. `names: ["sevrice-a"]` selecting nothing; only runtime observability helps there.

## Migration path

Grep of `crates/core/src/defs/` (idiom occurrences, excluding legitimate dispatch): **6** `unwrap_or_default()` on converted `Dynamic`s (`container.rs:286,468,497,532,536`, `app/install.rs:29`), **8** `into_string().ok()`/`as_bool().ok()` silent defaults (`selector.rs:32,41`, `app/install.rs:56,74,79,83,87`, `app.rs:238`), **5** dropping `try_cast`/`filter_map` uses (`selector.rs:21,23,30,39`, `container.rs:528`), **3** unchecked `as u32`/`as u16` casts (`container.rs:651,690`, `ingress.rs:198`) — roughly 17 logical sites in five files.

Order, by blast radius of the bug being fixed:

1. `collection/selector.rs` — the workload-control hazard; makes `from_map` fallible.
2. `container.rs` — `pids_limit`, `stop_timeout`, the `command`/`arg`/`env`/healthcheck-`cmd` arrays.
3. `ingress.rs` — redirect code range.
4. `app/install.rs` (`parse_param_defs` and the `params` map take) and `app.rs` `extract_description`.
5. Fold the adjacent gaps into the same pass: `min <= max` in the `scale(Range)` overload, hexdigit check in `validate_image_ref`.

Each step is covered by the existing pure BSL eval harness: `run_test_script_err`/`run_test_script_app` in `crates/core/src/tests.rs`, with per-builder test files already present (`tests/container.rs`, `tests/deployment.rs`, `tests/ingress.rs`, `tests/collection.rs`, `tests/param.rs`, `tests/app.rs`). `Selector::from_map`/`matches` are additionally testable as plain functions.

## Enforcement

- **Tests**: one `run_test_script_err` case per converted builder asserting the malformed input throws and the message names the argument (e.g. `pids_limit(4294967297)`, `select(#{ types: "service" })`, `command(["nginx", 8080])`, `params: #{ pw: #{ kind: 42 } }`). These are the report's "easy" testability column.
- **Spec**: add a tracey item to `docs/spec/language.md`, e.g. `l[bsl.args.strict]`: *"A builder or function argument whose type or value does not match its documented signature raises a script error at evaluation time; malformed input is never coerced, defaulted, or silently ignored."* Annotate the `take` module with `l[impl bsl.args.strict]` and let per-builder items (`l[collection.select.*]`, `l[container.pids-limit]`, …) keep their existing annotations at the call sites. This states the *what* (throw on malformed input) without baking in the helper API.
- **Lint**: a workspace `clippy.toml` with `disallowed-methods = ["rhai::Dynamic::try_cast"]` is too blunt — `try_cast` is legitimate dispatch in `collection.rs`, `app/volume.rs`, and widely used in `runtime/`. Scope the ban instead: a step in the existing `rust.yml` lint job that greps `crates/core/src/defs/` for `unwrap_or_default\(\)` (after a `Dynamic` conversion), `into_string\(\)\.ok\(\)`, `as_bool\(\)\.ok\(\)`, `filter_map.*try_cast`, and `as u(16|32)` outside `defs/take.rs`, failing with a pointer to this document. Sites that are genuine dispatch carry a `#[expect(clippy::disallowed_methods, reason = "...")]`-style marker comment the grep allowlists (`// take: dispatch`). Crude, but the defs layer is small, the idiom is textual, and the grep is exactly how this audit found the sites.
