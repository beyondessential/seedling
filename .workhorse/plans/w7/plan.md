# W7: move rhai past 1.25.1

## Findings

- The data race the 1.25.1 hold names no longer occurs in BSL scripts. The `on_var` resolver for `app` (G7) hands each read of `app` a fresh copy, so a closure never shares `app` with the method it is passed to. No BSL language change is needed.
- What fails on rhai 1.27.0 is "Function not found: anon$…". The two places that run a stored closure (`runtime/barrier/replay.rs` and `runtime/probe.rs`) compile a separate `__bsl_closure.call(...)` script, and since upstream commit 648c6848 a `FnPtr` no longer carries the functions of the script that made it. rhai main fails the same way, so this is fixed on Seedling's side.
- 1.27.0 has an aliasing UB regression fixed on rhai main and listed for 1.27.1, so the dependency is a pinned git commit on main until 1.27.1 is published.
- A residual race remains for a script function that takes `app` as a parameter and registers a closure capturing it. It fails loudly at definition time and is left as is.

## Explicitly rejected

- Shipping rhai 1.27.0 from crates.io: carries the UB regression.
- Adopting the Grain VM: opt-in, aimed at footprint and precompiled bytecode, and BSL scripts are dominated by native calls it does not speed up.

## First PR

- [ ] Run stored closures against the definition's functions, through one helper shared by replay and probe
- [ ] Pin rhai to a commit on main; replace the hold comment with the reason and the switch back to `=1.27.1`
- [ ] Drop the stale dependabot ignore for `rhai >= 1.26`
- [ ] Turn on rhai's `compact_str` feature and drop the `smartstring` patch (root and fuzz workspace)
- [ ] clippy, fmt, spec style and citation checks; core tests

## After the first PR merges

- [ ] Measure `setup_language` (engine build, BSL type registration, scope)
- [ ] If engine building is a real cost, share prebuilt packages across engines (rhai's standard package, and Seedling's own registrations where possible)
- [ ] Switch rhai back to `=1.27.1` from crates.io once it is published and past the 7-day publish age
