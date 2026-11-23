# Set up the std module for BSL helpers

## Decisions

- A script cannot list the module's helpers. A definition states the Seedling versions it supports in `seedling.toml`, which settles which helpers it can call.
- The module ships empty.
- Rhai's `import` keyword is disabled: its default resolver reads `.rhai` files from the daemon's filesystem, which breaks replicability and could shadow `std`. A bundle already combines its script files, so nothing in BSL needs it.
- Rhai's `sleep()` is removed: it pauses evaluation without costing operations, so it escapes the engine limits.
- Replicability is a language-level rule in the BSL overview, not only a helper convention, so the engine drops Rhai's `timestamp()` (the `no_time` feature removes it and the `Instant` type with it).

## Build

- [x] Register an empty `std` static module in `defs::register`, which every engine goes through, so every context reaches it.
- [x] Enable Rhai's `no_time` feature; no script, test or fixture called `timestamp()`.
- [x] Disable the `import` keyword, so a script containing it fails to compile. A refusing module resolver would miss an `import` whose alias is unused inside a block, which Rhai's optimiser drops before it runs.
- [x] Make `sleep` throw: `disable_symbol` only reaches keywords and `sleep` is in Rhai's core package, so the engine registers throwing `sleep` overloads, which it finds before any package's.
- [x] Tests: `std::` with an unknown name raises a function-not-found error in the static, action, gate-body and validator contexts, and a script cannot add to `std`; `timestamp()` is not in scope; `sleep()` throws without pausing; `import` is refused.
  - [x] `std`, `timestamp()` and `sleep()`
  - [x] `import`
- [x] Add a "Standard helpers" section to `docs/bsl-scripting.md` introducing the module, its calling form and its conventions, and a replicability note under the language fundamentals.
