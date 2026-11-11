---
id: OPS
---

# Lifecycle operations

## Lifecycle Operations

<!-- from runtime: Lifecycle Operations -->

> r[operation.lifecycle]
> A lifecycle operation is the top-level unit of scripted orchestration.
> It is a single execution (possibly interrupted and replayed) of an action closure.

> r[operation.lifecycle.single]
> At most one lifecycle operation may be in progress across all applications on a node at any time.

> r[operation.lifecycle.single.intra-app]
> If a lifecycle operation is requested for an application that already has one in progress, the request must be rejected immediately.

> r[operation.lifecycle.single.inter-app]
> If a lifecycle operation is requested for an application while a different application's operation is in progress, the request must be queued.
> There may be at most one queued operation per application.
> If an operation is already queued for that application, the new request must be rejected.
> Queued operations are started in request order once the current operation completes.

> r[operation.lifecycle.events]
> Lifecycle operations are initiated by these events:
>
> - **Normal boot** (prior state exists, no interrupted operation): the `start` action.
> - **Boot, interrupted operation exists**: replay of the interrupted operation. This includes installs that were in progress when the runtime stopped; an app persisted in the `Installing` status must have its install replayed from the persisted params and action log.
> - **Param change**: the `on_change` handler registered on the parameter that changed, when the change matches one of the [transitions](rule:l/param.on-change.transitions) in the language spec, the app is installed, and a handler is registered.
> - **Operator request**: a named action, including `install`.
> - **Schedule**: a BSL `on_schedule` cron expression fires.

> r[operation.lifecycle.param-change]
> A param change is a lifecycle operation.
> It is subject to the same [concurrency restrictions](rule:r/operation.lifecycle.single) as all other lifecycle operations.
> Only one parameter may be changed at a time, including one changed alongside a definition update.
>
> Checking that no other operation holds the app and admitting the change's `on_change` handler are one decision with respect to admitting any other operation for the app, so none is admitted after the change is committed and before its handler.

> r[operation.lifecycle.generations]
> Every lifecycle operation record must carry a *source generation* and a *target generation*.
>
> - For an operation triggered by a [generation bump](rule:r/generation.bumps) (param change, script update), the source generation is the generation that was current immediately before the bump, and the target generation is the new generation produced by the bump.
> - For an operation not triggered by a generation bump (operator-invoked actions, `start`, replay), the source and target generations are equal — the current generation at the time of dispatch.
>
> Replay of an interrupted operation must reconstruct the AppDef using the operation's stored target generation, and `old` (when applicable) using the source generation.
> This must hold even if further generations have been committed in the meantime; the operation always sees the world it was scheduled for.

> r[operation.lifecycle.completion]
> When a lifecycle operation completes (the action closure returns), the full [desired state](rule:r/desired-state.steady) — derived from the AppDef at the app's current generation — takes effect and the reconciler maintains it autonomously.
> The operation's outcome is recorded in the [generation history](rule:r/generation.history) entry that triggered it (when applicable).

> r[operation.cancel]
> The runtime must expose a way for operators to cancel an in-progress lifecycle operation.
>
> When cancellation is requested, the runtime must wake any currently-suspended barrier — including deadline-less barriers such as [`.terminated_eventually()`](rule:l/rt.started.terminated-eventually) — so that the cancel takes effect within the same observation cycle rather than waiting for the next poll or deadline.
>
> A cancelled operation reaches a terminal state distinct from success and from failure. Cleanup (dynamic resource teardown, current-operation clearing) must run as for a failed operation, and the outcome must be recorded so operators can tell a cancel apart from an ordinary failure after the fact.

> r[operation.cancel.persistence]
> A cancel request must persist across a runtime restart. When the daemon crashes after the cancel was accepted but before the operation observed it, the replay must resume into a pre-cancelled state so the operation terminates at its next barrier rather than re-executing work the operator has already asked to abandon.
>
> The persisted cancel flag is scoped to a single `operation_id`. A later operation — whether resumed after completion of the current one or spawned fresh — must not inherit a cancel from a stale row.

> r[operation.cancel.stuck-recovery]
> An app whose persisted phase is `Installing` but for which no `current_operation` row matches must be recoverable: the operator must always have a path to an actionable state.
>
> The runtime must implement this in two places. On daemon startup, after loading apps and the persisted current operation, any app stuck in `Installing` with no matching current_operation must be transitioned to `Uninstalling`. The cancel-action endpoint must, when called on an app in this state, perform the same transition and return success rather than failing with "no active operation".
>
> The transition target is `Uninstalling` rather than `NotInstalled` because a partially-installed app may already have running containers or units from `rt.start()` calls the previous operation made before its interruption. The reconciler skips `NotInstalled` apps, so a direct revert would strand those resources. `Uninstalling` engages the normal teardown path; the reconciler converges to `NotInstalled` once the teardown completes.
>
> Stuck states arise from a previous run whose cancel cleanup cleared `current_operation` but whose phase-revert persist was interrupted by crash or restart, or from a daemon that died between persisting the phase and persisting the operation row.

> r[operation.params]
> When a lifecycle operation is dispatched with params, the params must be persisted alongside the operation record. On replay, the persisted params must be restored and passed to the action closure.
> Params may contain secret values, so the persisted form must be encrypted with the same cipher used for stored secret params.
>
> Shell sessions do not persist params; shells are not replayable.

## Operation Volume Params

<!-- from runtime: Lifecycle Operations / Operation Volume Params -->

Some operations must hand a writable or read-only path to an action closure without giving that path a stable, app-visible name, such as the site volume an operator supplies for a `volume` action param. The runtime uses reserved param keys and operation-scoped volume bindings to do this in a collision-free way.

> r[operation.volume-param]
> When an internal operation hands a volume to an action closure, it must not use a fixed name hard-coded in the BSL script. Instead, for each logical binding the operation wants to provide, the runtime must:
>
> 1. Generate a fresh name that is unique for the operation — typically `seedling-op-<operation-id>-<logical-key>`. The name must not appear anywhere in the operator-provided param map.
> 2. Register that name in the operation-scoped volume binding table (see [volume.external.dynamic](rule:l/volume.external.dynamic)), mapping it to the actual filesystem path and the intended `read_only` flag. The binding lives for the duration of the operation and is removed when the operation ends.
> 3. Insert a param named `<logical-key>_volume` into the `param` map delivered to the action closure, whose value is the generated name.
>
> The action closure resolves the volume by reading the param, then calling `app.external_volume(param["<logical-key>_volume"])`.
>
> Because the generated name is produced by the runtime, not chosen by the script, different operations cannot collide with each other and cannot collide with operator-configured static external volume mappings.

> r[operation.volume-param.filename]
> An operation may additionally instruct an action closure to read or write a specific filename under an injected volume. It must not rely on a fixed filename known only to the runtime: instead, for each `<logical-key>_volume` param it adds, the runtime may add a companion param named `<logical-key>_filename` whose value is the filename the action closure must use.
>
> This keeps the filename an implementation detail of the runtime — the runtime may rename it or change its structure without requiring every app's BSL script to be rewritten.

> r[operation.volume-param.reserved]
> Param keys ending in `_volume` or `_filename` are reserved. The runtime must reject operator-provided params whose keys end in either suffix (see [action.invoke](rule:i/action.invoke) and [action.params](rule:l/action.params)). Only the runtime may insert such keys, and only as described in [operation.volume-param](rule:r/operation.volume-param) and [operation.volume-param.filename](rule:r/operation.volume-param.filename).

## Action Composition

<!-- from runtime: Lifecycle Operations / Action Composition -->

> r[operation.composition]
> An action closure may invoke other actions by calling [`Action.invoke(params?)`](rule:l/action.call) on an Action handle obtained from [`app.action(name)`](rule:l/action.lookup) or returned by `app.on_action()` / `app.on_start()`.
> The invoked action's closure runs inline within the calling operation, sharing its `rt`, its operation_id, and its action log.
> Barriers within the invoked action are barriers of the overall operation.

> r[operation.composition.cycles]
> The runtime must reject cycles in action invocation before the called closure runs.
> A `.call()` whose action name is already on the current call stack — directly (the action calling itself) or transitively (any earlier frame on the stack) — must throw.
> The check uses the action name; renaming a captured closure does not bypass it.
> The thrown error must name the offending chain, e.g. `start → foo → bar` for the `bar.call()` that closes the cycle, so the operator can identify the script bug.

> r[operation.composition.params]
> The runtime applies the called action's declared param schema to the supplied `params` map before invoking the closure, using the same validation rules as operator invocation: required-field enforcement, default application, [reserved-key](rule:l/action.params) rejection, and `kind: "volume"` resolution against the [operation-scoped volume bindings](rule:r/operation.volume-param). Validation errors throw before the closure runs.
> The result of validation must be deterministic across replays so that the action log entry recorded for the call (see [history.action-log.entries](rule:r/history.action-log.entries)) faithfully describes what the closure observed.

> r[operation.composition.params.replay]
> A sub-action call already recorded in the action log is replayed with the params its entry holds, and is not validated again.
> Validation answers to the world as it stands — it resolves `kind: "volume"` params against the [operation-scoped volume bindings](rule:r/operation.volume-param) and applies the action's declared schema as the current definition declares it — so re-running it would let a call that was committed with valid params stop replaying because an operator deleted the referenced site volume or an update changed the schema.
> A recorded call whose entry names a different action than the one the closure is now calling is a [replay divergence](rule:r/barrier.replay.positional), not a call to replay.

## Shell Sessions

<!-- from runtime: Lifecycle Operations / Shell Sessions -->

> r[operation.shell]
> Shell actions are not lifecycle operations.
> They may run concurrently with a lifecycle operation and with other shell sessions.

> r[operation.shell.resources]
> Resources created within a shell session are dynamic.
> They are cleaned up when the session ends.
