# Check a definition bundle offline from the CLI

## Tech notes

- Built on T7's split of `seedling-core` into a portable core and `seedling-linux`.
  `seedling-ctl` ships for Linux, macOS and Windows and links only the portable core on all three; `seedling-linux` stays out of it, keeping the CLI free of libsystemd.
- The check runs the server's own path: `evaluate_validated` with the `--set` values as both stored and proposed values, which covers `finish_evaluation`'s whole-definition checks, kind checks and validators.
- The app seed is drawn fresh for each check, as a seed for any app is; derived values play no part in whether a definition is valid.
- Host constants start from core's `SystemFacts::default`, which leaves memory undetermined, with threads from `sysconst::detect_available_threads` and the architecture from the standard library, then take the `--const` overrides.
  Reading `/proc/meminfo` stays in `seedling-linux`, so the check has no memory default on any platform.
  The check leaves undetermined memory out of the scope instead of failing the evaluation, and the engine's variable resolver throws where a script reads it, so the error lands at that line. Which of the two an undetermined constant does is a field of `SystemFacts`, so the runtime keeps failing the whole evaluation.
- The engine-limit options are defined once, in the portable core, and used by both the daemon and `definitions check`, so their names and defaults cannot drift apart.
- `--ref` reuses `runtime/definition/fetch.rs` and `auth.rs` from the portable core, with the CLI's own version as the running version; `auth_file_candidates` already searches the right files off Linux.
- `ScriptError` is a string, with `Bundle::remap_error` rewriting positions in the text. The check's `file:line` and JSON `file`/`line` need the error's own position as data, so the evaluator carries the remapped file and line of the error's position alongside the message, rather than the check parsing them back out of the text.

## Steps

- [x] `definitions check` in `seedling-ctl`: source reading and include assembly shared with `apps create`, `--ref` fetched by the CLI, version requirement, bundle rules, evaluation, validation, exit status
- [x] `--set`, `--const` and engine-limit options, and their errors
- [x] Seedling's own CI runs `definitions check` over every definition under `apps/`
- [x] Document the command in `docs/`
