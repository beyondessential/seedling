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

- A no-op set/unset (value already matches; unset of an already-unset param)
  short-circuits at the OI handler before any persistence: no generation
  bump, no `on_change`, response is `{ "schedule": "not_scheduled",
  "generation": <current> }`.
- For real transitions (`None → Some`, `Some(s₁) → Some(s₂)` with `s₁ ≠ s₂`,
  `Some → None`), bump the generation, then schedule `on_change` if a handler
  is registered and the app is installed.
- Reject `set_param` / `unset_param` with `operation_in_progress` when an op
  is in flight or queued. Same for `/apps/update`.
- OI: return `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }`
  per spec.

Tests: each transition triggers `on_change`; same-value set is no-op; updates
during in-flight ops are rejected.

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

**Spike validation (done):** Tested against an isolated Caddy 2 container.
Confirmed: `tls.automation.policies` alone does not trigger acquisition;
`tls.certificates.automate` + matching policy does; cached cert is reused
when http server routes are added later (mtime unchanged); Caddy's data
volume persists certs at `<data>/caddy/certificates/<issuer>/<host>/<host>.crt`.

**Risk reassessment after reading code:** The `build_proxy_config` refactor
risk was overstated. `VirtualHost` already separates `tls_acme` from `routes`,
and the warm-cert path is fully additive — it only emits new `tls.certificates.automate`
entries and adds subjects to the existing automation policy. No structural
change to the existing builder.

#### Step 1 — Rhai surface (done, commit `d77bb43d`)

- `CallKind::WarmCerts` variant.
- `RuntimeInstance.do_warm_certs` filters to TLS-terminating Ingresses,
  records the call in the action log, returns a `Started`.
- `OperationProgress.warm_cert_hostnames: BTreeSet<String>` populated by
  `from_log` for `WarmCerts` entries (separate from the standard desired
  state — reconciler does not start routing the ingress).

#### Step 3 — Caddy config emission (done, commit `d77bb43d`)

- `ProxyConfig.warm_cert_hostnames` field; `augment_with_warm_certs` adds
  hostnames not already covered by routed vhosts.
- `compute_proxy_config` resolves ingress resource names to hostnames via
  `AppDef`, filters to `tls=true` ingresses, calls augment.
- `build_caddy_config` emits both policy subjects and
  `tls.certificates.automate` entries when warm hostnames are present.
- Tests pin the new behaviour and the no-double-listing case.

#### Step 2 — Cert observation (todo)

- Read Caddy data volume mount path via `podman volume inspect seedling-caddy-data`
  at startup, cache on `Reconciler`.
- Each tick, check `<data>/caddy/certificates/<issuer>/<host>/<host>.crt` for
  every hostname in any app's `warm_cert_hostnames`. If present, parse the
  cert (e.g. `x509-parser`) to extract expiry. Persist `cert_valid` observation
  against the corresponding ingress instance.
- Add new obs kinds `cert_valid` and `cert_acquisition_failed`. Do NOT map
  `cert_valid` to `LifecycleState::Ready` for ingress (would conflate with
  routing readiness — see lifecycle.ingress: "accepting traffic AND certs valid").

#### Step 4 — Barrier resolution + fault (todo)

- Add `is_warm: bool` flag to `Started`. `do_warm_certs` sets it true.
- `WorldStateOracle::cert_valid_for(instance) -> bool`. `DbWorldOracle` checks
  for a `cert_valid` observation; `TestWorldOracle` defaults to false (set
  via helper).
- `check_barrier`: when `is_warm` and required is `Ready`, use `cert_valid_for`
  instead of `lifecycle_state(...).has_reached(Ready)`. Standard barrier
  semantics for non-warm Starteds unchanged.
- Add `r[impl fault.cert-acquisition]`: file `cert_acquisition_failed` when
  cert observation hasn't appeared after a configurable threshold (e.g.
  3 minutes), include the most recent Caddy error log line if available.
  Clear on subsequent `cert_valid` observation.
- Tests: end-to-end with a stub Caddy data dir; barrier blocks until cert
  file appears; replay across restart finds existing cert and resumes;
  fault filed when cert never appears.

**Until step 2+4 land:** `rt.warm_certs(...).ready()` resolves immediately
when `Started.ctx` is `None` (test mode) and blocks indefinitely otherwise
(production mode), because the standard ingress `Ready` lifecycle never
fires for non-routed ingresses. Production callers should not yet rely on
the barrier — only the API surface and Caddy emission are usable.

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

## Resolved design points

1. **Same-value set is a no-op.** `set_param("x", "v")` when the current value
   is already `"v"` does not persist, does not bump the generation, does not
   schedule a handler. Same for `unset_param` against an already-unset param.
2. **Script and param updates during in-flight ops are RPC-rejected** with
   `operation_in_progress`, not queued or deferred. There is therefore no
   window where the current generation is ahead of the reconciler's effective
   AppDef. The previous spec text on `i[app.update]` ("takes effect at the
   next evaluation boundary") is replaced by the rejection rule.
3. **Cert warming barrier:** all hostnames of all selected ingresses must be
   `valid` before `.ready()` resolves.

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
