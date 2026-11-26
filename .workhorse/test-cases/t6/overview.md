# Check a definition bundle offline from the CLI

Scenarios for `seedling-ctl definitions check`. Every case runs with no daemon running and no `--endpoint` reachable, so nothing passes by talking to a server. Exit statuses: 0 passed, 1 failed check or unreadable source, 2 invalid command line.

## Sources

- [x] Check `apps/postgres.seed.rhai` as a file source, with `--set` values for its required parameters and `--const AVAILABLE_MEMORY`, since it reads it: exits 0 with no output (verifies spec: CLI#definition-checks)
- [x] Check a folder holding `seedling.toml` with a two-file `script` list: both files are evaluated in order, exits 0 (verifies spec: CLI#definition-checks)
- [ ] Check a folder with a `.git` directory and a `.seedignore` excluding a scratch file that holds invalid UTF-8: the excluded paths play no part, exits 0 (verifies spec: CLI#definition-folders)
- [ ] Check a folder holding a symlink not left out by `.seedignore`: error names the symlink, nothing is evaluated (verifies spec: CLI#definition-folders)
- [x] Check a folder whose `include` table copies a script from a sibling directory in the same repository: the included script is evaluated, exits 0 (verifies spec: CLI#include-assembly)
- [x] Check a folder whose `include` key names a source that does not exist and that the folder holds no copy of: `source` error at `seedling.toml` naming the key, nothing evaluated (verifies spec: CLI#include-assembly, CLI#check-results)
- [x] Check a file source named `seedling.toml`: `source` error, nothing evaluated, exits 1 (verifies spec: CLI#definition-sources)
- [ ] Check a `https://github.com/<owner>/<repo>/tree/<ref>/<folder>` URL: the folder at that ref is downloaded and checked, error locations are bundle paths (verifies spec: CLI#definition-sources, CLI#check-results)
- [ ] Check `--ref` naming a definition manifest on a local test registry: the bundle is fetched and checked, exits 0 (verifies spec: CLI#definition-checks)
- [ ] Check `--ref` naming an image index with entries for `<0.1`, `>=0.1` and no annotation: the entry with the highest minimum the CLI's version satisfies is the one checked (verifies spec: CLI#definition-checks, DEFN#artefact-selection)
- [ ] Check `--ref` on a registry that requires credentials, with them in `~/.docker/config.json` and then in `$XDG_RUNTIME_DIR/containers/auth.json`: both authenticate (verifies spec: CLI#definition-checks)
- [x] Check `--ref example/def:1` (no explicit registry host) and `--ref ghcr.io/x/y` (no tag or digest): exits 2, and the test registry records no request (verifies spec: CLI#definition-checks)
- [ ] Check `--ref` against an unreachable registry and against a missing tag: `source` error naming the cause, exits 1 (verifies spec: CLI#check-results)
- [x] Give both a source and `--ref`: refused before checking anything (verifies spec: CLI#definition-sources)

## Order of checks

- [x] A bundle whose `seedling.toml` has `seedling = ">=999"` and also an unknown field: one `version` error naming the requirement and the CLI's version, located at `seedling.toml`, exits 1 (verifies spec: CLI#definition-checks, CLI#check-results)
- [ ] `--ref` to an index whose every entry requires a later Seedling: `version` error naming each entry's requirement, with no location (verifies spec: CLI#check-results)
- [ ] `--ref` to a manifest whose `vnd.bes.seedling.versions` annotation differs from the bundle's requirement: `bundle` error, exits 1 (verifies spec: CLI#check-results)
- [x] A bundle whose `script` list names a missing file: `bundle` error naming the path, nothing evaluated (verifies spec: CLI#definition-checks, BND#metadata-file)
- [x] A bundle whose metadata table holds `nan`: `bundle` error located at `seedling.toml` (verifies spec: CLI#check-results, BND#metadata-table)
- [x] A bundle over 4,096 files: `bundle` error, nothing evaluated (verifies spec: DEFN#bundle-rules)
- [x] A script with a syntax error on line 12 of its second script file: `script` error at that file and line 12, not the concatenated line (verifies spec: CLI#check-results, BND#script-formation)
- [ ] A script calling a BSL method with an argument of the wrong type: `script` error naming the argument, exits 1 (verifies spec: CLI#definition-checks, BSL#arguments)
- [ ] A script whose deployments order after each other in a cycle: `script` error naming the cycle (verifies spec: CLI#definition-checks)
- [ ] A script declaring a gate whose exclusive selection breaks the gate rules: `script` error (verifies spec: CLI#definition-checks)
- [x] A script containing an `import` statement that never runs: `script` error (verifies spec: BSL#evaluation)
- [x] A script that fails evaluation and also has `--set` values a validator would reject: only the `script` error is reported (verifies spec: CLI#definition-checks)

## Sample parameter values

- [x] A script that throws only when parameter `mode` is set to `cluster`: passes without `--set`, fails with `--set mode=cluster` (verifies spec: CLI#sample-parameter-values)
- [x] `--set port=abc` for an integer-kind parameter that also has a validator: one `validation` error naming `port` from the kind check, and the validator does not run (verifies spec: CLI#sample-parameter-values, PARAM#kind-checks)
- [x] Two `--set` values each rejected by its validator: two `validation` errors, one per parameter, exits 1 (verifies spec: CLI#check-results)
- [x] `--set dtaabase_url=x` where the definition declares only `database_url`: `validation` error saying `dtaabase_url` is not declared (verifies spec: CLI#sample-parameter-values)
- [x] `--set mode` (no `=`) and `--set a=1 --set a=2`: each exits 2 and evaluates nothing (verifies spec: CLI#sample-parameter-values, CLI#check-results)
- [x] `--set url=a=b`: the parameter `url` gets `a=b` (verifies spec: CLI#sample-parameter-values)

## Host constants

- [x] A script that throws unless `TIMEZONE == "UTC"`, `NODE_NAME == ""` and `HOST_HAS_IPV4`, `HOST_HAS_IPV6`, `NAT64_ACTIVE` and `HAS_SNAPSHOTS` are all false: passes with no `--const` (verifies spec: CLI#check-host-constants)
- [ ] A script that prints `AVAILABLE_THREADS` and `CPU_ARCHITECTURE` into a throw: the values match what the daemon on the same Linux machine gives a script (verifies spec: CLI#check-host-constants, CONST#host-constants)
- [x] `--const CPU_ARCHITECTURE=aarch64 --const AVAILABLE_MEMORY=2147483648 --const HOST_HAS_IPV6=true --const TIMEZONE=Pacific/Auckland`: the script sees each value (verifies spec: CLI#check-host-constants)
- [x] `--const AVAILABLE_THREADS=0`, `--const AVAILABLE_MEMORY=lots`, `--const HOST_HAS_IPV4=yes`, `--const CPU_ARCHITECTURE=`, `--const TIMEZONE=Mars/Olympus`: each exits 2 and evaluates nothing (verifies spec: CLI#check-host-constants)
- [x] `--const IDLE_CMD=x`, `--const NOT_A_CONSTANT=1`, and the same constant given twice: each exits 2 (verifies spec: CLI#check-host-constants)
- [ ] On each of the Linux, macOS and Windows builds, a script that reads `AVAILABLE_MEMORY`: with no `--const` the evaluation fails as a `script` error at the line that reads it; with `--const AVAILABLE_MEMORY=2147483648` it passes (verifies spec: CLI#check-host-constants)
- [x] A closure that captures `AVAILABLE_MEMORY`: with no `--const` the `script` error lies at the line that creates it (verifies spec: CLI#check-host-constants)
- [x] A script that reads `AVAILABLE_MEMORY` only inside a function it calls: with no `--const` it fails as it does with the constant given, at the line in the function that reads it (verifies spec: CLI#check-host-constants)
- [x] A script that never reads `AVAILABLE_MEMORY`, or reads it only in a branch not taken: with no `--const` it passes (verifies spec: CLI#check-host-constants)
- [x] A script that declares its own `AVAILABLE_MEMORY` variable and reads it: with no `--const` it passes (verifies spec: CLI#check-host-constants)
- [ ] On the macOS and Windows builds, a script that prints `AVAILABLE_THREADS` and `CPU_ARCHITECTURE` into a throw: both are the machine's own values (verifies spec: CLI#check-host-constants)

## Engine limits

- [x] A script looping 200,000 times: fails as a `script` error with the default limits, passes with `--script-max-operations 0` (verifies spec: CLI#definition-checks, LIM#limits)
- [x] The same script with `--script-max-operations 1000` fails where the default would also fail, and a script of 50,000 operations passes by default and fails at 1000 (verifies spec: CLI#definition-checks)
- [x] `--script-max-call-depth 0`: exits 2 and evaluates nothing (verifies spec: CLI#definition-checks, LIM#limits)
- [x] `definitions check --help` lists every `--script-max-*` option the daemon has, with the same defaults (verifies spec: CLI#definition-checks)

## No runtime effects

- [x] A definition with an action and a gate whose closures throw at once: passes, since neither closure runs (verifies spec: CLI#definition-checks)
- [x] A definition using `rt` only inside an action: passes with no container engine installed (verifies spec: CLI#definition-checks)

## Output

- [x] Folder source `seedling/central` with a script error at line 7 of `lib/db.rhai`: stderr line begins `seedling/central/lib/db.rhai:7: ` (verifies spec: CLI#check-results)
- [x] File source `apps/broken.seed.rhai` with a script error at line 3: stderr line begins `apps/broken.seed.rhai:3: ` (verifies spec: CLI#check-results)
- [x] Folder source `seedling/central` including `lib/common.rhai` from `../shared/common.rhai`, with a script error at line 5 of it: the error names the shared file it was read from and line 5, not `seedling/central/lib/common.rhai` (verifies spec: CLI#check-results)
- [x] An error thrown inside a function defined on line 20 and called on line 40: the location is line 20 (verifies spec: CLI#check-results)
- [ ] A `validation` error and a `version` error from an index: printed without a location prefix (verifies spec: CLI#check-results)
- [x] `--json` on a passing bundle: stdout is exactly `{"errors":[]}` (whitespace aside), exits 0 (verifies spec: CLI#check-results)
- [x] `--json` on a script error: one object with `kind` `script`, `file`, `line`, `message` set and `parameter` null, exits 1 (verifies spec: CLI#check-results)
- [x] `--json` with two rejected sample values: two `validation` objects, each with `parameter` set and `file` and `line` null (verifies spec: CLI#check-results)
- [x] `--json --set novalue`: stdout empty, error on stderr, exits 2 (verifies spec: CLI#check-results)
- [ ] A GitHub Actions job running the check on a broken folder shows an inline annotation on the failing line

## Parity and CI

- [ ] A bundle with a script error, registered with `apps create` on a running daemon and checked with `definitions check`: the fault's message and the check's message name the same file, line and cause (verifies spec: CLI#definition-checks)
- [ ] An `apps update` with `--set` values the server refuses with `validation_failed`, then the same bundle and values through `definitions check`: the same parameters are rejected for the same reasons
- [x] Seedling's CI runs `definitions check` over every definition under `apps/`, and a deliberately broken one fails the job
- [ ] The release builds of `seedling-ctl` for macOS and Windows run `definitions check` on `apps/postgres.seed.rhai` and exit 0
