# Quantity parameter kind

Add the `quantity` kind and `std::parse_quantity`, which read a Kubernetes-style quantity as a whole signed 64-bit amount with an optional `range`. Move `container.memory` onto the same grammar, accepting either a quantity or an integer number of bytes. Specs: Actions (ACN) `#quantities`, `#param-kinds`, `#kind-arguments`, `#kind-parsers`, `#presentation-of-kinds`; Containers `#resource-limits`; Apps (APPS) `#parameters`; Parameters (PARAM) `#kind-checks`; parameter validation and action parameters.

## Notes

- **Parser.** The parser goes in a new `crates/core/src/defs/quantity.rs`, beside `url.rs` and `purl.rs`, and is written with winnow per the repo rule. The value is split into sign, digits with an optional `.`, and suffix. Suffixes are matched case-sensitively. `E` followed by an optional sign and digits is an exponent; `E` on its own is exa.
- **Exact amount.** The digits are held as an integer mantissa with a count of fractional digits, so that amount = mantissa × 10^(exponent − fraction digits) × 2^binary. First strip the mantissa's trailing zeros, then compute in checked `i128`:
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
- **Quantity form for bounds.** `format_quantity(i64)` writes an amount as the largest binary suffix whose quotient has at most two decimal places exactly (`15.5Gi`). Failing that, it uses the largest such decimal suffix. Failing that, it writes the plain integer. Range errors (`must be between 64Mi and 15.5Gi`) and the console's `mem:` chip both use it. `IntRange` gets a way to display its bounds through a formatter, so `integer` keeps decimal bounds and `quantity` gets quantity form.
- **Integer stand-ins.** `ParamKind::check_type` is the single place where a boolean or integer is matched to a kind. Letting `Quantity` take `SuppliedValue::Int` there covers integer `default_value`s (both builders), JSON integers for `/apps/params/set`, and action invocations, all at once.
- **Container memory type.** `memory` becomes a byte count end to end (`ContainerDef`, `ContainerSummary`, `ContainerSpec`), and podman gets `--memory=<bytes>`, since a bare number there is bytes. The container's desired-state comparison changes shape for any app that sets a limit. Seedling is unshipped, so this needs no migration.

## Core: the kind

- [ ] `cargo add winnow` in `crates/core`, pinned to the full version
- [ ] `crates/core/src/defs/quantity.rs`: the winnow grammar, exact amount, suffix hint, `format_quantity`; wire the module into `defs.rs`
- [ ] `param_def.rs`: add `ParamKindName::Quantity` (`"quantity"`, arguments `["range"]`) and `ParamKind::Quantity { range }`, and handle them in `name()`, `kind_with` and `without_arguments`
- [ ] `ParamKind::parse`: add a `Quantity` arm returning `ParsedValue::Int`, with the range error in quantity form
- [ ] `ParamKind::check_type`: accept `SuppliedValue::Int` for `Quantity`
- [ ] `take_range`: word its errors for any kind with a `range`, not only `integer`
- [ ] `stdlib/kinds.rs`: register the one- and two-argument `std::parse_quantity`, the way `parse_integer` is registered
- [ ] `oi/handler/appdef_json.rs`: emit `range` for a `Quantity` kind
- [ ] Add `// spec:` citations on each new arm and registration

## Core: container memory

- [ ] `defs/container.rs`: remove `validate_memory_limit`; `memory` takes a string (through the quantity parser) or an `INT`, each at least 1; any other type throws; the field becomes `Option<i64>`
- [ ] `defs/summary.rs`: `ContainerSummary.memory` becomes `Option<i64>`
- [ ] `system/types.rs` and `system/translate/container.rs`: `ContainerSpec.memory` holds bytes and translates to `--memory=<bytes>`; confirm podman reads a bare number as bytes
- [ ] `oi/shells/volume_session.rs` and other `memory: None` constructors: compile-fix only

## Console

- [ ] `src/lib/quantity.ts`: the same grammar, exact amount (BigInt), suffix hint and `formatQuantity`
- [ ] `src/lib/params.ts`: add `quantityProblem(value, range)` and block an invalid `quantity` in `paramsBlocker`
- [ ] `KindInput.tsx` / `hasKindInput`: a `quantity` text field with the `= n` resolved amount as an end adornment, the description on the helper line, and the error replacing it, as in the mockup
- [ ] `AppDetail.tsx` `ParamsSection`: route `quantity` through `KindInput` (edit actions stay after the adornment), and have `unstageable` block an invalid quantity
- [ ] `src/lib/types.ts`: `container.memory` becomes `number | null`; `AppDetail.tsx` shows the `mem:` chip through `formatQuantity`; update `AppDetail.fixtures.ts`

## Docs

- [ ] `docs/bsl-scripting.md`:
  - the kinds list
  - the kind arguments paragraph (`range` on `quantity`)
  - the kind-check summary
  - the operator-interface and JSON-integer sentences
  - the parser list and return values
  - a short quantity grammar paragraph
  - the `.memory("512Mi")` example and its comment

## Tests

- [ ] `defs/quantity.rs` unit tests:
  - every suffix
  - exponents with and without a sign, `E` alone against `E3`
  - `.5Gi`, `5.Gi` and signs
  - `0`, `-0` and `0e99999999999999999999`
  - the `i64` bounds, with `8Ei` rejected
  - `1.1Ki`, `1e-3` and `1500e-3` rejected; `15000e-3` accepted
  - whitespace anywhere rejected
  - the hints for `512m`, `512MiB` and `1MB`
  - `format_quantity` cases
- [ ] `tests/param.rs`: a `quantity` kind name and map with a `range`, invalid ranges, an integer default, `parsed()` returning an integer, and a range error in quantity form
- [ ] `tests/stdlib.rs`: `parse_quantity` in the helper list; the one- and two-argument calls
- [ ] `runtime/action_params.rs`: a JSON integer is accepted for a `quantity` param and checked against its range
- [ ] `oi/handler/params/tests.rs`: a JSON integer set for a `quantity` parameter is stored as its decimal form
- [ ] `oi/handler/apps/tests.rs`: `range` is emitted for a `quantity` param, and a container's `memory` is an integer
- [ ] `tests/container.rs`: `512Mi` and an integer accepted; `512m` rejected with the hint; `0`, `-1Ki` and a float rejected
- [ ] `system/translate/container/tests.rs`: `--memory=536870912`
- [ ] Console: `lib/quantity` tests matching the Rust cases; `ParamFields.test.tsx` / KindInput covering the resolved amount, the range error and the hint; the `mem:` chip in the AppDetail tests

## Finish

- [ ] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`
- [ ] In `crates/web/frontend`: `npm test` and `npm run build`
