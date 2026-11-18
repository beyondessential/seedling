# H6: how a definition reacts to changes

## Wider question

The narrow fix (fire `on_change` on effective-value changes) refines a model that may itself be wrong. Before settling it, review how a definition reacts to changes in its inputs, at the BSL and runtime level.

Problems with the current model:

- The trigger is the stored value, not what the definition sees. A definition update that moves a default, or a default derived from another parameter, runs no handler.
- A change sets or unsets one parameter. Parameters that belong together can't move together, which pushes definitions toward packing structured data into one parameter.
- One handler per parameter, at most one per change. The Tamanu demos repeat the same "render config, restart servers" handler for every config parameter, and there's no way to order work across parameters.
- Handlers do two different jobs: re-applying state the reconciler can't see (config rendered into a volume by a job, then a restart), and ordered transitions between generations (stop, migrate, start). The first exists because that state sits outside the resource graph, where update strategies would roll it with no handler.

## Parked: effective-value change handlers

Drafted as specs, then backed out pending the wider question. Agreed as recommendations if the per-parameter model survives:

- `on_change` fires when a change moves the parameter's effective value (what `value()` returns: stored value, else default), and only then. Set, unset and definition update are one kind of event; pinning a parameter to its current effective value fires nothing. The effective value before a change is the previous definition at the previous values; after, the new definition at the new values. The handler the new definition registers runs, with `old` as today.
- A change that would fire more than one handler is refused, with a new `multiple_handlers` wire error naming the parameters. Pinning all but one first, then unsetting each afterwards, sequences them. `/apps/plan`'s `on_change_would_fire` shows the list beforehand.
- A parameter counts as moved only if both generations declare it; a newly declared parameter starts at its value in the new generation.
- On an installed app whose current values evaluate, a parameter change whose proposed values fail to evaluate is refused, so a skipped upgrade can't hide behind a broken intermediate generation.
- Validators stay as they are: they get no `old`, so they can't guard a transition.

## Kept: desired-state hold for generation-initiated operations

Specced in Reconciliation (RCN#desired-state): from the commit of a generation whose change initiates an operation until that operation ends, including while queued, the app's desired state is held as for an operation in progress.

- Code gap: `crates/core/src/system/reconcile.rs` holds an `Installed` app's desired state only while `active_progress` is set or `start_pending` is true. `active_progress` is first written once the closure runs, so a `param_change` handler that is queued, or admitted but not yet running, leaves the new generation's whole graph as the desired state. The hold has to be in place before the new definition is visible to the reconciler, e.g. a flag like `start_pending` set in the step that swaps `app.def`.

## Docs to update

- `docs/bsl-scripting.md`: the `on_change` comment near the version example.
- `docs/runtime-overview.md`: initiating-events table.
