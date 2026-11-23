# Set up the std module for BSL helpers

## Decisions

- A script cannot list the module's helpers. A definition states the Seedling versions it supports in `seedling.toml`, which settles which helpers it can call.
- The module ships empty.
- Replicability is a language-level rule in the BSL overview, not only a helper convention, so the engine drops Rhai's `timestamp()` (the `no_time` feature removes it and the `Instant` type with it).

## Build

- [ ] Register an empty `std` static module on the engine in `setup_language`, so every context that evaluates through it reaches the module.
- [ ] Enable Rhai's `no_time` feature; check no script, test or fixture calls `timestamp()`.
- [ ] Tests: `std::` with an unknown name raises a script error; `timestamp()` is not in scope.
- [ ] Add a "Standard helpers" section to `docs/bsl-scripting.md` introducing the module, its calling form and its conventions.
