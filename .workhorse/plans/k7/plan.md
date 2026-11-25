# Quantity parameter kind

Add the `quantity` kind and `std::parse_quantity`, which read a Kubernetes-style quantity as a whole signed 64-bit amount with an optional `range`. Move `container.memory` onto the same grammar, accepting either a quantity or an integer number of bytes. Specs: Actions (ACN) `#quantities`, `#param-kinds`, `#kind-arguments`, `#kind-parsers`, `#presentation-of-kinds`; Containers `#resource-limits`; Apps (APPS) `#parameters`; Parameters (PARAM) `#kind-checks`; parameter validation and action parameters.

## Notes

- **Parser.** The parser goes in a new `crates/core/src/defs/quantity.rs`, beside `url.rs` and `purl.rs`, and is written with winnow per the repo rule. The value is split into sign, digits with an optional `.`, and suffix. Suffixes are matched case-sensitively. `E` followed by an optional sign and digits is an exponent; `E` on its own is exa.
- **Exact amount.** The arithmetic uses `num-bigint` (the 0.4 line already in the dependency tree). The digits are held as an integer mantissa with a count of fractional digits, so that amount = mantissa × 10^(exponent − fraction digits) × 2^binary. First strip the mantissa's trailing zeros, then compute in checked `i128`:
  - A zero mantissa gives 0 whatever the exponent.
  - A negative power of ten that doesn't divide the mantissa means a non-whole amount.
  - Any overflow, or a result outside `i64`, means out of range.
  - Exponent digits too long to fit an `i64` count as out of range when positive and as not whole when negative.
  - Nothing is ever rounded or capped. That rules out a float path.
- **Suffix hint.** When the parse fails only because of the trailing letters, try mapping them to a suffix:
  - A lone `m` maps to `Mi`.
  - Any `i` form (`mi`, `MiB`, `Mib`) maps to the binary suffix in its proper case.
  - A trailing `B` or `b` on a decimal form maps to the decimal suffix in its proper case (`MB` → `M`, `KB` → `k`).
  - The error then reads like "m is milli; did you mean 512Mi?". Anything else gets the plain error.
- **Quantity form for bounds.** `format_quantity(i64)` writes an amount as its shortest exact form: among the suffixes it is a multiple of to two decimal places, and the plain integer, the shortest text wins, with ties going to binary over decimal and larger over smaller (`15.5Gi`, `1.5G`, `1230`). Range errors (`must be between 64Mi and 15.5Gi`) and the console's `mem:` chip both use it. `IntRange` gets a way to display its bounds through a formatter, so `integer` keeps decimal bounds and `quantity` gets quantity form.
- **Integer stand-ins.** `ParamKind::check_type` is the single place where a boolean or integer is matched to a kind. Letting `Quantity` take `SuppliedValue::Int` there covers integer `default_value`s (both builders), JSON integers for `/apps/params/set`, and action invocations, all at once.
- **Container memory type.** `memory` becomes a byte count end to end (`ContainerDef`, `ContainerSummary`, `ContainerSpec`), and podman gets `--memory=<bytes>`, since a bare number there is bytes. The container's desired-state comparison changes shape for any app that sets a limit. Seedling is unshipped, so this needs no migration.

## Core: the kind

- [x] `cargo add winnow` in `crates/core`, pinned to the full version
- [x] `crates/core/src/defs/quantity.rs`: the winnow grammar, exact amount, suffix hint, `format_quantity`; wire the module into `defs.rs`
- [x] `param_def.rs`: add `ParamKindName::Quantity` (`"quantity"`, arguments `["range"]`) and `ParamKind::Quantity { range }`, and handle them in `name()`, `kind_with` and `without_arguments`
- [x] `ParamKind::parse`: add a `Quantity` arm returning `ParsedValue::Int`, with the range error in quantity form
- [x] `ParamKind::check_type`: accept `SuppliedValue::Int` for `Quantity`
- [x] `take_range`: word its errors for any kind with a `range`, not only `integer`
- [x] `stdlib/kinds.rs`: register the one- and two-argument `std::parse_quantity`, the way `parse_integer` is registered
- [x] `oi/handler/appdef_json.rs`: emit `range` for a `Quantity` kind
- [x] Add `// spec:` citations on each new arm and registration

## Core: container memory

- [x] `defs/container.rs`: remove `validate_memory_limit`; `memory` takes a string (through the quantity parser) or an `INT`, each at least 1; any other type throws; the field becomes `Option<i64>`
- [x] `defs/summary.rs`: `ContainerSummary.memory` becomes `Option<i64>`
- [x] `system/types.rs` and `system/translate/container.rs`: `ContainerSpec.memory` holds bytes and translates to `--memory=<bytes>`; confirm podman reads a bare number as bytes
- [x] `oi/shells/volume_session.rs` and other `memory: None` constructors: unchanged, `None` still fits

## Console

- [x] `src/lib/quantity.ts`: the same grammar, exact amount (BigInt), suffix hint and `formatQuantity`
- [x] `src/lib/params.ts`: add `quantityProblem(value, range)` and block an invalid `quantity` in `paramsBlocker`
- [x] `QuantityAmount.tsx` and the text-field paths of `ParamFields.tsx`: a `quantity` text field with the `= n` resolved amount as an end adornment, the description on the helper line, and the error replacing it, as in the mockup (a text field rather than `KindInput`, so the edit actions sit inside the field after the amount)
- [x] `AppDetail.tsx` `ParamsSection`: the edit row's text field shows the amount and the kind's error, and `unstageable` blocks an invalid quantity
- [x] `src/lib/types.ts`: `container.memory` becomes `number | null`; `AppDetail.tsx` shows the `mem:` chip through `formatQuantity`; update `AppDetail.fixtures.ts`

## Docs

- [x] `docs/bsl-scripting.md`:
  - the kinds list
  - the kind arguments paragraph (`range` on `quantity`)
  - the kind-check summary
  - the operator-interface and JSON-integer sentences
  - the parser list and return values
  - a short quantity grammar paragraph
  - the `.memory("512Mi")` example and its comment

## Tests

- [x] `defs/quantity.rs` unit tests:
  - every suffix
  - exponents with and without a sign, `E` alone against `E3`
  - `.5Gi`, `5.Gi` and signs
  - `0`, `-0` and `0e99999999999999999999`
  - the `i64` bounds, with `8Ei` rejected
  - `1.1Ki`, `1e-3` and `1500e-3` rejected; `15000e-3` accepted
  - whitespace anywhere rejected
  - the hints for `512m`, `512MiB` and `1MB`
  - `format_quantity` cases
- [x] `tests/param.rs`: a `quantity` kind name and map with a `range`, invalid ranges, an integer default, `parsed()` returning an integer, and a range error in quantity form
- [x] `tests/stdlib.rs`: `parse_quantity` in the helper list; the one- and two-argument calls
- [x] `runtime/action_params.rs`: a JSON integer is accepted for a `quantity` param and checked against its range
- [x] `oi/handler/params/tests.rs`: a JSON integer set for a `quantity` parameter is stored as its decimal form
- [x] `oi/handler/apps/tests.rs`: `range` is emitted for a `quantity` param, and a container's `memory` is an integer
- [x] `tests/container.rs`: `512Mi` and an integer accepted; `512m` rejected with the hint; `0`, `-1Ki` and a float rejected
- [x] `system/translate/container/tests.rs`: `--memory=536870912`
- [x] Console: `lib/quantity` tests matching the Rust cases; `ParamFields.test.tsx` / KindInput covering the resolved amount, the range error and the hint; the `mem:` chip in the AppDetail tests

## Finish

- [x] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`
- [x] In `crates/web/frontend`: `npm test` and `npm run build`
