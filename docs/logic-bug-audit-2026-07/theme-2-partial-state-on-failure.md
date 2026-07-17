# Theme 2: Failure paths that keep partial state instead of the previous good state

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 2.

## The failure pattern

Each affected site performs a fallible step and then lets its *result* become observable regardless of whether the step succeeded. What varies is where the state lives, and that matters for the fix:

- **(a) Swap-in-partial-value.** `evaluate_script` (`crates/core/src/runtime/apps.rs`) deliberately returns `(App, Option<ScriptError>)` — an `App` is always produced, populated up to the point the script threw. That contract is right for `AppRegistry::register` (an app may legitimately be registered with unset params and no previous def exists), but `AppRegistry::reload` (`runtime/apps.rs:156-169`) applies the same contract where a previous *good* def exists: it unconditionally does `entry.app = app`, contradicting its own doc comment ("On failure the existing AppDef keeps running"). `update_app` (`crates/core/src/oi/handler/apps.rs:1561`) then diffs derived state against the partial def — volume hold (`apps.rs:1573-1677`), `clamp_scaling_decisions` (`apps.rs:1684-1712`), stale-forward teardown (`apps.rs:1754-1778`), `sync_action_schedules` (`apps.rs:1781`) — turning a typo into data relocation. `reload_and_persist_apperror` (`crates/core/src/oi/handler/params.rs:87`) has the same swap on the `set_param`/`unset_param` paths.

- **(b) Drop-from-set-then-apply-absolute-state.** `Reconciler::snapshot_all_apps` (`crates/core/src/system/reconcile.rs:483`) `continue`s past an app whose `compute()` errors (`reconcile.rs:555-562`). That would be harmless if downstream consumers were incremental, but routes, nftables rules, and the proxy config are *absolute*: they are rebuilt from the surviving snapshots and applied wholesale, so a skipped app's DNAT, service routes, and vhosts are actively removed while its containers run on. Worse, `tick()` treats an empty snapshot list as idleness (`reconcile.rs:675-678`) and calls `tear_down_idle` (`reconcile.rs:1156`), flushing all rules and removing Caddy, the resolver, and NAT64. The per-app `continue` in `phases::compute_routes` (`crates/core/src/system/reconcile/phases.rs:114-117`) and its siblings in `compute_nftables_rules`/`compute_proxy_config` have the same drop-then-apply shape.

- **(c) In-memory/durable divergence.** `register_app` (`crates/core/src/oi/handler/apps.rs:1208`) inserts into the in-memory `AppRegistry` first, then makes three separate `db.call`s (`persist_app_fields`, `bump_register`, `persist_app_fields` again). Any DB error returns to the client without removing the entry: `/apps/list` shows an app that a restart will silently drop (`load_from_db` skips generation-0 rows), and retrying `/apps/create` is rejected. `set_param`/`unset_param` show the mirror image: durable write committed, then an error returned when `schedule_on_change` is rejected.

The common invariant is **"on failure, observable state is unchanged (or equals the last good state)"**. The mechanisms cannot be common, because "observable state" is an in-memory `ArcSwap`'d def in (a), an externally-applied absolute network state in (b), and a registry/DB pair in (c).

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| Failed script evaluation in `/apps/update` still triggers destructive post-reload actions (volume hold, scaling wipe, forward teardown) — C1 | [§4](../logic-bug-audit-2026-07.md#4-oi-handlers-apps-actions-params-templates-status-faults) (root cause noted in [§10](../logic-bug-audit-2026-07.md#10-runtime-appdesired-state-image-management)) | critical |
| App skipped on desired-state/registry error has its whole data plane torn down (and all-apps failure triggers full idle teardown) | [§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine) | medium |
| `register_app` leaves the app registered in memory when DB persistence fails | [§4](../logic-bug-audit-2026-07.md#4-oi-handlers-apps-actions-params-templates-status-faults) | low |
| `set_param`/`unset_param` persist the change, then return an error and skip the event (adjacent instance of the same invariant) | [§4](../logic-bug-audit-2026-07.md#4-oi-handlers-apps-actions-params-templates-status-faults) | medium |

## Would a high-level change help?

**Verdict: partially — one invariant, but three boundary-specific disciplines. A single generic mechanism (e.g. a transaction framework) would fit (a) and (c) and not (b).**

- **(a) yes.** The bug is entirely a contract problem: `evaluate_script`'s partial-result return is correct for registration and wrong for reload, and nothing in the type system distinguishes the two. Making `reload` commit-on-success removes every downstream instance at once — `update_app`'s four destructive diffs and both param paths are all diffs of *new def vs registry def*, so if the registry never holds a partial def, the diffs are inherently safe. One fix site, three callers protected.
- **(b) yes, as a stated invariant rather than a shared type.** The reconciler is level-triggered and rebuilds absolute state each tick; the discipline needed is "an app may only leave the applied absolute state through an explicit lifecycle transition (`AppPhase`), never through an error". That is a rule about two predicates (what feeds the absolute builders, when `tear_down_idle` may run), not a rule a wrapper type can enforce — but it is one rule, and the codebase already applies it elsewhere: a Caddy bring-up failure *skips* the nftables/proxy apply for the tick (`reconcile.rs:796`, "skipping nftables and proxy this tick") instead of applying a reduced state. The fix extends that precedent to compute errors.
- **(c) yes.** Durable-first ordering (evaluate → persist in one transaction → only then make the entry observable) is a mechanical rewrite of `register_app`, and the same ordering rule ("commit durable state last, after every fallible step that can reject the request") fixes the `set_param` persist-then-error inversion.

## Proposed pattern

### (a) Commit-on-success reload — `crates/core/src/runtime/apps.rs`

Keep `evaluate_script`'s partial-result contract (registration needs it), but make `reload` the only place that decides whether a partial value may become observable, and have it refuse:

```rust
pub enum ReloadOutcome {
    Applied,
    /// Evaluation failed: the previous good def keeps running, the new
    /// script text and fault are recorded, no derived state may be diffed.
    KeptPrevious(ScriptError),
}

pub fn reload(&mut self, name: &AppName, script: String, params: &..., limits: &...) -> ReloadOutcome {
    let (app, raw_error) = evaluate_script(name, &script, params, limits);
    let Some(entry) = self.entries.get_mut(name.as_str()) else { return ReloadOutcome::Applied };
    entry.script = script; // durable truth: the generation has/will bump to this script
    match raw_error {
        None => { entry.app = app; entry.script_error = None; ReloadOutcome::Applied }
        Some(e) => {
            entry.script_error = Some((e.to_string(), Timestamp::now()));
            ReloadOutcome::KeptPrevious(e) // entry.app untouched
        }
    }
}
```

Note the deliberate divergence: `entry.script` follows the new generation (so `/apps/show` and a later param-set re-evaluate the *new* script), while `entry.app` stays last-good — exactly the spec's `i[app.update]` wording. Callers gate: `update_app` runs the volume-hold diff, `clamp_scaling_decisions`, forward teardown, and `sync_action_schedules` only on `Applied`; `reload_and_persist_apperror` needs no diff changes but inherits the safe registry state.

### (b) Known-set invariant in the reconciler — `crates/core/src/system/reconcile.rs`

Minimal form, matching the existing Caddy precedent: make error-skips visible to the tick.

```rust
fn snapshot_all_apps(&self) -> (Vec<AppSnapshot>, usize /* skipped on error */);

let (apps, skipped) = self.snapshot_all_apps();
if apps.is_empty() {
    if skipped == 0 { self.tear_down_idle().await; }   // genuine idleness only
    return false;
}
// later: if skipped > 0, skip applying routes/rules/proxy this tick
// (pod and volume phases are per-app incremental and stay safe to run).
```

Stronger form: a `last_good_snapshot: HashMap<AppName, AppSnapshot>` on `Reconciler`, updated on successful compute and *removed only on a phase transition out of Installed/Installing* — on compute error, push the cached snapshot instead of `continue`, so healthy apps still get fresh absolute state the same tick. The per-app registry-error `continue`s in `phases::compute_routes`/`compute_nftables_rules`/`compute_proxy_config` need the same treatment (carry forward that app's last contribution, or poison the corresponding apply for the tick). The minimal form is a few lines and removes the outage; the carry-forward refinement can follow.

### (c) Durable-first registration — `crates/core/src/oi/handler/apps.rs`

Evaluate the script without touching the registry, commit all rows in one transaction (the repo idiom is `db.conn.unchecked_transaction()`, as in `runtime/gc.rs:179`), and only then insert the `AppEntry`:

```rust
let (app, script_error) = crate::runtime::apps::evaluate_script(&params.app, script, &BTreeMap::new(), &state.script_limits);
let generation = state.db.call(move |db| {
    let tx = db.conn.unchecked_transaction()?;
    persist_app_fields(db, &name, 0, false, false, false)?;      // FK target for generations
    let g = crate::runtime::generations::bump_register(db, &name, &script)?;
    persist_app_fields(db, &name, g, false, false, false)?;
    tx.commit()?; Ok(g)
})?;
state.registry.write().insert_registered(params.app.clone(), script, app, script_error, generation, ...);
```

On any error nothing is observable and the retry contract holds. The same "commit durable last, observable last" ordering applied to `params.rs` means `schedule_on_change` runs *before* the param row is written, so a rejection leaves no committed change behind.

## What it prevents — and what it does not

Prevents: the C1 destruction class (any future def-diff added to `update_app` is automatically safe, because the registry can never hold a partial def); the reconciler's error-driven data-plane teardown and the all-apps idle teardown; phantom half-registered apps and the persist-then-error inversion in params.

Does not prevent: wrong-but-successfully-evaluated scripts (an operator who really deletes a volume from the script still triggers a hold — by design); staleness while errors persist (carry-forward serves last-good routes for an app whose registry keeps erroring — the `registry_fault` filed in `snapshot_all_apps` remains the operator signal, and this is the correct trade against teardown); crash windows between the DB commit and the in-memory insert in (c) (benign: restart rebuilds from DB via `load_from_db`); and theme 3 (observation failures) — a *failed observation* is a different conflation even though it also destroys good state.

## Migration path

1. **(a) first** — it is the critical finding. Change `AppRegistry::reload` to `ReloadOutcome`; the compiler then finds every caller (`update_app`, `reload_and_persist_apperror`) and forces the gating decision at each. Behavioural change is confined to the failure path, which today is the bug.
2. **(b) minimal form** — `(Vec<AppSnapshot>, usize)` return plus the two predicate guards (idle teardown, absolute-apply skip). No new state, no schema change. Carry-forward of last-good snapshots is a follow-up once the stub-System tests exist.
3. **(c)** — rewrite `register_app` durable-first; fold the two `persist_app_fields` calls and `bump_register` into one transaction. Then apply the same ordering to `params.rs` (`schedule_on_change` before persist).

Each step is an independent jj commit; none depends on another.

## Enforcement

**Tests.**
- Unit, in-memory `Db` not needed: register a valid script into `AppRegistry`, `reload` with `throw "boom"`, assert `entry.app` still has the original resources, `entry.script` is the new text, `script_error` is set (fix site for C1, called out as unit-testable in the report).
- TestOi harness (`crates/core/src/oi/test_support.rs`): register+install an app with a named volume, a scaled deployment, and a schedule; `/apps/update` with a parse error; assert the volume is not held, `scaling_decisions` rows intact, forwards alive, schedule rows (incl. `last_fired_at`) intact, and the response still succeeds with a `script_error` fault filed.
- Stub `System` (`crates/core/src/system/stub.rs`): run a tick with applied rules, make the instance registry error for the sole app on the next tick, assert the stub data plane saw no `apply_rules`/`apply_routes` with the app absent and that Caddy/resolver teardown was not invoked.
- Fault-injecting DB handle: make `bump_register` fail inside `register_app`, assert the registry does not contain the app and a retried `/apps/create` succeeds.

**Tracey spec items** (`docs/spec`, phrased as requirements, not mechanisms):
- interface.md, under app update: "an update whose script fails to evaluate leaves the running definition and all state derived from it — volume data, scaling decisions, forwards, schedules — unchanged".
- interface.md, under registration: "a registration that fails is not observable afterwards: the app does not appear in listings and a retried registration succeeds".
- runtime.md, reconciliation: "an installed app's applied network state is removed only by an explicit lifecycle transition, never because computing its desired state failed"; "full idle teardown occurs only when no app is installed or installing".

**Review checklist.** When a handler mutates both memory and DB: is the durable write committed before the state becomes observable, and does every early `return Err` leave both untouched? When a loop over apps `continue`s on error: is anything downstream applied as absolute state, and does an empty result trigger teardown? When a function returns a value alongside an error (`(T, Option<E>)`): may that value ever replace a previous good one?
