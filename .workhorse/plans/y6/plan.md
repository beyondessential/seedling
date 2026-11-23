# Pair every parameter kind with a parse function

Each parameter kind gets a `std::parse_<kind>` helper, and that helper's acceptance is the runtime's kind check, both on action and shell invocations and, new here, on stored app parameters during parameter validation. `text` and every other single-line kind refuse control characters and line separators.

## Technical notes

- `parse_text` decides by Unicode general category query (Cc, Zl, Zp) through a dependency that exposes general categories, not a code-point table of our own. The dependency maintains its table; we do not. `unicode-properties` with its `general-category` feature is the pick: it is from the unicode-rs project, like the `unicode-segmentation` and `unicode-normalization` crates already in the lockfile. Add it with `cargo add` and pin the full version.
- The runtime's kind check and the `std::parse_<kind>` helpers share one implementation per kind, so the two cannot drift. `run_requirements` in `crates/core/src/runtime/action_params.rs` calls the same functions the `std` module registers.
- Stored app parameters gain a kind check in the parameter validation path (`/apps/params/set`, `/apps/update`, `/apps/plan` rejections), before each validator; a value its kind rejects skips its validator.
- The parse lives on `ParamKind` (`ParamKind::parse(self, &str)`), and the `std` module registers a helper for every variant by iterating `ParamKind::ALL`, so a kind added later cannot ship without its helper. The helper's name is derived from `as_str()` with `-` written as `_`.
- Layout matches card V6's helpers: `crates/core/src/defs/stdlib.rs` holds `module()`, and each group of helpers is a submodule with a `register(&mut rhai::Module)`. This card adds `defs/stdlib/kinds.rs`, so whichever of the two lands second resolves a one-line conflict in `stdlib.rs`.
- The runtime check on stored parameters does not skip an empty value, unlike `run_requirements`, which treats an empty action param as absent.

## Parsers

- [ ] `cargo add unicode-properties --features general-category` in `crates/core`
- [ ] `ParamKind::ALL` and `ParamKind::parse` in `crates/core/src/defs/param_def.rs`, returning a `ParsedValue` (`Text(String)` or `Email { local, domain }`) or the reason as a string; cite `// spec: ACN#kind-parsers` per arm
  - [ ] Single-line rule shared by `text`, `email`, `password`, `weak-password` and `random`: refuse any char whose general category is Control, LineSeparator or ParagraphSeparator, naming the offending character by code point in the reason
  - [ ] Move `is_valid_email` and `is_strong_password` out of `action_params.rs` into the email and password arms; `email` splits at the first `@` into `local` and `domain`
  - [ ] `volume` checks the general name rules with the same validator `SiteVolumeName::new` uses
- [ ] `crates/core/src/defs/stdlib.rs` with `module()`, and `defs/stdlib/kinds.rs` registering `parse_<kind>` for every `ParamKind::ALL`; replace the empty `rhai::Module::new()` in `crates/core/src/defs.rs` with `stdlib::module()`
  - [ ] Take the argument as `Dynamic` and convert with `defs::take::take_string`, so a non-string raises the argument-rules error rather than "function not found"
  - [ ] A rejected value raises a runtime error carrying the parser's reason, so a validator that lets it propagate reports that reason
  - [ ] `Email` returns a `rhai::Map` with `local` and `domain`; every other kind returns the string unchanged

## Kind checks on invocations

- [ ] `run_requirements` in `crates/core/src/runtime/action_params.rs` replaces its per-kind `match` with `def.kind.parse(value)`, pushing `"{field}: {reason}"`; empty values keep skipping the check as now
- [ ] Confirm the `volume` arm's name check in `run_requirements` and `validate_volume_params`' own `SiteVolumeName` parse don't report one bad name twice; keep the existence check in `validate_volume_params`
- [ ] Re-cite kind code from `ACN#param-kinds` to `ACN#kind-parsers` where it implements a parser rule, including `crates/web/frontend/src/lib/params.ts` and `ParamFields.tsx`

## Kind checks on stored parameters

- [ ] `check_kinds` in `crates/core/src/runtime/apps.rs`, cited `// spec: PARAM#kind-checks`: for each parameter set in `proposed` that the evaluated `app.def.params` declares, run `kind.parse`, collecting a `Rejection` per failure; an empty value is checked
- [ ] `run_definition` runs `check_kinds` before `run_validators` and passes the rejected names so `run_validators` skips them; update the `Rejection` doc comment to cover kind rejections
- [ ] Confirm the fallback evaluation in `validate_proposed` (`crates/core/src/oi/handler/params.rs`) and `/apps/plan` (`crates/core/src/oi/handler/apps.rs`) pick up kind checks through `evaluate_validated` with no change of their own, and that registration and reload stay unchecked

## Tests

- [ ] `crates/core/src/tests/stdlib.rs`: every `ParamKind::ALL` member resolves to a `std::parse_<kind>` helper; `std::parse_weak_password` exists
- [ ] `tests/stdlib.rs`: `parse_text` refuses LF, CR, tab, NUL, ESC, DEL, U+0085, U+2028 and U+2029, and accepts accented letters and emoji; `parse_multiline` accepts line breaks and control characters
- [ ] `tests/stdlib.rs`: `parse_email` returns `local` and `domain`, splits `a@b@c.d` at the first `@`, refuses `@x.y`, `a@b`, `a@.b`, `a@b.` and an address with a line break; `parse_password` refuses `password` and accepts a strong passphrase; `parse_weak_password` accepts `a`; `parse_random` refuses a line break; `parse_volume` refuses `ab` and `-abc`
- [ ] `tests/stdlib.rs`: a non-string argument raises the argument-rules error; a parser called in a validator propagates its reason as the rejection reason; helpers are callable in the static, action and validator contexts
- [ ] `action_params.rs` unit tests: a `text` param with a line break is refused naming the field and the reason; `multiline` with one is accepted; an empty `email` param is absent rather than refused
- [ ] `crates/core/src/oi/handler/params/tests.rs`: `/apps/params/set` refuses an `email` parameter set to `not-an-email` and to `""` with `validation_failed` naming the parameter and the parser's reason; a validator on a kind-rejected value does not run; unsetting with `null` passes
- [ ] Same file or `apps` handler tests: `/apps/update` changing a stored parameter's kind from `text` to `email` with a non-email value is refused; `/apps/plan` lists the kind failure under `rejections`

## Docs

- [ ] `docs/bsl-scripting.md`: `text` is single-line in the kind comment by `.kind("text")`; a section on the `std::parse_<kind>` helpers, what `parse_email` returns, and that stored parameters are kind-checked before their validator
