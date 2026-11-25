# Check a definition bundle offline from the CLI

## Tech notes

- Blocked on splitting `seedling-core` into a portable core and a Linux platform crate (recorded on this card as needing a card of its own).
  `seedling-ctl` ships for Linux, macOS and Windows, so it can only link the evaluator once core builds on all three, Windows included.
- The check runs the server's own path: `evaluate_validated` with the `--set` values as both stored and proposed values, which covers `finish_evaluation`'s whole-definition checks, kind checks and validators.
- The app seed is drawn fresh for each check, as a seed for any app is; derived values play no part in whether a definition is valid.
- Host constants come from `SystemFacts`, with `--const` overrides applied over its defaults; `AVAILABLE_MEMORY`'s default needs a source on macOS and Windows, since `/proc/meminfo` exists only on Linux.

- The engine-limit options are defined once, in the portable core, and used by both the daemon and `definitions check`, so their names and defaults cannot drift apart.
- `--ref` reuses `runtime/definition/fetch.rs` and `auth.rs` from the portable core, with the CLI's own version as the running version.
  `auth_file_candidates` falls back to `/run/containers/<uid>/auth.json` through `rustix::process::getuid`, which needs a non-Unix path for Windows; `~/.docker/config.json` is the file that applies there and on macOS.
- `ScriptError` is a string, with `Bundle::remap_error` rewriting positions in the text. The check's `file:line` and JSON `file`/`line` need the error's own position as data, so the evaluator carries the remapped file and line of the error's position alongside the message, rather than the check parsing them back out of the text.

## Steps

- [ ] `definitions check` in `seedling-ctl`: source reading shared with `apps create`, `--ref` fetched by the CLI, version requirement, bundle rules, evaluation, validation, exit status
- [ ] `--set`, `--const` and engine-limit options, and their errors
- [ ] Seedling's own CI runs `definitions check` over every definition under `apps/`
- [ ] Document the command in `docs/`
