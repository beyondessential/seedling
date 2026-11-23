# Pair every parameter kind with a parse function

## Technical notes

- `parse_text` decides by Unicode general category query (Cc, Zl, Zp) through a dependency that exposes general categories, not a code-point table of our own. The dependency maintains its table; we do not. Add it with `cargo add` and pin the full version.
- The runtime's kind check and the `std::parse_<kind>` helpers share one implementation per kind, so the two cannot drift. `run_requirements` in `crates/core/src/runtime/action_params.rs` calls the same functions the `std` module registers.
- Stored app parameters gain a kind check in the parameter validation path (`/apps/params/set`, `/apps/update`, `/apps/plan` rejections), before each validator; a value its kind rejects skips its validator.
