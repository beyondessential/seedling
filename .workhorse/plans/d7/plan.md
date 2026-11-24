# Free-form metadata in seedling.toml

Adds the `[metadata]` table to the bundle's metadata file, readable by scripts through `app.metadata()` and reported in definition provenance, per the definition bundles spec (BND#metadata-table) and the app definitions spec (DEFN#bundle-rules, DEFN#provenance-reporting).

## Design notes

- **Converted once, at bundle build.** The table is converted from TOML to a `serde_json::Map` in the strict metadata pass (`read_script` today), which already runs lazily so a bundle for a newer Seedling can still be held and reported as unsupported. JSON is the shape both consumers need: provenance reports it as is, and `app.metadata()` turns it into a `Dynamic` with the existing JSON-to-Rhai conversion in `runtime/barrier/replay.rs`, which keeps integers and floats apart.
- **Dates and times are converted by hand.** Serialising a `toml::Value` through serde turns a datetime into a private wrapper object, so the conversion matches on `toml::Value` and turns `Datetime` into its `Display` text, which writes `T` between date and time.
- **Non-finite floats are refused in the strict pass**, alongside the other metadata rules, so they surface as `bundle_invalid` when a definition is installed or a template stored.
- **Provenance never reports `{}` for a table it could not read.** Every stored bundle has passed the strict pass (installs call `check_installable`, templates call `script()`), but `Source::to_json` reads the table through a `Result`, so an unreadable table becomes an internal error rather than an empty object.
- **The history page reads the table from canonical bytes**, as it already does for the Seedling requirement, so it does not rebuild every listed bundle. `requirement_of_canonical` generalises to reading the metadata file's contents, and both the requirement and the table are parsed from them.
- **`last.metadata()` needs no gate-specific code.** `last` is an `App` built on the last success's bundle, so a method on `App` that reads `this.bundle` serves both.

## Bundle

- [ ] `crates/core/src/runtime/definition/bundle.rs`: replace the `script: Result<Script, BundleError>` field with a `Result` over a struct holding the `Script` and the metadata table (`serde_json::Map<String, Value>`), filled by the strict pass
- [ ] Accept the `metadata` field in the strict pass: refuse a non-table value with the existing "must be a …" wording, convert every value recursively (string, integer, float, boolean, datetime as text, array, table), and refuse a non-finite float naming its key path
- [ ] Add `Bundle::metadata()` returning the table, or the strict pass's error
- [ ] Replace `requirement_of_canonical` with a reader returning the metadata file's contents from canonical bytes, and parse the requirement and the table from those contents; keep the "files sorted by path, stop past `seedling.toml`" shortcut
- [ ] Cite `BND#metadata-table` and `DEFN#bundle-rules` on the conversion and the float check
- [ ] Tests in `bundle/tests.rs`: absent file and absent field give an empty table; every TOML type converts as the spec's table gives, including all four datetime types and nested arrays and tables; `metadata = 1` and `inf`/`nan` at depth are `bundle_invalid`; a bundle for an unsupported Seedling with a malformed table is still held and reported as unsupported; the canonical reader agrees with the built bundle

## Script access

- [ ] Make `json_value_to_rhai_dynamic` reachable from `defs` (move it beside the other boundary conversions in `defs` rather than widening `replay`'s visibility, and update its callers)
- [ ] `crates/core/src/defs/app/file.rs`: register `metadata` on `App`, returning a fresh map converted from `this.bundle.metadata()`, cited `BND#metadata-table`
- [ ] Tests in `crates/core/src/tests/`: `app.metadata()` in static, action and validator contexts; a missing key reads as `()`; mutating the returned map leaves a second call unchanged; `last.metadata()` in a gate body reads the last success's bundle after an update changed the table

## Provenance

- [ ] `crates/core/src/runtime/definition/provenance.rs`: add `metadata` to `to_json`/`to_json_with`, and make both fallible on an unreadable table
- [ ] Update the call sites in `oi/handler/apps.rs` (four) and `oi/handler/templates.rs` (one) to map the error to `internal`
- [ ] `crates/core/src/runtime/generations.rs`: `bundle_requirement` becomes a reader of both the requirement and the table from a stored bundle, and the history page's per-hash cache in `oi/handler/apps.rs` holds both
- [ ] Tests: provenance carries `metadata` for fetched and pushed definitions, `{}` without a table, and the history page reports the same table as the app detail

## Web console

- [ ] `crates/web/frontend/src/lib/types.ts`: add `metadata: Record<string, unknown>` to `DefinitionProvenance`, and update `AppDetail.fixtures.ts` and the component tests' fixtures

## Docs

- [ ] `docs/bsl-scripting.md`: add `[metadata]` to the `seedling.toml` section, with an example defaulting a version parameter from `app.metadata().version`
- [ ] `docs/deploying.md`: show a definition publishing `metadata.version`, and that `--set version=…` is then needed only to deploy a release other than the one the definition was published for
