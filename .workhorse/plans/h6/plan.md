# H6: level-triggered gates

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

- `on_change` is removed, not kept alongside. Ordered reactions become gates; configuration in environment variables or mounted content is spec-hash content the update strategies already roll.
- A gate's body is a closure with `rt`. A Job-only form may come later as shorthand.
- Scope is the full model: gates, `after` on gates and on resources, and folding in the start and install actions.
- Satisfaction is a recorded key plus an optional `.check()` probe: a failing probe makes the gate unsatisfied again, so a database restored behind Seedling's back re-runs the migration.
- The start action is removed; boot order comes from `after`. After a Deployment means its instances are running and healthy, after a Job that it completed successfully; satisfied gates don't re-run at boot.
- `after` stays within one app. Apps never depend on other apps: a definition depends on a contract the operator fulfils (Tamanu's database), whose parts can come after a gate whose check probes that contract.
- A gate's run holds only its dependants and its exclusive selection; the rest of the app keeps converging.
- The install action is removed. Install moves the app straight to `Installed` and the gates' first runs do the setup. Install parameters go; one-off operator input is an ordinary action run after install, and an app that needs it starts unprovisioned until the operator runs it.
- A gate's check is a container command (exit 0 is satisfied), not a closure. It runs before the gate's dependants start from stopped (at boot and after the gate's own run), and on an optional interval.
- A key is any BSL value; maps and arrays are digested in a canonical form.
- The concept is named a gate (`app.gate(name)`).

### Design so far

Working name: a **gate**. A static resource that the runtime keeps converged, like any other.

- **Key**: a value the definition computes in the static context, e.g. `version.value()`, or a digest of rendered config. The gate is satisfied when its last success was for the current key. Where the key came from (stored value, default, derived value, definition update) does not matter.
- **Body**: a closure run as a lifecycle operation whenever the gate is unsatisfied. It receives `last`, the app at the gate's last success, or none on its first run. A version that moved twice while failing still upgrades from the release the database was last migrated to, and the first run is how a body tells install from upgrade.
- **Ordering**: `resource.after(gate)` keeps a resource's instances for a new key from starting until the gate has succeeded for that key. Existing instances keep running until then and then roll by their update strategy, because the gate's key is part of the dependant's spec hash. `gate.exclusive(selection)` stops the selection before the body runs and keeps it stopped until the gate succeeds. `gate.after(gate)` chains gates.
- **Failure**: an unsatisfied gate holds its dependants and files a fault; it retries with growing back-off up to a cap (no terminal give-up), and an operator can retry it at once. Cancelling a gate's run pauses its retries until an operator retries it.
- **Durable state**: per gate, a digest of the key and the generation of its last success, written when the body succeeds. Only a digest is stored, since a key may carry a secret (e.g. a password-rotation gate keyed on the password); `last` is reconstructed from generation history.
- **Undeterminable key** (evaluation fails): the gate is neither satisfied nor unsatisfied; nothing is started, stopped or run for it.
- **Changes**: with nothing per-parameter left to trigger, a change may set or unset several parameters at once.

Tamanu sketch:

```rhai
let migrate = app.gate("migrate")
    .key(version.value())
    .exclusive(parts)                  // api, tasks, sync, fhir, web stopped first
    .run(|rt, last| {
        signal_upgrading.call(rt);
        rt.start(migrate_job).terminated(3600).ensure_success();
    });

parts.after(migrate);                  // new release starts only once migrated
supervisor.after(migrate);             // rolls last, never stopped by the gate
```

Other cases from the demo definitions, as gates:

- Postgres major upgrade: a gate keyed on the major, `last` giving the major to upgrade from; refusing a downgrade is a validator or a failing body.
- Postgres password rotation: a non-exclusive gate keyed on the password, whose body runs `ALTER USER` against the running server.
- Postgres config rendered into a volume: either mounted content in the spec hash, or a gate keyed on the rendered config's digest that the server comes after.

What else it subsumes:

- J6 for upgrades: a failed gate holds its dependants by construction, across reboots.
- V14's Retry upgrade action and readiness check (a gate keyed on the database URL and crypto key that every part comes after), and the install's first migration (the gate's first run).
- The start action.
- The install action, subject to the summary below.

### Proposed defaults (not yet put to the user individually)

- `after` orders starts only. A dependant waits for its target before starting; it is not stopped when the target later goes down. Only `exclusive` stops anything. A target the operator has stopped or scaled to zero keeps its dependants waiting, and status says what they wait on.
- A cycle in `after` throws at evaluation.
- A body that succeeds while the gate's check then fails counts as a failed run, so a check that never passes backs off rather than looping.
- Before the exclusive stop, the runtime pulls the images the gate's body and its dependants' new specs need; a failed pull fails the run with nothing stopped.
- Unsatisfied gates run in `after` order, then in declaration order, one lifecycle operation at a time as today. Operator and scheduled actions queue alongside them.
- A gate's body gets the full `rt`. When its run ends, the app returns to its whole graph as after any operation, apart from what the gate holds.
- Uninstalling clears gate records, so a reinstall repeats every first run.
- Faults: a withholding `gate_failed` per gate (app `faulted`), filed on a failed run and cleared on success.
- Interface: gates and their state (satisfied, pending, running, failed) in `/apps/show`; `/apps/gates/retry`; a gate event; `apps gate retry` in the CLI; a retry button in the console. `/apps/plan` reports the gates a change would make unsatisfied, in place of `on_change_would_fire`. Image discovery probes gate bodies and checks in place of install, start and `on_change` handlers.

### Removing the install action

What install does today, and where each goes:

- First-run setup (initdb, first migration, render config, create roles, warm certificates): the gates' first runs, with `last` absent. One-off setup is a gate with a constant key.
- A pre-flight check before anything starts: a gate every part comes after.
- Bringing parts up in order: `after`.
- Retrying a failed install, cancelling one: gate back-off and operator retry; cancel pauses the gate, uninstall backs out.
- Replay, `install_interrupted`, and the runtime's path out of a stuck `Installing` phase: no longer needed, since install no longer runs a closure.

Costs that have a workaround:

- A failed first run leaves the app `Installed` and faulted, not `NotInstalled`. Removing an app that never worked takes an uninstall before the remove.
- The `installing` status could be derived (installed, with a gate that has never succeeded) so the console and Canopy reporting keep it.
- Install parameters (operator input validated against a schema, held only for the install) move to an ordinary action run after install, as V14's Provision is. Actions already take the same schemas, and volume params too, which install parameters can't.

The one thing with no direct replacement: gating the app's start on operator input that is never stored. With install parameters, the initial admin password is given before anything runs and is never kept. As an action, the parts are already up when the operator provides it. Ways round it:

- Accept it: the app starts unprovisioned and the operator runs the action (V14's choice; a Tamanu facility uses its setup wizard anyway).
- Store it as a secret parameter and key a gate on it, giving up "never stored".
- Let an action's success satisfy a gate (`gate.by_action(provision)`), so dependants wait for the operator. Another concept.

Prior art: Kubernetes controllers (level-triggered reconcile) and init containers (per-pod gating); make and Bazel (re-run a recipe when its input key changes); systemd ordering (`After=`, `ExecCondition=`).

## Spec drafting

Drafted: Gates and start order (GATE), Gate runs (GRUN), and the removal of change handlers, the start action and the install action across the BSL, runtime and interface specs.

- `/apps/install/invoke` becomes `/apps/install`, taking only the app; `install_in_progress` and `install_interrupted` go.
- Parameter and definition changes are no longer refused while an operation runs or is queued, only while another change is being applied: with no handler to admit, nothing needs the app idle.
- `/apps/show` gains `gates` and per-resource `waiting_on`; a `GateStateChanged` event; `/apps/gates/run` and `apps gate run`; `/apps/plan` reports `gates_would_run`.
- Gate faults (`gate_failed`, `gate_check_unavailable`, `gate_state_unreadable`) are withholding, so an app whose gate is failing reads `faulted`; `installing` is derived from a gate with no last success.

Still to draft: several parameters per change (params, update, events, generation history, CLI, console), pending the API shape.

## Other cards in flight

- F6 (shell sessions' dynamic resources, implemented, not merged): no conflict in behaviour. It edits the same places: the desired-state section of Reconciliation, Lifecycle operations, the runtime handle, `system/reconcile.rs`, `runtime/desired.rs` and `oi/handler/actions/lifecycle.rs`. Expect to rebase over it.
- G6 (DTLS termination, implemented, not merged): proxy and ingress only; overlaps in `system/reconcile.rs` and the faults spec, textually.
- J6 (guards and holds, specced, implementation waiting on this card): overlaps in behaviour. Its holds reference the boot start action this card removes, and a gate's exclusive selection covers the failed-upgrade case it was raised for. Revisit J6 once the gate specs are drafted.

## Explicitly rejected

- Effective-value `on_change` (fire when the value `value()` returns moves), with a `multiple_handlers` refusal for changes that would fire more than one handler. It fixed the triggers but kept the edge-triggered model.
- A definition-update hook that runs on every definition update.
- Keeping `on_change` alongside gates.

## Superseded: desired-state hold for generation-initiated operations

Specced in Reconciliation (RCN#desired-state) before the pivot: from the commit of a generation whose change initiates an operation until that operation ends, including while queued, the app's desired state is held. With `on_change` gone no change initiates an operation, so this gives way to the gate holds above; the code gap it covered (`crates/core/src/system/reconcile.rs` holding only while `active_progress` or `start_pending` is set) applies to gate runs too, which have to hold their dependants from the moment the gate becomes unsatisfied, including while queued.

## Docs to update

- `docs/bsl-scripting.md`: `on_change`, `on_start` and `on_install` sections and examples.
- `docs/runtime-overview.md`: initiating-events table.
- The demo definitions in `apps/` move to gates.
