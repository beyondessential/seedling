# Plan: invoking actions from BSL with params

## Problem

We need a way for BSL scripts to invoke another action from inside an
action body, with proper params validation. Today there is no such
primitive:

- `rt.start(action_handle)` looks like it should work but is a silent
  no-op (`extract_instances` doesn't recognise the Action variant —
  see `crates/core/src/runtime/barrier/runtime.rs:392`).
- `rt.start(app)` deliberately does not run any actions; that is by
  design (otherwise every full app start would re-run every defined
  action).
- The `params` schema declared in `app.on_action(name, fn, #{ params:
  ... })` is currently only consumed by the OI invocation path
  (operator → CLI/Web → `oi/handler/actions.rs:invoke_action`), with no
  script equivalent.
- Code reuse via top-level Rhai closures (`let do_thing = |rt, ...| {
  ... };`) doesn't share param schemas; the caller has to hand-roll
  validation.

## Goal

Introduce a script-level invocation primitive for actions:

- A getter `app.action(name: string) -> Action`, available only in
  dynamic context (inside an action body), throwing in static context
  and throwing if `name` does not match a registered action.
- `app.on_action(...)` continues to return the same `Action` type — so
  scripts can either capture the handle on definition or look it up by
  name later.
- A method `Action.call(params?: object)` that:
  - Throws if invoked outside an action body.
  - Validates `params` against the action's declared schema (apply
    defaults, reject unknown / reserved keys, enforce required fields,
    resolve `kind: "volume"` to a site-volume binding).
  - Invokes the action's closure inline in the current `rt` context,
    propagating any exception.

## Design decision: drop "Action is a Resource"

`8f6c0302` ("make actions behave like resources") implemented
`Collection` on `Action` and made `AppBag` enumerate actions, but the
runtime side (`extract_instances`) was never wired through. The
abstraction is at odds with the explicit design we want: `rt.start(app)`
must keep ignoring actions, and an Action's lifecycle does not match the
resource state machine (`scheduled` / `running` / `ready` /
`terminated`). Action invocation is a function call, not a scheduling
event.

We will simplify to: **an `Action` is an invocable handle, not a
resource**. The plan deletes the Collection wiring on Action rather
than papering over it.

Specifically:

- Remove the `Collection` methods (`one`, `only`, `except`, `select`)
  from `Action`'s `CustomType` impl
  (`crates/core/src/defs/action.rs:74`).
- Remove `Action` entries from `AppBag::ids()` / `fetch()`
  (`crates/core/src/defs/collection/bag.rs:14`).
- `extract_instances` stops needing an Action branch (and we change its
  fall-through to error explicitly rather than silently returning empty
  — protects against future "I passed a thing rt.start can't schedule"
  bugs).
- Drop `Action` from `ResourceType.*` (script-facing). Keep
  `ResourceKind::Action` internally for action-log / history records
  where that kind label is already in use.
- Remove the spec wording added by `8f6c0302` that says "Action
  implements Collection ... treated as an opaque Resource".

This keeps `rt.start(app)` correct (it never ran custom action closures
and now stops pretending it might) and removes the only thing that
forced us to handle Action as a resource downstream.

## API surface (BSL)

```rhai
app.on_action("seed", |rt, p| {
    let migrate = app.action("migrate");
    migrate.call();
    migrate.call(#{ batch_size: "100" });
}, #{
    description: "Re-seed the database",
});

app.on_action("migrate", |rt, p| {
    rt.start(app.job()
        .image(image.call())
        .command(["migrate", "--batch-size", p["batch-size"]])
    ).terminated().ensure_success();
}, #{
    description: "Run pending migrations",
    params: #{
        "batch-size": #{
            kind: "text",
            default_value: "1000",
        },
    },
});
```

Calling `app.action("does-not-exist")` throws. Calling
`app.action("seed")` in the static (top-level) context throws. Calling
`migrate.call(...)` in the static context throws. Calling
`migrate.call(#{ unknown_key: "x" })` throws (unknown key). Calling
`migrate.call(#{})` succeeds because `batch-size` has a default;
calling it without `params` would also use defaults.

## Spec changes

### `docs/spec/language.md`

1. Edit `l[action.type]`: drop the "Action implements Collection /
   opaque Resource" sentence added by `8f6c0302`. Replace with: "Action
   is an opaque handle that can be invoked from a dynamic context."
2. Add `l[action.lookup]`:
   > `app.action(name: string)` returns the `Action` previously
   > registered with that name. It is only valid in dynamic context;
   > calling it from the static context, or with a name that has no
   > registered action, throws.
3. Add `l[action.call]`:
   > `Action.call(params?: object)` invokes the action's closure
   > inline, in the calling context's runtime, with `params` (defaulting
   > to an empty map). The runtime validates `params` against the
   > action's declared schema before invoking the closure: required
   > fields must be present, defaults are applied to absent optional
   > fields, unknown or reserved keys are rejected, and `kind: "volume"`
   > references must resolve to an existing site volume. Validation
   > errors throw before the closure runs. The closure runs with the
   > same `rt` and accumulates into the same operation log as the
   > caller; there is no separate operation_id, no separate history
   > entry. Exceptions thrown by the called closure propagate to the
   > caller. Calling `.call()` outside a dynamic context throws.
4. Edit `l[rt.start]` to add: "Action handles are not valid arguments
   to `rt.start`; pass them through `Action.call()` instead."
5. Edit `l[action.install]`: keep `rt.start(app)` as the install
   default and add a note that this schedules the App's static
   resources but does not invoke any custom action closures (start,
   on_install, etc. are handled by their own dispatch).
6. `col(val)` Collection coercion table (around lines 105–106): drop
   `Action` from the list of values that coerce to a Collection, and
   drop "and actions" from the App-coerces-to-Collection wording. The
   ResourceType enum row that lists `Action` (around line 213) stays —
   `Action` is still a valid name for action-log identity records and
   for `apps history` filtering — but document it as "internal /
   audit-log only", not script-selectable. *Confirm with felix*
   whether to drop it from the script-facing enum entirely; the easier
   path is to leave it but remove all the ways it can reach
   `extract_instances`.

### `docs/spec/runtime.md`

7. Rewrite `r[operation.composition]` (lines 494–497): replace the
   "calling `rt.start()` on a resource of type Action" wording with
   "calling `Action.call(params?)` on an Action handle obtained from
   `app.action(name)` or the return of `app.on_action()`." Keep the
   inline-execution and shared-barriers semantics.
8. `r[operation.composition.cycles]` (lines 499–501) is already
   correct in spirit (cycle detection required); tighten it to say
   the check uses the action *name* against an active call stack and
   throws *before* the closure runs. The error message must name the
   chain.
9. Add `r[operation.composition.params]` next to the above: the
   runtime applies the called action's param schema in the same way
   as operator invocation (defaults, required-fields, reserved-key
   rejection, volume binding resolution). Schema validation
   determinism is required for replay.
10. Update `r[history.action-log.entries]` (lines 303–310) to add a
    `SubActionInvoked` entry kind: emitted before each `.call()` runs,
    capturing `(action_name, validated_params)` and a `call_index`.
    Entry replays as already-invoked: on replay the FnPtr is
    re-entered but params are recovered from the log rather than
    re-validated, so a schema change between operation start and
    replay does not desync.

### `docs/spec/interface.md`

11. No changes required — `i[action.invoke]` and the install path are
    operator-driven and unaffected. (Sanity check: confirm there are
    no statements forbidding what we're now allowing.)

### `docs/spec/web.md`

12. No changes required.

### `docs/runtime-overview.md`

13. Rewrite the "Action Composition" subsection (lines 100–106):
    replace "Action closures can invoke other actions" + the
    `on_upgrade` example (which is the dropped-pre-`aa514239` API)
    with the new `Action.call` flow, the `app.action(name)` getter,
    the no-recursion guarantee, and the `SubActionInvoked` log line.

### `docs/bsl-scripting.md`

14. Replace the "Sharing logic between actions" section (lines
    501–526, added in the previous correction commit) with the real
    `app.action(name).call(params?)` example. Keep the closure pattern
    as a *secondary* hint for code that doesn't care about the action
    metadata (param schema, schedule, history entry); lead with
    `.call()` for the primary "invoke another action" story.

### `docs/backup-app.md`, `docs/resource-context-rules.md`, `docs/networking.md`, `docs/threat-model.md`, `docs/skill/*`

15. No changes required — these docs only ever use `app.on_action(...)`
    as an action-definition primitive; none reference Action handles
    as resources or describe action invocation.

## Implementation outline

Files and roughly the change in each:

- `crates/core/src/defs/action.rs`
  - Drop the Collection methods from `CustomType`.
  - Add `with_fn("call", |this, ...| ...)` for the no-params and
    one-arg (`Map`) overloads.
  - Both overloads delegate to a free function that reads
    thread-local context (see below) and returns
    `Result<(), Box<EvalAltResult>>`.

- `crates/core/src/defs/app/action.rs` (or a new `lookup.rs` in the
  same module)
  - Register `app.action(name)` on the App `TypeBuilder`.
  - Body: assert `is_in_action_closure()`; load `app.def`; check
    `def.actions.contains_key(name)`; return
    `Action::new(name, app_name)` or throw "no such action".

- `crates/core/src/defs/collection/bag.rs`
  - `AppBag::ids()` returns only `def.resources.keys()` — drop the
    action-id chain.
  - `AppBag::fetch()` no longer needs the Action branch.

- `crates/core/src/runtime/barrier/runtime.rs`
  - `extract_instances` fall-through: on an unrecognised value,
    `Err("rt.start: argument is not a resource or collection")`
    instead of returning empty. Cheap defensive change once Actions
    can no longer reach this code path.

- `crates/core/src/runtime/barrier.rs` (operation context)
  - The operation ctx already holds enough to look up the action
    closure via the existing `replay.rs` machinery. Add a
    `BTreeMap<ActionName, FnPtr>` field populated when `replay.rs`
    drains `end_closure_capture()` — keep the captured `actions` map
    alive for the duration of the run instead of dropping it.
  - Expose a `RuntimeInstance::call_action(name, params)` that:
    1. Looks up the FnPtr.
    2. Loads the action's param schema from `app.def.actions[name]`.
    3. Calls a shared validation helper (see next bullet).
    4. Converts the validated `serde_json::Map` back to a `rhai::Map`.
    5. Invokes the FnPtr via `fnptr.call_within_context(...)` with
       `(rt_dyn, params_map)` — same arity as operator-invoked
       actions.
    6. Surfaces the result.

- `crates/core/src/oi/handler/actions.rs`
  - Extract the existing `validate_action_params`,
    `apply_action_param_schema`, `validate_volume_params` into a
    sibling module (e.g. `oi/handler/actions/validate.rs`) so both
    the OI path and the new script path call the same code.
  - The script path passes the live `OiState` (or a narrower trait)
    so volume binding can resolve site volumes the same way.

- Action-log / history
  - Sub-action calls do not get their own operation_id or top-level
    history entry; their `rt.*` calls extend the outer operation's
    action log via the existing `call_index` counter.
  - On every `.call()` we emit a `SubActionInvoked` log entry
    capturing `(action_name, validated_params)` immediately before
    the closure runs. This is recorded with a `call_index` so it
    replays deterministically and so `apps history` can render the
    nested call chain to operators.

- No-recursion enforcement
  - The operation ctx gains an `action_stack: Vec<ActionName>`
    representing the chain of currently-executing actions. The
    operator-invoked action sits at the bottom of the stack; each
    `.call()` pushes the called name on entry and pops it on exit
    (via an RAII guard so a panic / propagated exception still pops).
  - `.call(name)` checks `action_stack.contains(name)` *before*
    pushing; if it does, throw with a message naming the cycle —
    e.g. `"action 'bar' is already on the call stack: start → foo →
    bar"`. This catches direct self-call (`bar` calling `bar`) and
    any indirect cycle (`foo → bar → foo`).
  - The check is on `ActionName`, not closure identity, so renaming
    a closure does not bypass it.

- `extract_instances` defensive change covers any future regression.

## Replay

The `.call()` path runs the FnPtr inline. On replay, the recovered
FnPtr is the same one (closures are captured from a re-run of the
script that produces the same `AppDef`, idempotency-checked against
the stored generation). Sub-action `rt.*` calls continue using the
same `call_index` so the action log replays deterministically.

The only new thing is param validation: this must be deterministic
across replays (it is — same schema, same input). If a future change
makes volume-binding resolution stateful (e.g. binding TTL), we'd need
to record the resolved binding in the log; flag for review when that
ever happens.

## Tests

Unit tests at `crates/core/src/tests/action.rs`:

1. `app.action("missing")` throws "no such action".
2. `app.action("foo")` at top level throws "static context".
3. `Action.call()` at top level throws "static context".
4. Action B calls action A with valid params; A's closure runs, sees
   the params it expects.
5. Missing required param → throws with the field name.
6. Unknown / reserved key → throws.
7. `kind: "volume"` param: passing a valid site-volume name resolves
   the binding; passing an unknown name throws.
8. Default value applied when caller omits an optional field.
9. Exception from the called closure propagates to the caller.
10. Replay test: an operation calling a sub-action with params
    survives a runtime restart at any point in the call.
11. Direct self-call (`bar.call()` from inside `bar`) throws with
    a cycle error.
12. Indirect cycle (`foo` → `bar` → `foo`) throws when the second
    `foo.call()` is attempted; the error names the chain.
13. `app.action("install")` throws "no such action".
14. `app.action("start").call()` succeeds and runs the on_start
    closure.
15. `SubActionInvoked` log entries appear in `apps history` for each
    nested `.call()` and replay deterministically.

## Migration

There are no current users of `rt.start(action_handle)` (silent
no-op), and no observed users of `app.select(#{ types:
[ResourceType.Action] })`. Removing both is safe. We add the change to
the changelog so anyone with private scripts using either knows.

## Resolved decisions

- **Calling `start`**: allowed. `app.action("start")` resolves because
  `on_start` registers under the name `"start"` in `def.actions`.
- **Calling `install`**: not allowed. Install lives in a separate slot
  (`captured.install`, not in `def.actions`), so `app.action("install")`
  naturally throws "no such action" — keep that as the surfaced error.
- **History visibility**: outer-only at the top level; emit a
  `SubActionInvoked` log entry per `.call()` so the nested chain is
  inspectable in `apps history` without polluting the operation list.
- **Recursion**: forbidden, as a hard error. A `.call(name)` is
  rejected (before the closure runs) if `name` is already anywhere
  on the active call stack — direct self-call or indirect cycle.
  See the "No-recursion enforcement" bullet under Implementation
  outline.
- **Return value**: unit. Actions are side-effect commands; failure
  is signalled by exceptions.

## Sequencing

1. Spec edits — `docs/spec/language.md` items 1–6 and
   `docs/spec/runtime.md` items 7–10 — land first so the design is
   visible to anyone reading the spec mid-implementation.
2. App-as-Collection cleanup: drop actions from `AppBag`, drop
   Collection methods from `Action`, change `extract_instances`'s
   fall-through to a clear error. Mechanical and self-contained.
3. Extract param validation from `oi/handler/actions.rs` into a
   shared module so the OI path and the new script path use the same
   code.
4. Wire the operation ctx to retain captured actions and an
   `action_stack`; expose `RuntimeInstance::call_action`; register
   `app.action()` and `Action.call()`; emit `SubActionInvoked` log
   entries.
5. Tests (unit + integration + replay).
6. Update `docs/bsl-scripting.md` and `docs/runtime-overview.md` to
   the new example and narrative (items 13, 14).
7. `unplan` commit removing this file.
