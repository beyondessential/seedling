# Report apps to Canopy as applications

Each app whose definition calls `app.canopy_type(type)` is reported to Canopy as an application keyed by its app name. The machine's four existing checks move into the report's `machine` section. An app that relies on Canopy is faulted with `canopy_offer_absent` once no offer has been live for fifteen minutes.

## Notes

- **Push format.** Canopy's current push has `machine: { health, detail }` and `applications: { <key>: { type, health, detail } }`. A push with no `machine` section is the transitional unified format, so sending `machine` is what makes Canopy read `applications` at all. `StatusPayload` in `bestool-canopy` is an opaque object, so no client upgrade is needed. Keep `source` and `healthy` at the top level.
- **One bad entry rejects the whole push.** Canopy returns 400 for the entire report if any application entry is malformed. The BSL slug check is what prevents that, so the report builder can trust `AppDef::canopy_type`.
- **`skipped` closes issues in Canopy.** That is why `Operating` re-sends the last graded result rather than `skipped`. The last grade lives in the database so a restart during an operation does not recover the check.
- **Could not determine is not "untyped".** If a stored definition fails to evaluate on reload (`script_error`), its row in `canopy_apps` stays put. Only a definition that evaluated and declares no type removes it.
- **`canopy_offer_absent` is retained, not a pure condition.** At startup the period has not elapsed (it runs from daemon start), so a sweep that cleared whenever the period hadn't elapsed would drop a held fault on every restart. The sweep only files. Clearing happens on the four named events.
- **ctl needs no code.** `ctl apps show` and `ctl canopy status` print the OI response as JSON, so the new fields flow through. Check this by eye rather than adding a renderer.
- `AppStatus::Uninstalling` is the internal name for `Deregistering`.

## BSL declaration

- [ ] Add `canopy_type: Option<String>` to `AppDef` in `crates/core/src/defs/app.rs`.
- [ ] Register `app.canopy_type(type)` on the `App` custom type beside `description`. It throws a script error when `is_in_action_closure()`, and when the value is not lowercase letters and digits in hyphen-separated segments starting with a letter. A later call replaces an earlier one. Annotate `l[impl app.canopy-type]`.
- [ ] Tests in the defs test module: a valid type is recorded, the last call wins, an untyped app has `None`, a call inside an action closure is a script error, and each malformed shape throws (uppercase, leading digit, leading or trailing hyphen, empty segment, empty string). `l[verify app.canopy-type]`.
- [ ] Run `etc/ci/check-defs-coercion.sh` to make sure the argument goes through `defs::take` or an equivalent throw.

## Storage

- [ ] New migration (next `version` after 58) in `crates/core/src/runtime/db.rs`:
  - `canopy_apps(app TEXT PRIMARY KEY, type TEXT NOT NULL, since INTEGER NOT NULL, last_grade TEXT)`.
  - `ALTER TABLE canopy_settings ADD COLUMN last_offer_live_at INTEGER` and `ADD COLUMN enabled_at INTEGER`.
- [ ] In `crates/core/src/runtime/canopy/store.rs`:
  - `sync_app_type(db, app, Option<&str>)`: upsert with `ON CONFLICT DO UPDATE`. Stamp `since` only when the type changes or the row is new. Delete the row on `None`.
  - `delete_app(db, app)`.
  - `set_last_grade(db, app, grade)`.
  - `list_apps(db)`.
  - `mark_offer_live(db, at)`.
  - `set_enabled` stamps `enabled_at` only on a false→true transition.
- [ ] Store tests: `since` survives re-applying the same type, a type change restamps it, `None` deletes the row, and `enabled_at` is untouched when enabling an already-enabled instance.

## Keeping the type record current

- [ ] Call `sync_app_type` in `/apps/create` and `/apps/update` (`crates/core/src/oi/handler/apps.rs`), beside the `sync_unsupported` call, so it runs on every applied definition.
- [ ] Call it on startup reload for every app whose definition evaluated. Skip apps whose reload filed `script_error`.
- [ ] Call `delete_app` where deregistration finishes and faults are cleared (`deregister_app`, around `clear_all_faults_for_app`), so a `Deregistering` app is still reported until it's gone.

## Report payload

- [ ] Restructure `build_payload` in `crates/core/src/runtime/canopy.rs`. Move the four checks into `machine.health` and the `canopy.report.extra` fields into `machine.detail`, and add `applications`. Update `r[impl canopy.report.checks]` and `r[impl canopy.report.extra]` placements.
- [ ] Add a `Skipped` variant to `Result_`.
- [ ] Build one entry per registered app with `canopy_type`, keyed by app name, with `type` and a single `health/app` check (`r[impl canopy.app.key]`). Grade it as follows (`r[impl canopy.app.health]`):
  - `Running` → passed, `Degraded` → warning, `Faulted` → failed;
  - `NotInstalled`, `Installing`, `Uninstalling` → skipped;
  - `Operating` → the stored `last_grade`, or skipped when there is none.
- [ ] Persist `last_grade` whenever the app is graded outside `Operating`.
- [ ] Put the app's status and active faults (kind, resource where there is one, description) in the `health/app` check's fields, from `faults::list_active_faults` filtered to the app.
- [ ] Rewrite the payload tests (`the_report_carries_the_fixed_check_set`, `the_report_names_seedling_as_its_source_and_describes_only_seedling`, and the `find` helper) for the `machine` section.
- [ ] Add tests:
  - two apps of one type are two entries keyed by name;
  - an untyped app is absent;
  - each status grades per the table;
  - an operating app re-sends its earlier grade, including after the `canopy_apps` row is re-read from a fresh `Db` handle (restart severed);
  - fault detail lists kind, resource and description.

## Missing-offer fault

- [ ] Record offer liveness:
  - `mark_offer_live(now)` on `/canopy/offer`;
  - on every report-loop tick while `state.canopy.current().is_some()`, whether or not the report succeeds;
  - wherever an offer ends (withdraw, `remove_by_conn` in `oi/server.rs`, `revoke_all` on disable).
  
  Ticking every minute keeps the value accurate to within a reporting interval across a crash.
- [ ] Give `OiState` a wall-clock `started_at: Timestamp` beside `start_time`.
- [ ] Sweep on each report-loop tick, before the skip check (`r[impl canopy.offer.absent]`):
  - Proceed only while Canopy access is enabled and no offer is live.
  - For each `canopy_apps` row, the deadline is `max(last_offer_live_at, started_at, enabled_at, since) + 15 min`.
  - Past the deadline, file `canopy_offer_absent` app-wide via `faults::file_once`. The description says when an offer was last live, or that none ever has been, and the same message is logged at error.
- [ ] Clear with `clear_faults_by_kind` across all apps:
  - on `/canopy/offer`;
  - on `/canopy/settings/set { enabled: false }`;
  - when the sweep runs with an offer live or access disabled.
  
  Clear the one app's fault when `sync_app_type` removes its row. Deregistration is already covered by `clear_all_faults_for_app`.
- [ ] Tests:
  - fifteen minutes from each of the four starting moments, each with the others earlier;
  - a short absence files nothing;
  - a held fault survives a restart (fresh state over the same `Db`) and clears on the next offer;
  - disabling clears it and files nothing while disabled;
  - re-enabling restarts the period;
  - an app whose definition drops its type loses the fault;
  - an untyped app is never faulted.
- [ ] Update the module doc comment in `runtime/canopy.rs`, which says a host without an offer has "no fault to explain".

## Operator interface

- [ ] `/apps/show`: add `canopy_type` next to `definition` (`apps.rs` around the `"definition"` key). `i[impl app.canopy.describe]`, with a handler test for both the typed and untyped case.
- [ ] `/canopy/status` (`crates/core/src/oi/handler/canopy.rs::status`): add `applications`, an array of `{ app, type }` sorted by app name, taken from the registry's evaluated definitions. `i[impl canopy.status]` and a handler test.

## Web

- [ ] `crates/web/frontend/src/lib/types.ts`: add `canopy_type: string | null` on the app detail type and `applications: { app: string; type: string }[]` on the Canopy status type.
- [ ] `DefinitionSection.tsx`: add a "Canopy type" row after "Seedling" when set, in monospace as in the mockup (`w[impl routes.apps.canopy-type]`). Test both cases in `DefinitionSection.test.tsx`.
- [ ] `routes/Canopy.tsx`: list the reported applications, linking each app name to its detail page and showing its type (`w[impl routes.canopy]`). Test the empty and populated cases in `Canopy.test.tsx`.
- [ ] Update `AppDetail.fixtures.ts` for the new field.

## Finish

- [ ] `cargo clippy`, `cargo fmt`, frontend lint and tests, `tracey query status` with no uncovered H5 requirements, and the `etc/ci/` greps.
