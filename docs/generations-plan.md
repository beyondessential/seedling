# Generations, on_change, warm_certs implementation plan

This plans the implementation of the spec changes in commit
`spec: redesign on_change, add generations and cert warming, drop rt.reconcile`.

## Goals (recap)

- Replace UUID `version_id` with monotonic per-app `generation` that bumps on
  register, script update, and param set/unset.
- Materialise a real `old` App (reconstructed from the previous generation) for
  `on_change` handlers, replacing the `App::default()` stub.
- Add `rt.warm_certs(selection)` that pre-provisions TLS certificates without
  routing traffic.
- Add `/apps/generations` history endpoint and `/apps/plan` dry-run RPC.
- Add `ParamSet`, `ParamUnset` events; carry source/target generation on
  operation events and on the `current_operation` field of `app.describe`.
- Drop `rt.reconcile` from the BSL surface and the runtime.
- Delete generation history on deregister; re-register starts a fresh lineage.

## Out of scope

- `on_reload` (script-change handler equivalent of `on_change`).
- Cross-generation diff with a non-current "active generation" (multi-param
  ambiguity — explicitly rejected during design).
- Date-parameterised cert validity queries.
- Migrating pre-existing `app_versions` rows into per-generation history. Any
  history that predates this change collapses into a single `Register`
  generation per app at migration time. (Audit log retains the rest.)

## Schema changes

All in `src/runtime/db.rs` migrations. Add a new migration version.

### New tables

```sql
CREATE TABLE script_bodies (
    hash TEXT PRIMARY KEY,        -- content hash (sha256 hex)
    body TEXT NOT NULL
);

CREATE TABLE generations (
    app TEXT NOT NULL,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    kind TEXT NOT NULL,           -- 'register' | 'script_update' | 'param_set' | 'param_unset'
    param_name TEXT,              -- non-null for param_set/param_unset
    previous_value TEXT,          -- nullable; null encodes None
    new_value TEXT,               -- nullable; null encodes None
    script_hash TEXT NOT NULL,    -- always set; references script_bodies.hash
    operation_id TEXT,            -- nullable; references autonomous_operations.id (string-ified) when this change scheduled an op
    outcome TEXT,                 -- nullable: 'pending' | 'succeeded' | 'failed'
    outcome_error TEXT,           -- nullable; details when outcome = 'failed'
    PRIMARY KEY (app, generation),
    FOREIGN KEY (app) REFERENCES registered_apps(name) ON DELETE CASCADE
);

CREATE INDEX idx_generations_app ON generations(app, generation DESC);
```

### Modified tables

- `registered_apps`: drop `current_version_id` (UUID), add
  `current_generation INTEGER NOT NULL`. The current script body is
  retrievable via `generations.script_hash` for `(app, current_generation)`.
- `action_log`: add `source_generation INTEGER` and `target_generation INTEGER`
  columns (both nullable for back-compat with pre-migration rows; required for
  new rows). Pass through `DbActionLog::commit()` → `insert_action_log_entry()`.
- `autonomous_operations`: add `source_generation INTEGER` and
  `target_generation INTEGER` (same shape as above).

### Dropped tables

- `app_versions`: replaced by `generations` + `script_bodies`. Migration
  collapses pre-existing per-app state into a single `register` row.

### Migration of existing data

1. Hash every distinct script body across `registered_apps.script` and existing
   `app_versions.script`; insert into `script_bodies` (deduped).
2. For each row in `registered_apps`, write a `generations` row at
   `generation = 1`, kind `register`, with the current script's hash, no param
   transition, no operation, no outcome.
3. Set `registered_apps.current_generation = 1`.
4. Drop `current_version_id` and the `app_versions` table.

`params` table: unchanged in shape. Param history lives only in `generations`
going forward; pre-migration param values are present in `params` but their
arrival times are lost.

## Phased implementation

Each phase is a self-contained vertical slice that can be committed and
reviewed independently. Phases do not assume the next is in flight.

### Phase 1 — generation foundation

- Migration: create new tables, add columns, run data migration, drop old.
- `runtime::apps`: introduce `Generation = u64`; replace `version_id: String`
  references through the registry types.
- `runtime::generations` (new module): `bump_register`, `bump_script_update`,
  `bump_param_set`, `bump_param_unset`, `record_outcome`, `current`,
  `reconstruct_app_def(app, generation)`. Reconstruction loads the script body
  by hash and walks `generations` to assemble the param map at generation N.
- OI handlers (`/apps/create`, `/apps/update`, `/apps/params/set`,
  `/apps/params/unset`): bump generation atomically with the change. Fix the
  pre-existing gap where `/apps/update` did not bump version (now
  `bump_script_update`).
- `script_error` faults remain orthogonal: a failed script eval does not bump
  the generation (the change is rejected).

Tests: round-trip migration, sequence of generation bumps, AppDef
reconstruction at arbitrary historical generations, deregister wipe.

### Phase 2 — operation source/target generation

- Add `source_generation` and `target_generation` to the in-memory
  `OperationRecord`, persist them on enqueue, propagate through the action log
  rows.
- Scheduler: when an op is enqueued in response to a param change or script
  update, source = pre-bump generation, target = post-bump generation. For
  operator-invoked actions (`start`, `install`, named actions), source =
  target = current generation at dispatch.
- `barrier::replay`: read the op's `target_generation` to load the script body
  for closure recovery (instead of always using the current script).

Tests: replay across an intervening generation bump (op should not see
the newer state); operator action source == target.

### Phase 3 — `old` materialisation in on_change replay

- Replace `let old_app = App::default()` at `barrier/replay.rs:295` with
  `generations::reconstruct_app_def(app, source_generation - 1)`.
- Edge case: `source_generation == 1` means there is no prior. Per the spec,
  `on_change` is not fired on install, so a `source = 1` for a `param_change`
  op should not happen in practice. Guard with a debug assertion and treat as
  empty App if it ever does.

Tests: an `on_change` handler that reads `old.param("foo").value()` sees the
prior value; `old.deployment("frontend")` sees the prior shape; chained
changes (param → param → param) each see the immediately prior generation
as `old`.

### Phase 4 — `on_change` transition semantics

- Schedule `on_change` only when the transition matches one of the three
  defined transitions (`None → Some`, `Some(s₁) → Some(s₂)` with `s₁ ≠ s₂`,
  `Some → None`). Setting a param to its current value should still bump the
  generation? *Open question — see below.* Working assumption: bumping happens
  only when the value actually changes (no-op set returns
  `schedule = "not_scheduled"` and does not bump).
- OI: return `{ "schedule": ..., "generation": <int> }` per spec.

Tests: each transition triggers `on_change`; same-value set does not.

### Phase 5 — drop `rt.reconcile`

- Remove `rt.reconcile` from `runtime::barrier::runtime` (the stubbed
  `with_fn("reconcile", ...)`).
- Remove `CallKind::Reconcile` from `runtime::barrier::ActionLogEntry` and its
  handling in `OperationProgress::from_log`.
- Update tracey annotations (`l[impl rt.reconcile]`, `r[impl reconcile.*]`).
- Audit existing scripts in tests for `rt.reconcile` usage; rewrite as
  `stop` + `start` where they appear.

Tests: existing on_change tests still pass; no surprises in the test corpus.

### Phase 6 — `rt.warm_certs`

- Rhai API: `rt.warm_certs(collection) -> Started`. Selects TLS-terminating
  ingresses from the collection. Returns a `Started` whose `.ready()` barrier
  is satisfied when all selected ingresses' certs are observed `valid`.
- Cert observation: extend the proxy reconciler to query Caddy's certificate
  state per hostname, persisting facts (`cert_status`, `cert_expiry`) to the
  observation history. New observation kinds drive both the standard ingress
  `Ready` lifecycle and the warm_certs barrier.
- Caddy config: refactor `system::translate::proxy::build_proxy_config` to
  support a "warm-only" entry per ingress — a TLS-automation declaration
  without an associated route. Push this when warm_certs targets an ingress
  that hasn't been `rt.start`-ed.
- Action log: record `warm_certs` calls so replay is idempotent. Re-issuing
  warm_certs against an already-`valid` cert returns immediately.
- Faults: file `cert_acquisition_failed` on persistent failure; clear on
  subsequent `valid` observation.

Tests: barrier blocks until cert observed valid; replay across restart
finds the cert already valid and resumes; persistent ACME failure files
fault and barrier eventually deadlines.

Risk: refactoring the proxy config builder to carry "cert-only" entries
alongside route-bearing entries. Worth a focused review.

### Phase 7 — interface surface

- `app.describe`: replace `version_id` with `generation`; add
  `source_generation` / `target_generation` to `current_operation`.
- `/apps/script`: rename `version` param to `generation`, return
  `{ script, generation }`.
- `/apps/generations` (new): paginated history per
  `i[generation.history]`.
- Events: rename `version_id` → `generation` on `AppRegistered`, `AppUpdated`;
  add `previous_generation`. Add `ParamSet` / `ParamUnset` events. Add
  `source_generation` / `target_generation` on `OperationStarted`,
  `OperationCompleted`, `OperationFailed`.

Tests: serialisation round-trip; rename does not leave dangling
`version_id` strings in the OI response shapes.

### Phase 8 — `/apps/plan` dry-run RPC

- Accepts `proposed_script` and/or `proposed_params`, evaluates a hypothetical
  AppDef without persisting anything.
- Diffs against current AppDef: walk both resource maps, classify each
  resource as `added`, `removed`, or `modified` (with field list for
  `modified`).
- `on_change_would_fire`: any param whose effective value (or whose Option
  state) would change.
- Errors from script eval are returned in the response, not raised as
  faults (no persistence side effects).

Tests: param-only proposal; script-only proposal; combined; failing script
returns errors.

### Phase 9 — CLI surface and tracey

- Update `src/ctl/apps.rs` to use `generation` everywhere `version_id` was
  previously surfaced.
- Add CLI subcommands for `/apps/generations` (history view) and `/apps/plan`
  (dry-run).
- Tracey: add `l[impl ...]` and `r[impl ...]` annotations for every new spec
  rule; remove annotations for deleted spec rules; run `tracey query
  uncovered`.

## Open questions to resolve before / during implementation

1. **Same-value set:** does `set_param("x", "v")` when the current value is
   already `"v"` bump the generation? Working assumption: no. Confirm.
2. **Script update during in-flight op:** spec says new AppDef takes effect
   "at the next evaluation boundary after the operation completes." Does the
   generation bump immediately (at update time) or only at the boundary?
   Working assumption: immediately, but the in-flight op continues at its
   target_generation. This means the current generation can be ahead of the
   reconciler's effective AppDef during the window. Probably fine — confirm.
3. **Param set during in-flight op:** spec defers to the lifecycle queue
   (`SameAppOperationInProgress` rejection or queued). Same question for
   generation bump timing. Working assumption: bump only on accept (queued
   counts as accepted).
4. **Cert warming for ingresses with multiple hostnames:** does each hostname
   need to reach `valid` independently before the barrier resolves, or is
   any-one enough? Spec says "every selected ingress" but doesn't pin
   per-hostname. Working assumption: all hostnames of all selected ingresses.

## Suggested commit granularity

One commit per phase as a default. Phase 1 may need to split the migration
from the higher-level `runtime::generations` API. Phase 6 should split cert
observation, Caddy refactor, and the Rhai-side `rt.warm_certs` into separate
commits because each is independently reviewable.

## Tests / tracey checklist

Per the AGENTS rules, every new feature gets tests. Highlights:

- Generation bump on each trigger; deregister wipes history.
- AppDef reconstruction at arbitrary historical generations.
- `on_change`: each transition fires; not fired on install; `old.*` reads
  consistent with prior state.
- Replay across runtime restart: source/target generation persisted, `old`
  reconstructed correctly.
- `rt.warm_certs`: barrier waits for `valid`; replay idempotent; fault on
  persistent failure.
- `/apps/plan`: each combination of proposed_script / proposed_params;
  failing script case.

`tracey query uncovered --spec-impl runtime/main` and equivalents must come
out clean by the end of phase 9.
