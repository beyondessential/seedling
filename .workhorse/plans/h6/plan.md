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

- Several parameters per change: `/apps/params/set` takes `params` (name to string, or `null` to unset) and `/apps/params/unset` goes; `/apps/update` and `/apps/plan` take the same object; `ParamSet`/`ParamUnset` become `ParamsChanged` with `changes`, a shape `AppUpdated` shares; history kind `param_change` with a `params` list; `apps params set <app> a=1 --unset b` and repeatable `--set`/`--unset` on `apps update`.
- Code citations of removed sections (`PARAM#change-handlers`, `ACN#start-action`, `ACN#install-action`, `ACI#install-parameters`, `ACI#install-completion`, `OPS#stuck-install-recovery`, `OPS#parameter-changes`, `PRM#parameter-set`, `PRM#parameter-unset`, `CLI#parameter-change-on-update`, `CLI#action-shell-and-install-parameters`) go with the code changes.

Fresh-eyes review round: findings folded in. Decisions taken with it:

- `rt.start`/`rt.stop` leave out what a gate holds and workloads waiting on an unsatisfied gate, except in that gate's own body (J6's rule for its holds, kept for gates).
- A run invalidates the gate's last success when it starts, and a failed check invalidates it too, so the gate is unsatisfied until a run succeeds.
- A stale instance is not stopped while its workload waits on a target that is not ready, whatever its update strategy.
- `installing` means installed and not yet set up (every gate satisfied at once since install, recorded durably).
- Actions and schedules don't wait for setup.
- A gate hold covers the current definition's exclusive selection; a gate no longer declared releases it and keeps its record until uninstall.
- Source and target generations collapse to one operation generation, current at dispatch; operation events, `/apps/list` and `current_operation` carry `gate` for a gate run.
- A cancelled gate run pauses the gate durably and files `gate_failed`.

Next: build, following the steps below.

## Other cards in flight

- F6 (shell sessions' dynamic resources) and G6 (DTLS termination): merged, and this card is rebased over them. F6's shell `rt.start` returns a `Started` over its whole selection, now less what a gate holds.
- J6 (guards and holds): absorbed and cancelled. A gate's exclusive selection is its guard, held through failure, cancel and restart and released when the gate is satisfied. Taken from it: `rt.start`/`rt.stop` leave held resources out; an operator stop of a held resource is accepted and outlasts the hold; scaling decisions take effect once the hold ends; a restart of a held deployment is refused; `/apps/show` reports `held_by` per resource.

## Explicitly rejected

- Effective-value `on_change` (fire when the value `value()` returns moves), with a `multiple_handlers` refusal for changes that would fire more than one handler. It fixed the triggers but kept the edge-triggered model.
- A definition-update hook that runs on every definition update.
- Keeping `on_change` alongside gates.
- J6's `rt.guard` holds alongside gates, and with them durable holds for ordinary actions and `/apps/resource/unhold`/`/apps/unhold`. An operator ends a gate hold by running the gate or moving its key.
- A `held` key on `Started.select`: a gate holds its selection `Unscheduled`, so there are no held instances to filter.

## Superseded: desired-state hold for generation-initiated operations

Specced in Reconciliation (RCN#desired-state) before the pivot: from the commit of a generation whose change initiates an operation until that operation ends, including while queued, the app's desired state is held. With `on_change` gone no change initiates an operation, so this gives way to the gate holds above; the code gap it covered (`crates/core/src/system/reconcile.rs` holding only while `active_progress` or `start_pending` is set) applies to gate runs too, which have to hold their dependants from the moment the gate becomes unsatisfied, including while queued.

## Implementation notes

Grounded in the code as of the rebase over F6 and G6.

- **Closures.** `ClosureCapture` (`defs/app.rs`) holds `install` and `param_changes` today; it gains `gates: BTreeMap<GateName, Option<FnPtr>>` and loses both. `AppDef` gains `gates: BTreeMap<GateName, GateDef>` (key, check Job and interval, exclusive selection as resolved resource ids, `after` targets, body present or not) and loses `install` and `param_changes`. `AppDef` stays `Send`; the body is picked from the capture at run time, as actions are in `run_operation`.
- **`after` on pods.** Stored on `PodDef` as a list of targets (gate, Deployment or Job by name), resolved and cycle-checked once the script's evaluation completes, alongside the gates' exclusive selections and the self-wait checks.
- **Keys.** Canonical JSON: `serde_json::Value` with map members sorted, which keeps integers and floats apart. The digest is a keyed MAC under a subkey derived from the host's secret key, added to `runtime/secrets.rs` beside the AEAD `Cipher` (there is no keyed digest helper today). The same digest feeds the gate record and the spec hash.
- **Spec hash.** `Actuator::desired_spec_hash` (`system/actuator.rs`) folds in `restart_gen` today; it also folds in the key digests of the gates a workload is ordered directly after. Nothing in `defs` feeds the hash, so this is the only place.
- **Gate state.** A new `runtime/gates.rs` owns the records, the per-iteration state (satisfied, unsatisfied, undetermined as a distinct variant, never folded into either), initiation, back-off, pauses, checks and setup. It is the one place that decides whether a gate runs; the reconciler and the OI ask it rather than reading records themselves.
- **Holds and start order** go into the desired-state computation (`runtime/desired.rs::compute_steady`): held resources join the stopped set as `Unscheduled` whatever their scale, workloads waiting on an unready target get no new instances (the `held_starts` path in `system/reconcile/pods.rs`), and their stale instances join `compute_stop_inhibitions`. The `start_pending` hold and the `Installing` branch in `system/reconcile.rs` go.
- **Checks** start the check Job as a fresh instance outside the desired state, from `runtime/gates.rs`, and read the outcome the way `db_run_failed` reads a Job's exit. Job readiness as a target reads the most recent instance's outcome, not the in-memory `completed_jobs` set.
- **Operations.** Triggers stay strings: `boot`, `install` and `param_change` go, `gate` comes in. `source_generation`/`target_generation` collapse to one generation in `ActiveOperation`, `QueuedOperation`, `CurrentOperation`, `OperationContext` and the events. `AppHold::admit` and `schedule_on_change` go; the change hold stays, refusing operations while a change is applied, but `hold_for_change` no longer refuses a change while an operation runs or is queued.
- **Generation history.** Multi-parameter entries go in a child table keyed by app, generation and parameter name, with the plaintext and ciphertext value columns `write_param_change` uses today. `operation_id`, `outcome` and `outcome_error` on `generations` go with `attach_operation`, `record_outcome` and `record_operation_outcome`.
- **Phase.** `AppPhase::Installing` and the `installing` column go. Seedling is unshipped, so the migration moves any `installing` app to installed and drops queued `boot`, `install` and `param_change` requests rather than carrying them.
- **`ParamDef`/`ParamKind`** stay (parameters, actions and shells use them) and move out of `defs/install.rs`, which goes with the install action.
- **Withholding faults.** `WITHHOLDING_KINDS` in `runtime/faults.rs` is a fixed array with a three-placeholder query in `fault_standing`; build the placeholders from the array so adding the gate kinds does not need the SQL edited by hand.

## Build steps

### Stages

Build order, with the labels used in commit history and conversation (these are labels for slices of the steps below, not extra work). Tick the steps as each lands; a stage is done when every step it names is ticked.

- **A, several parameters per change:** done (Parameters and generations, the parameter parts of Operator Interface, CLI and Console).
- **B1, install and start actions removed:** done.
- **B2, `on_change` and edge-triggered plumbing removed:** done.
- **G1, BSL gates and storage:** done (the BSL steps, the keyed digest, the spec hash).
- **G2, gate runtime and reconciler:** built but not audited; its steps stay unticked until each is checked against its code and test. Gate runs, checks, holds and start order exist in code; `AppStatus::Installing` derivation and the image probe's `HandlerKind::Gate` landed with it.
- **G3, gate surfaces:** not started. `/apps/show` gates, `held_by`, `waiting_on`; `/apps/gates/run`; `/apps/restart` and `/apps/resource/stop` for held resources; `gates_would_run`; `apps gate run`; the console's gate parts; the OI and event tests.
- **G4, definitions, docs and test cases:** not started. The demo definitions, docs, test cases, citations for the gate code.

### Baseline

- [x] Run `cargo clippy`, `cargo fmt --check`, `etc/ci/check-spec-style.sh` and the `etc/ci/` greps on the rebased branch and fix anything outstanding (the citation check is expected to fail only on the removed sections)

### Storage

- [x] One migration for the whole card, v82 (`runtime/db/migrations/v82.sql`, registered in `runtime/db.rs`, version asserts in `runtime/db/tests.rs`). Seedling is unshipped and the card is one piece of work, so any further schema change goes into this file, never a new version; its tests seed a v81 database and migrate to the latest:
  - [x] `gate_records`: app, gate, key digest, generation of last success, valid, consecutive failures, last failure time and description, paused-for key digest
  - [x] `gate_holds`: app, gate, covered resources
  - [x] `registered_apps.set_up`; drop `installing`, moving installing apps to installed
  - [x] `generation_params` child table; move `param_set`/`param_unset` rows and `script_update` rows with a parameter into it, set kind `param_change`, drop `param_name`/`previous_value`/`new_value` and their ciphertext columns, and `operation_id`/`outcome`/`outcome_error`
  - [x] `current_operation` and `queued_operations`: one `generation` column and a `gate` column beside the action; drop queued `boot`, `install` and `param_change` rows
- [x] Keyed digest helper in `runtime/secrets.rs`, with a test that the digest differs across secret keys and is stable for one

### BSL

- [x] `defs/gate.rs`: `Gate` builder (`key`, `run`, `check`, `exclusive`, `after`) registered in `defs::register`, `app.gate(name)` in `defs/app/gate.rs`; static-context and frozen checks as Deployment/Job do; conversions through `defs::take`
- [x] Key canonicalisation and the JSON-representable check, throwing on anything else
- [x] `check`: named Job with a deadline, one gate per Job; `exclusive`: no dynamic resources; resolution of `exclusive(app)` at the end of evaluation
- [x] `after` on `PodDef::mixin` (Deployment and Job), static context only
- [x] End-of-evaluation checks: `after` cycles, exclusive selection holding a target, check Job in its own gate's selection or ordered after it
- [x] Remove `Param::on_change` (`defs/param.rs`), `app.on_start` and the `"start"`/`"install"` name rules in `defs/app/action.rs`, `app.on_install` and `defs/app/install.rs`; move `ParamDef`/`ParamKind` out of `defs/install.rs` and update every import
- [x] `run_operation` (`runtime/barrier/replay.rs`): dispatch a gate body from the capture, call it with `rt` and `last` by arity, build `last` with `generations::reconstruct_app_def` at the gate's last success or pass unit; drop the install, start-default and change-handler paths and `change_handler_call_script`; `check_idempotent` compares gates
- [x] `rt` exclusions (`runtime/barrier/runtime.rs`): `extract_instances`/`do_start_or_query`/`do_stop` leave out gate-held resources and workloads waiting on an unsatisfied gate, except the running gate's own; `rt.restart` throws on them; a shell's `Started` leaves them out too

### Gate runtime

- [x] `runtime/gates.rs`: record load and write (unreadable is its own outcome), key digests from the evaluated definition, state per gate
- [x] Initiation from the reconciliation loop: due when unsatisfied, not paused, targets ready, back-off due; declaration order; admit with trigger `gate`; a refused admission is retried, not failed
- [x] Run preparation: invalidate the record, pre-pull the body's probed images, the check's and the dependants', fail with nothing stopped on a pull failure; record the hold, stop the selection, wait for Terminated
- [x] Outcome: run the check at the run's generation, write the success record (digest, generation, valid) or count the failure; a failed record write fails the run; clear the hold on satisfaction
- [x] Back-off (30s doubling to 1h, due at once on a key move), pauses on cancel, `gate_failed` while unsatisfied after a failed or cancelled run
- [x] Checks: before starting a dependant with no running instance, after a body, on the interval outside runs; check Job started outside the desired state and judged by exit status; a failed check invalidates; an unstartable check makes the gate undetermined and holds `gate_check_unavailable` with back-off
- [x] `gate_state_unreadable`; gates no longer declared (withdraw queued run, clear faults, release hold, keep record); setup record; uninstall deletes records, holds and setup, retrying before `NotInstalled`
- [x] Gate faults in `WITHHOLDING_KINDS`, with `fault_standing` building its placeholders from the array

### Operations and lifecycle

- [x] Collapse source/target generation across `runtime/scheduler.rs`, `runtime/history.rs`, `oi/handler/actions/lifecycle.rs`, `runtime/barrier/replay.rs`, `oi/shells/session.rs`, `runtime/gc.rs`
- [x] Persist and restore gate runs (`gate` instead of an action) in the current-operation and queue records; replay a gate run
- [x] `lifecycle.rs`: drop `enter_installing_phase`, `finalize_install`, `revert_install_phase`, `recover_unreplayable_install`, `initiate_start`, `start_pending`; completion returns to steady state; cancelled and failed gate runs file `gate_failed`, not `operation_failed`/`operation_cancelled`; drop the outcome writes to generation history
- [x] `oi/handler/actions.rs::cancel_action`: drop the Installing path
- [x] `runtime/scheduler.rs`: drop `AppHold::admit` and `ChangeBeingApplied` uses tied to `on_change`; keep the change hold blocking operation admission
- [x] `crates/daemon/src/main.rs`: drop boot starts and install replay; the startup sweep moves only `NotInstalled` apps with recorded instances to `Uninstalling`
- [x] `runtime/schedules.rs`: phase check without `Installing`

### Reconciler

- [x] `runtime/desired.rs::compute_steady`: gate holds as `Unscheduled` whatever the scale, start order, check Jobs out of the desired state; gate run desired state as steady plus the body's starts and stops
- [x] `system/reconcile.rs`: drop the `Installing` and `start_pending` branches; `AppEntry::is_steady` without `start_pending`; gate state evaluated per iteration through `runtime/gates.rs`
- [x] `system/reconcile/pods.rs`: no new instance of a waiting workload; no stale instance stopped while its workload waits, under rolling, replace or Job
- [x] `system/actuator.rs::desired_spec_hash`: fold in the direct gate targets' key digests
- [x] Job readiness as a target from the most recent instance's outcome

### Parameters and generations

- [x] `runtime/generations.rs`: `Kind::ParamChange` with a list; `bump_param_change` takes several changes; `param_map_at` and `reconstruct_app_def` read the child table; `script_update` with several parameters
- [x] `oi/handler/params.rs`: `/apps/params/set` with `params` (refusals for empty, duplicate names, bad names, non-string values), one generation and one `ParamsChanged`; remove `unset_param` and `schedule_on_change`; `hold_for_change` refuses only a concurrent change
- [x] `oi/handler/apps.rs::update_app`: `params` object with the same refusals; `AppUpdated` with `changes`
- [ ] `oi/handler/apps.rs::dry_run_plan`: `proposed_params` object, `gates_would_run` in place of `on_change_would_fire` (done: `proposed_params` object; open: `gates_would_run` is an always-empty placeholder until gate state feeds it)

### Operator Interface and protocol

- [x] `crates/protocol/src/events.rs`: `ParamsChanged` and the change entry shape shared with `AppUpdated`; drop `ParamSet`/`ParamUnset` and `AppUpdatedParam`; operation events with `operation_generation` and optional `gate`; `GateStateChanged`; builders on both senders; wire-shape tests in `events/tests.rs`
- [x] `crates/protocol/src/error.rs`: drop `InstallInProgress`
- [ ] `oi/handler.rs` routing: `/apps/install` (app only, refusals per ACI) in place of `/apps/install/invoke`; drop `/apps/params/unset`; add `/apps/gates/run` with the refusals in CTL; delete `oi/handler/actions/install.rs` and its tests (done: `/apps/install`, `/apps/params/unset` dropped; open: `/apps/gates/run`)
- [ ] `/apps/show`: `gates`, per-resource `held_by` and `waiting_on`, `current_operation` with `gate` and `generation`, action kinds without `install`; `/apps/list` with `gate`; status from the setup record and gate faults (`runtime/apps/status.rs`)
- [x] `/apps/generations`: `param_change` with a `params` list; no operation outcome
- [ ] `/apps/restart` refused while a gate holds the deployment; `/apps/resource/stop` accepted for a held resource
- [x] `oi/test_support.rs`: `TestOi::install` against `/apps/install`

### CLI

- [x] `crates/ctl/src/apps.rs`: `apps install <app>` without parameters (drop `parse_install_params`); `apps params set <app> name=value... --unset name...`; drop `apps params unset`; `apps update --set/--unset` repeatable, refusing a parameter named twice (`definition::param_change` in `crates/ctl/src/definition.rs`); `apps plan --param` building the object, with `--unset`
- [ ] `apps gate run <app> <gate>` as a `GateCommand` beside `ParamCommand`

### Console

- [ ] `crates/web/frontend/src/lib/types.ts`: gates, `held_by`, `waiting_on`, `CurrentOperation.gate`/`generation`, `GenerationKind` and entry `params`, `PlanResponse.gates_would_run`, event fields; drop install requirement types
- [ ] `routes/AppDetail.tsx`: install without the parameter dialog; params section staging several edits into one `/apps/params/set`; a gates section with state and a run button; held and waiting markers on resources; current-operation banner for gate runs; event lists (done: install without the dialog, staged params; open: gates section, held and waiting markers, gate banner)
- [ ] `components/UpdateDefinitionDialog.tsx` and `PlanParamDialog.tsx`: several parameter changes; `PlanDiff.tsx` reads `gates_would_run` (done: several parameter changes; open: `PlanDiff` reading real `gates_would_run`)
- [x] `components/GenerationHistory.tsx` and `EventsSidebar.tsx`: `param_change` lists, `ParamsChanged`, `GateStateChanged`
- [x] `crates/web/src/actor_activity.rs` and `event_broker.rs` tests: new event names
- [ ] Frontend tests listed against these files (`AppDetail.test.tsx`, `AppDetail.resources.test.tsx`, `AppDetail.fixtures.ts`, `GenerationHistory.test.tsx`, `DefinitionSection.test.tsx`, `PlanDiff.test.tsx`, `EditScript.test.tsx`) (done for the parameter and install files; open for gates)

### Images

- [x] `runtime/probe.rs`: `HandlerKind::Gate` in place of `Install`, `Start` and `ParamChange`; probe gate bodies with `last` as a run would build it; `oi/handler/images.rs` key format for the handler map

### Tests

- [x] Rewrite the BSL tests that use the removed handlers (`crates/core/src/tests/param.rs`, `barrier.rs`, `action.rs`, `runtime.rs`, `app.rs`, `action_call.rs`) onto actions and gates, deleting those that only covered `on_change`, `on_start` or `on_install`
- [x] New BSL tests for `Gate`: builder errors, key equality, cycles and self-waits, `last` at the last success
- [x] Gate runtime tests that sever in-memory state between the record write and its read: hold across a restart, pause across a restart, back-off across a restart, unreadable record leaving the gate undetermined
- [x] Reconciler tests: held selection stays down whatever the scale, waiting workload keeps its stale instances, check Job never in the desired state, spec hash moves with a direct gate key only
- [ ] OI tests: multi-parameter set and update refusals, events and history; `/apps/install`; `/apps/gates/run` refusals; changes accepted while an operation runs
- [ ] Draft the card's test cases (`.workhorse/test-cases/h6/overview.md`) from the gate, gate-run and parameter-change specs

### Definitions and docs

- [ ] `apps/postgres.seed.rhai`: install and `on_change` handlers to gates (major upgrade keyed on the major, password rotation keyed on the password, rendered config keyed on its digest)
- [ ] `apps/tamanu-central.seed.rhai` and `apps/tamanu-facility.seed.rhai`: install and version handlers to a migration gate with the parts in its exclusive selection and after it; the config handlers to environment or a config gate
- [ ] `docs/bsl-scripting.md`: gates, `after`, `last`; drop `on_change`, `on_start`, `on_install` and the install-parameter notes
- [ ] `docs/runtime-overview.md` initiating-events table; `docs/deploying.md` and `docs/dev-vm.md` CLI examples; `docs/failure-modes.md` if gates add a rule
- [ ] Code citations for the removed sections replaced with the new anchors; `etc/ci/check-spec-citations.sh` clean
