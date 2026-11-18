# H6: how a definition reacts to changes

## Wider question

The narrow fix (fire `on_change` on effective-value changes) refines a model that may itself be wrong. Before settling it, review how a definition reacts to changes in its inputs, at the BSL and runtime level.

Problems with the current model:

- The trigger is the stored value, not what the definition sees. A definition update that moves a default, or a default derived from another parameter, runs no handler.
- A change sets or unsets one parameter. Parameters that belong together can't move together, which pushes definitions toward packing structured data into one parameter.
- One handler per parameter, at most one per change. The Tamanu demos repeat the same "render config, restart servers" handler for every config parameter, and there's no way to order work across parameters.
- Handlers do two different jobs: re-applying state the reconciler can't see (config rendered into a volume by a job, then a restart), and ordered transitions between generations (stop, migrate, start). The first exists because that state sits outside the resource graph, where update strategies would roll it with no handler.

### What Tamanu's production definitions need (tamanu/V14)

From V14's specs (SDDEF, SDUPG) on its branch, not the demo definitions in this repo:

- Every parameter but the version is configuration passed as environment variables (no config file), and "changing one restarts the parts using it, with the new value". The patient portal URL adds or removes the portal and its ingress. All of that is resource-graph content that update strategies already roll, so none of it needs a handler.
- The version is the one ordered transition: pull the new images, tell the supervisor an upgrade started, stop every part but the supervisor, run migrations, start the new parts, replace the supervisor. It has to happen whenever the version that applies changes: a set, a definition update moving the default (re-fetching a floating tag), or a definition and version crossing a minor together.
- A failed migration leaves the parts down with a fault (J6's guard and hold), a **Retry upgrade** action reruns the upgrade to the version already set, and moving to an older release in the minor runs the same procedure.
- V14's review proposed the rule "the parts run the release the database is migrated to", a condition on state rather than a reaction to a change.

## Direction: level-triggered convergence as a first-class concept

Seedling has two halves that meet badly: the resource graph is level-triggered and unordered (the reconciler converges on it, a missed iteration costs nothing), while actions and `on_change` are edge-triggered and ordered (a missed event is lost). Every gap V14 hit sits at that boundary: a default that moved without an event (H6), a retry that is not an event (V14's Retry upgrade action), and a failed handler after which the level-triggered side starts the new release anyway (J6). The start action exists only because the graph has no ordering.

Working name: a **step**. A static resource that the runtime keeps converged, like any other.

- **Key**: a value the definition computes in the static context, e.g. `version.value()`, or a digest of rendered config. The step is satisfied when its last success was for the current key. Where the key came from (stored value, default, derived value, definition update) does not matter.
- **Body**: an action closure run as a lifecycle operation whenever the step is unsatisfied. It receives `last`, the app at the step's last success (none on first run), rather than the previous generation, so a version that moved twice while failing still upgrades from the release the database was last migrated to.
- **Ordering**: `resource.after(step)` keeps a resource's instances for a new key from starting until the step has succeeded for that key. Existing instances keep running until then and then roll by their update strategy, because the step's key is part of the dependant's spec hash. `step.exclusive(selection)` stops the selection before the body runs.
- **Failure**: an unsatisfied step holds its dependants and files a fault; it retries with growing back-off up to a cap (no terminal give-up), and an operator can retry it at once.
- **Durable state**: per step, the key and generation of its last success, written when the body succeeds.
- **Undeterminable key** (evaluation fails): the step is neither satisfied nor unsatisfied; nothing is started, stopped or run for it.

Tamanu sketch:

```rhai
let migrate = app.step("migrate")
    .key(version.value())
    .exclusive(parts)                  // api, tasks, sync, fhir, web stopped first
    .run(|rt, last| {
        signal_upgrading.call(rt);
        rt.start(migrate_job).terminated(3600).ensure_success();
    });

parts.after(migrate);                  // new release starts only once migrated
supervisor.after(migrate);             // rolls last, never stopped by the step
```

What it subsumes or replaces:

- `on_change`: configuration in environment variables or mounted content is spec-hash content the update strategies already roll; ordered reactions are steps keyed on what they depend on.
- The one-parameter-per-change rule (#M6): nothing per-parameter is left to trigger, so several parameters can change at once.
- J6 for upgrades: a failed step holds its dependants by construction, across reboots.
- V14's Retry upgrade action and readiness check (a step keyed on the database URL and crypto key that every part comes after), and the install's first migration (the step's first run).
- Possibly the start action, if `after` is enough to order a boot.

Prior art: Kubernetes controllers (level-triggered reconcile) and init containers (per-pod gating); make and Bazel (re-run a recipe when its input key changes); systemd ordering (`After=`, `ExecCondition=`).

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
