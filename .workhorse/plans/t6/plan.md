# Check a definition bundle offline from the CLI

## Tech notes

- Blocked on splitting `seedling-core` into a portable core and a Linux platform crate (recorded on this card as needing a card of its own).
  `seedling-ctl` ships for Linux, macOS and Windows, so it can only link the evaluator once core builds on all three, Windows included.
- The check runs the server's own path: `evaluate_validated` with the `--set` values as both stored and proposed values, which covers `finish_evaluation`'s whole-definition checks, kind checks and validators.
- The app seed is drawn fresh for each check, as a seed for any app is; derived values play no part in whether a definition is valid.
- Host constants come from `SystemFacts`, with `--const` overrides applied over its defaults; `AVAILABLE_MEMORY`'s default needs a source on macOS and Windows, since `/proc/meminfo` exists only on Linux.

## Steps

- [ ] `definitions check` in `seedling-ctl`: source reading shared with `apps create`, version requirement, bundle rules, evaluation, validation, exit status
- [ ] `--set` and `--const` parsing and their errors
- [ ] Seedling's own CI runs `definitions check` over every definition under `apps/`
- [ ] Document the command in `docs/`
