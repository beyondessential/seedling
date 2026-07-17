# Theme 4: Fault lifecycle asymmetries

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 4.

## The failure pattern

The fault store (`crates/core/src/runtime/faults.rs`) is deliberately minimal: `file_fault` is a bare
INSERT, `clear_fault` flips `cleared_at`, and the only bulk primitives are `clear_faults_by_kind`,
`clear_all_faults_for_app`, and `clear_faults_for_instance`. Everything else — dedup, subject identity,
and the decision of *when* a fault stops being true — is re-implemented at every call site, and the
sites have diverged into at least five idioms:

- **Ad-hoc dedup scans.** Most reconciler sites scan `list_active_faults` for an `already_filed`
  match before filing, but each picks its own identity: `(kind, instance_id)` in
  `file_instance_faults`, `(kind, resource_name)` for `service_degraded` and
  `health_check_replace_failed`, `(kind, description)` for `image_pull_failed` and
  `disallowed_registry`, bare `kind` in `file_system_fault`. Sites outside the reconciler
  (`runtime/audit.rs`) skip the scan entirely, so `audit_lag` duplicates without bound.
- **In-memory prev-set diffs on clear.** `reconcile_ingress_conflicts` and
  `reconcile_site_service_faults` (`system/reconcile/faults.rs`) clear only `prior \ current`, where
  `prior` is a `Reconciler` field (`prev_ingress_conflicts`, `prev_site_service_faults`) that starts
  empty on every daemon start — but the faults are in the DB, so a fault filed before a restart can
  never clear.
- **Full sweeps against the current set.** `reconcile_unresolved_site_attachments` and
  `sync_registry_faults` (`runtime/apps/registry_faults.rs`) do it right — every tick/evaluation they
  compare *all* active faults of the kind against the currently-true set — though the former encodes
  its key as a `[key]` substring of the description.
- **Over-broad clears.** `run_volume_backup` (`oi/handler/backups.rs:577`) calls
  `clear_faults_by_kind(db, backup_app, "backup_failed")` on any volume's success, wiping every other
  volume's fault; `file_pod_actuation_faults` clears `stop_failed` for any instance that emitted
  `stop_sent` this tick — including the instance whose stop just failed, because `pods.rs` pushes
  `stop_sent` before attempting the stop.
- **Missing halves.** `audit_lag` has no clear path at all (GC prunes only *cleared* faults);
  `tailscale_unreachable` has a clear condition implied by its doc comment
  (`runtime/tailscale.rs:37`) but no filing code; `add_registry`
  (`oi/handler/registries.rs`) never re-runs the sweep that `remove_registry` runs, so the natural
  remediation leaves `disallowed_registry` stale.

The common root cause: **there is no shared notion of a fault's identity or of its clear condition**.
Subject identity is smeared across `resource_name`, `instance_id`, and description-substring matching
(image refs, `host:port` tuples, `[key]` prefixes), so clears are either too broad (app + kind) or
fragile (string containment), and every site invents its own file/clear pairing — or forgets one half.

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| Later volume success erases earlier volume's backup failure faults (H8) | [§6](../logic-bug-audit-2026-07.md#6-backups-volumes-scheduling) | high |
| Adding a registry to the allowlist leaves stale `disallowed_registry` faults | [§5](../logic-bug-audit-2026-07.md#5-oi-handlers-tls-services-ingresses-images-registries-key_mgmt) | medium |
| `stop_failed` faults from scale-down stops are filed and immediately cleared in the same tick | [§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine) | medium |
| `ingress_conflict` and site-service endpoint faults never clear after a daemon restart | [§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine) | medium |
| Audit-lag faults are filed without dedup and are never cleared | [§7](../logic-bug-audit-2026-07.md#7-runtime-persistence-db-generations-history-audit-faults-gc) | low |
| Promised `tailscale_unreachable` fault is never filed | [§11](../logic-bug-audit-2026-07.md#11-runtime-site-networking-site-services-ingresses-attachments-external-mappings-tailscale) | low |

Related: the unclearable `proxy_failed`/`proxy_apply_failed` fault when the last ingress is removed
([§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine), "An empty proxy config is never
applied") is the same disease — the clear is tied to a success event that stops firing rather than to
the condition no longer holding.

## Would a high-level change help?

**Yes — this is the theme where a shared mechanism pays off most directly**, because five of the six
findings are not wrong *judgements* but wrong *bookkeeping*, and the bookkeeping is copy-pasted per
site. But one converge-everything helper is not enough; the fault kinds split into three shapes that
need different disciplines:

1. **Condition faults (reconciler-shaped).** "This is true right now": `ingress_conflict`,
   `site_service_endpoint_unresolvable`/`unroutable`, `site_ingress_target_missing`,
   `service_degraded`, `disallowed_registry`, `tailscale_unreachable`, `backup_failed` (true until the
   *same volume* backs up successfully). Each tick or triggering event can compute the full set of
   currently-true keys, and the store should **converge** to that set: file what is missing, clear
   what is no longer present. Dedup is automatic, clears are sweeps against the DB rather than an
   in-memory diff, and the mechanism is restart-safe by construction — exactly what
   `reconcile_unresolved_site_attachments` and `sync_registry_faults` already do by hand, and what the
   `prev_*`-diff sites fail to do.
2. **Event faults.** "This happened": `audit_lag`, and per-instance actuation results
   (`start_failed`, `stop_failed`, `observe_failed`) where each tick yields an outcome per instance
   rather than a global set. Converge-to-current does not fit `audit_lag` — there is no "currently
   lagging" set to compute. The discipline here is **dedup on file** (`file_once` on the key) plus a
   **documented, keyed clear**: the paired success event with the same subject clears it, and a kind
   with no success event (like `audit_lag`) must name its clear path explicitly (operator
   `fault.clear-app`, or recovery-of-the-writer). Crucially, the file set and clear set for one tick
   must be **disjoint per key** — computed from the instance's single outcome — which mechanically
   fixes the `stop_sent`/`stop_failed` same-tick cancellation.
3. **Latched (hard) faults.** `crash_loop` and `health_check_replace_failed` deliberately outlive
   their trigger — the comment in `file_replace_failed_fault` spells out that instance teardown must
   *not* clear it. Converging these to "currently observed" would silently un-latch them. They keep
   `file_once` dedup but clear only on explicit lifecycle events (generation bump, unit healthy again,
   operator clear).

Classifying every kind into one of these three shapes, and routing it through one of two helpers, fixes
all six findings and removes the idiom divergence that produced them.

## Proposed pattern

**Key shape.** Give faults a first-class identity:

```rust
pub struct FaultKey {
    pub app: AppName,          // "_system" / "seedling" sentinels included
    pub kind: String,          // "backup_failed", "ingress_conflict", ...
    pub subject: String,       // the *thing* that is faulty, never the description
}
```

`subject` is a new column on `faults` (added as a new `version < N` migration block at the bottom of
`crates/core/src/runtime/db.rs` — existing blocks are never edited). It absorbs what today hides in
three places: instance hex (`start_failed`), image ref (`image_pull_failed`), volume id
(`backup_failed` — currently *absent*, which is finding H8), `host:port` (`ingress_conflict`),
`site_ingress:port:protocol` (`site_ingress_target_missing`), deployment name
(`health_check_replace_failed`). `resource_type`/`resource_name`/`instance_id` remain as display
metadata; matching and clearing use `(app, kind, subject)` only. Description-substring matching
(`f.description.contains(&host_port)`) disappears.

**Two helpers in `runtime/faults.rs`:**

```rust
/// File iff no active fault has this key. Returns true if newly filed.
pub fn file_once(db: &Db, key: &FaultKey, meta: FaultMeta, description: &str)
    -> rusqlite::Result<bool>;

/// Converge active faults within `scope` to `current`: file missing keys,
/// clear active keys not in `current`. Restart-safe: reads the DB, not
/// in-memory prior sets.
pub fn sync_faults(db: &Db, scope: FaultScope, current: &BTreeSet<FaultKey>)
    -> rusqlite::Result<SyncOutcome>;
```

`FaultScope` selects which active faults the sweep owns — `Kind("ingress_conflict")` across all apps,
or `AppKind(app, "backup_failed")` — so a converge call can never clear kinds it does not manage.
Descriptions for newly-filed keys come from a closure or a `key -> description` map alongside
`current`.

**Site mapping.**

- `sync_faults`: `reconcile_ingress_conflicts` and `reconcile_site_service_faults` (delete
  `prev_ingress_conflicts`/`prev_site_service_faults`), `reconcile_unresolved_site_attachments`
  (already sweep-shaped; moves off description keys), `file_service_degraded_faults`,
  `sync_registry_faults` plus the inline duplicate of it in `re_evaluate_all_apps`
  (`oi/handler/registries.rs:141-178`) — and `add_registry` gains the same re-evaluation call
  `remove_registry` already has, or better, the sweep moves onto a reconcile tick so both converge
  without handler-side special cases. `run_volume_backup` converges
  `AppKind(app, "backup_failed")` with subject = volume id: success for `ok/data` files/clears only
  its own key. The Tailscale provider converges `Kind("tailscale_unreachable")` to `{key}` when the
  consecutive-failure counter crosses `FAULT_AFTER_FAILURES` and to `{}` on a successful poll —
  finally filing the promised fault.
- `file_once` + keyed clear: `file_pod_actuation_faults` and `file_volume_actuation_faults`
  (subject = instance hex; per-instance outcomes computed first so file/clear sets are disjoint —
  and `pods.rs` stops emitting `stop_sent` before the stop is attempted), `file_image_pull_faults`
  (subject = image ref), `audit_lag` in `runtime/audit.rs` (subject = `""` or the log path; clear
  path documented as operator clear plus optional clear-on-recovery).
- `file_once` + lifecycle clear only: `crash_loop`, `health_check_replace_failed` (unchanged
  semantics, shared dedup).

**Restart behaviour.** `sync_faults` compares this tick's computed truth against the persisted active
set, so a fault filed before a crash clears on the first tick after the condition stops holding —
no warm-up state, no `Reconciler` fields to forget to persist.

**Logging convention.** `file_fault` already emits `warn!` internally, so "wherever we file a fault we
also log" holds mechanically; keep that in the new helpers (and emit on clear too). The inverse —
"wherever we log an error we consider a fault" — cannot be mechanised and stays a review item; the
Tailscale `warn!`-without-fault gap is the canonical miss.

## What it prevents — and what it does not

Prevented by construction: duplicate active faults for one key; clears that outlive a restart
(prev-set diffs are gone); over-broad clears (a clear can only target a key the site computed, inside
its declared scope); same-tick file/clear churn; file-without-clear for condition faults (the sweep
*is* the clear). It also deletes four copies of the `already_filed` scan and two hand-rolled sweeps.

Not prevented: computing the wrong condition in the first place (the healthcheck-replace target bug in
§12 would survive any fault plumbing); trigger paths that are dead code (H4's unreachable
`crash_loop` detection); forgetting to call `sync_faults` at all from a new subsystem; and semantic
questions like whether an app leaving a still-conflicted `(host, port)` should clear *its* fault while
the conflict persists for others — keying by party makes that expressible, but someone still has to
decide it. Latched faults remain a judgement call per kind; the pattern only forces the call to be
written down.

## Migration path

1. Add the `subject` column (new migration block; existing active faults backfill from
   `instance_id`/`resource_name`, else `""`). Add `FaultKey`, `file_once`, `sync_faults` with unit
   tests against the in-memory `Db` (the harness in `runtime/faults/tests.rs` and the
   `ensure_faults_init` pattern in `gc.rs` tests already exist).
2. Port the two prev-set sites — this alone fixes the restart finding and deletes `Reconciler` state.
3. Key `backup_failed`/`backup_source_unavailable` by volume (fixes H8) and replace both
   `clear_faults_by_kind` calls in `backups.rs`; spec text `r[backup.execution]` is updated first to
   say per-volume clearing (its current "for the backup app" wording is what the bug implements).
4. Port the actuation sites with disjoint per-instance outcome sets (fixes the `stop_sent` churn);
   port `image_pull_failed` off description matching.
5. Route `audit_lag` through `file_once`; implement `tailscale_unreachable` via `sync_faults`;
   converge `disallowed_registry` on `registries/add` (and collapse the `registries.rs` inline copy
   back into `sync_registry_faults`).
6. Deprecate `clear_faults_by_kind` for anything except operator-initiated clears; leave
   `clear_all_faults_for_app` (deregistration) and `clear_faults_for_instance` (teardown) as the
   lifecycle escape hatches they are.

Steps are independent and each shrinks a finding; per the repo's jj discipline, each lands as its own
commit rather than one squashed rewrite.

## Enforcement

- **Per-site unit tests on the in-memory `Db`.** Each finding gets the test the audit already sketched:
  two-volume strategy where the first fails and the second succeeds, assert the fault survives;
  pre-seed an active `ingress_conflict`, run the sweep on a fresh `Reconciler` with an empty report,
  assert it clears; feed a `PodActuationUpdate` with both a `stop_failure` and its `stop_sent`,
  assert an active fault survives; call the audit-lag helper twice, assert one active fault; TestOi:
  file `disallowed_registry`, call `/registries/add`, assert cleared. Plus helper-level tests:
  `sync_faults` is idempotent, converges from any pre-seeded DB state, and never touches kinds
  outside its scope.
- **A tracey spec requirement for the lifecycle itself.** The existing `r[fault.*]` items in
  `docs/spec/runtime.md` define individual kinds; add a `r[fault.lifecycle]` family stating the
  *what*, not the *how*: every fault kind defines both its filing condition and its clearing
  condition; at most one active fault exists per (app, kind, subject); a condition fault is active
  exactly while its condition holds, including across daemon restarts; a latched fault documents the
  lifecycle event that clears it. Implementations annotate `file_once`/`sync_faults` and each ported
  site with `r[impl fault.lifecycle...]`, and the tests above carry `r[verify ...]`, so
  `tracey query status` shows which kinds have not adopted the discipline.
- **Review checklist.** (a) Every new `warn!`/`error!` on an operator-relevant failure states either
  which fault it pairs with or why none applies — and every new fault kind names its clear condition
  and shape (condition / event / latched) in a comment at the filing site. (b) Any direct
  `faults::file_fault` call outside the two helpers is a review flag. (c) Any clear keyed more
  broadly than the file (kind-wide clear for a subject-keyed fault) is a review flag — that exact
  mismatch is H8.
