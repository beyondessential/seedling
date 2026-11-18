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

### Decided

- `on_change` is removed, not kept alongside. Ordered reactions become steps; configuration in environment variables or mounted content is spec-hash content the update strategies already roll.
- A step's body is a closure with `rt`. A Job-only form may come later as shorthand.
- Scope is the full model: steps, `after` on steps and on resources, and folding in the start and install actions.

### Design so far

Working name: a **step**. A static resource that the runtime keeps converged, like any other.

- **Key**: a value the definition computes in the static context, e.g. `version.value()`, or a digest of rendered config. The step is satisfied when its last success was for the current key. Where the key came from (stored value, default, derived value, definition update) does not matter.
- **Body**: a closure run as a lifecycle operation whenever the step is unsatisfied. It receives `last`, the app at the step's last success, or none on its first run. A version that moved twice while failing still upgrades from the release the database was last migrated to, and the first run is how a body tells install from upgrade.
- **Ordering**: `resource.after(step)` keeps a resource's instances for a new key from starting until the step has succeeded for that key. Existing instances keep running until then and then roll by their update strategy, because the step's key is part of the dependant's spec hash. `step.exclusive(selection)` stops the selection before the body runs and keeps it stopped until the step succeeds. `step.after(step)` chains steps.
- **Failure**: an unsatisfied step holds its dependants and files a fault; it retries with growing back-off up to a cap (no terminal give-up), and an operator can retry it at once. Cancelling a step's run pauses its retries until an operator retries it.
- **Durable state**: per step, a digest of the key and the generation of its last success, written when the body succeeds. Only a digest is stored, since a key may carry a secret (e.g. a password-rotation step keyed on the password); `last` is reconstructed from generation history.
- **Undeterminable key** (evaluation fails): the step is neither satisfied nor unsatisfied; nothing is started, stopped or run for it.
- **Changes**: with nothing per-parameter left to trigger, a change may set or unset several parameters at once.

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

Other cases from the demo definitions, as steps:

- Postgres major upgrade: a step keyed on the major, `last` giving the major to upgrade from; refusing a downgrade is a validator or a failing body.
- Postgres password rotation: a non-exclusive step keyed on the password, whose body runs `ALTER USER` against the running server.
- Postgres config rendered into a volume: either mounted content in the spec hash, or a step keyed on the rendered config's digest that the server comes after.

What else it subsumes:

- J6 for upgrades: a failed step holds its dependants by construction, across reboots.
- V14's Retry upgrade action and readiness check (a step keyed on the database URL and crypto key that every part comes after), and the install's first migration (the step's first run).
- The start action, if `after` on resources orders a boot.
- The install action, if one-off operator input moves to ordinary actions (V14's Provision).

Prior art: Kubernetes controllers (level-triggered reconcile) and init containers (per-pod gating); make and Bazel (re-run a recipe when its input key changes); systemd ordering (`After=`, `ExecCondition=`).

## Explicitly rejected

- Effective-value `on_change` (fire when the value `value()` returns moves), with a `multiple_handlers` refusal for changes that would fire more than one handler. It fixed the triggers but kept the edge-triggered model.
- A definition-update hook that runs on every definition update.
- Keeping `on_change` alongside steps.

## Superseded: desired-state hold for generation-initiated operations

Specced in Reconciliation (RCN#desired-state) before the pivot: from the commit of a generation whose change initiates an operation until that operation ends, including while queued, the app's desired state is held. With `on_change` gone no change initiates an operation, so this gives way to the step holds above; the code gap it covered (`crates/core/src/system/reconcile.rs` holding only while `active_progress` or `start_pending` is set) applies to step runs too, which have to hold their dependants from the moment the step becomes unsatisfied, including while queued.

## Docs to update

- `docs/bsl-scripting.md`: `on_change`, `on_start` and `on_install` sections and examples.
- `docs/runtime-overview.md`: initiating-events table.
- The demo definitions in `apps/` move to steps.
