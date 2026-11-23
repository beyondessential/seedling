# Set up the std module for BSL helpers

## Decisions

- A script cannot list the module's helpers. A definition states the Seedling versions it supports in `seedling.toml`, which settles which helpers it can call.
- The module ships empty.
- Rhai's `import` is disabled: its default resolver reads `.rhai` files from the daemon's filesystem, which breaks replicability and could shadow `std`. A bundle already combines its script files, so nothing in BSL needs it.
- Rhai's `sleep()` is removed: it pauses evaluation without costing operations, so it escapes the engine limits.
- Replicability is a language-level rule in the BSL overview, not only a helper convention, so the engine drops Rhai's `timestamp()` (the `no_time` feature removes it and the `Instant` type with it).

## Build

- [ ] Register an empty `std` static module on the engine in `setup_language`, so every context that evaluates through it reaches the module.
- [ ] Enable Rhai's `no_time` feature; check no script, test or fixture calls `timestamp()`.
- [ ] Replace the engine's module resolver with one that refuses every path.
- [ ] Remove `sleep` from the engine (it is in Rhai's core package behind `no_std` only, so build the engine from `new_raw` plus the packages minus it, or register a throwing override); check no script calls it.
- [ ] Tests: `std::` with an unknown name raises a script error; `timestamp()` and `sleep()` are not in scope; `import` raises a script error.
- [ ] Add a "Standard helpers" section to `docs/bsl-scripting.md` introducing the module, its calling form and its conventions.
