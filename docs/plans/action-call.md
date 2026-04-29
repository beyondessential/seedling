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

## Spec changes (`docs/spec/language.md`)

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
  - Sub-action calls do not get their own history entry; their
    `rt.*` calls extend the outer operation's action log via the
    existing `call_index` counter.
  - Optional (recommended): emit a `SubActionInvoked` log entry
    capturing `(action_name, params)` so operators can see the
    nested call in `apps history`. This is purely informational and
    does not gate replay.

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

## Migration

There are no current users of `rt.start(action_handle)` (silent
no-op), and no observed users of `app.select(#{ types:
[ResourceType.Action] })`. Removing both is safe. We add the change to
the changelog so anyone with private scripts using either knows.

## Open questions

- **Calling `start` or `install`?** `app.action("start")` would
  resolve since `on_start` registers under name `"start"` in
  `def.actions`. Allowing scripts to call the start action from inside
  another action is probably fine — but `app.action("install")` would
  not resolve because install lives in a separate slot. Recommend:
  document that only names registered with `on_action` / `on_start`
  are reachable; install is not.
- **History visibility.** Should sub-action calls show up in `apps
  history` as a tree, or only as the outer operation? Recommend:
  outer-only by default; emit a `SubActionInvoked` informational entry
  so the detail is recoverable but the top-level history stays flat.
- **Recursion bound.** Should we cap call depth to detect runaway
  recursion? Recommend a soft cap (e.g. 32) that throws a clear error;
  cheap to add.
- **Returning a value.** Should `.call()` return anything? Recommend:
  return unit. Actions are side-effect commands. Failure is signalled
  by exceptions.

## Sequencing

1. Spec edits (`docs/spec/language.md`) and bsl-scripting guide
   updates land first so the design is visible.
2. Implement the App-as-Collection cleanup (drop actions from
   `AppBag`, drop Collection methods from `Action`,
   `extract_instances` defensive error). This is mechanical and
   self-contained.
3. Extract param validation into a shared module.
4. Wire the operation ctx to retain captured actions; expose
   `call_action` on the runtime; register `app.action()` and
   `Action.call()`.
5. Tests.
6. Update `docs/bsl-scripting.md` with the real example (replacing
   the placeholder section currently in the guide).
7. `unplan` commit removing this file.
