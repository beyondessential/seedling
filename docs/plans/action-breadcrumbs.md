# Plan: action breadcrumbs in the app journal

## Status

Shipped (commits 1fd3d4bf, 92b4f210, efa31c3b on top of the
do_stop / check_barrier / Installing-phase fixes):

- [x] `system::breadcrumb` module with `BreadcrumbKind` enum and
      `Breadcrumb::emit`, sending journald records via
      `systemd::journal::send`.
- [x] `do_start` / `do_stop` / `do_warm_certs` / `do_warm_images` /
      `do_signal` / `do_exec` / `do_write` emit on the first fresh
      execution (skipped on replay).
- [x] `Action.invoke` emits a `SubAction` breadcrumb with the
      validated params.
- [x] `pod.rs::start_pod_instance` emits a `UnitCreate` breadcrumb
      identifying the rt.* call.
- [x] Anonymous-resource summary (`description()` else
      `image: <short>; cmd: <head>`) on the rt.start breadcrumb;
      reusable via `system::breadcrumb::anon_summary`.
- [x] Per-replay-pass marker at `run_operation` entry.
- [x] `LogEntry` exposes `rt_call` and `script_pos`; the CLI logs
      renderer marks breadcrumb lines with a `>` prefix.

Outstanding — do NOT unplan until these are done or consciously
dropped:

- [~] Script positions via `NativeCallContext::position()`. **Dropped
      by felix after using the feature live** — the existing trace
      reads clearly without the script:line:col annotations and the
      change would touch every with_fn call site for marginal gain.
      The `SEEDLING_SCRIPT_POS` plumbing on `LogEntry` and the CLI
      renderer stays in place in case it's wanted later.
- [ ] `rt.query` and `rt.restart` breadcrumbs. `rt.query` currently
      reuses `do_start` so its breadcrumb labels as `start` — needs a
      separate code path or an explicit `Query` kind passed in.
      `rt.restart` goes through `restart_gens::bump_restart_gen` and
      doesn't have a breadcrumb hook yet.
- [~] Richer `apps logs` text formatter. **Dropped by felix** — the
      caret-prefixed render is fine in practice.
- [ ] Tests: unit tests on `BreadcrumbKind::message` formatting; an
      integration test that captures emitted breadcrumbs during a
      script run via a stub journald layer (the current
      `systemd::journal::send` path silently no-ops in CI).
- [x] Manual verification on a real install (re-install
      tamanu-central, scan `apps logs tamanu-central`, confirm the
      trace reads top-to-bottom). **Done by felix.**

## Problem

When an install or other lifecycle action fails, an operator currently
has to:

1. Read the fault description (one line, no context).
2. Open `seedling-ctl apps logs <app>` and scroll through the merged
   stream for every container that ran during the operation.
3. Cross-reference timestamps against the action_log table (sudo
   required) to figure out which `rt.*` call produced which container
   output.

The action_log carries the structured trace internally, but the journal
— which `apps logs` reads — does not. Tagging is fine for container
output (`SEEDLING_APP` / `SEEDLING_RESOURCE` / `SEEDLING_INSTANCE` are
all set on the systemd unit's stdout/stderr) but there is no
corresponding entry for *what Seedling did*. The operator can see the
container output but not the surrounding script flow.

A second pain point: when a container starts, journald shows the unit
coming up, but not why. For an anonymous job there is no resource name
to map back to a script location; the only identifier is the
`<app>-anon-job-<8hex>` display name, which gives no hint about what
that container is *for*.

## Goal

Make the journal a complete trace of an operation:

- **Every `rt.*` call** (start, stop, exec, signal, write, warm_certs,
  warm_images, query, action_call/SubAction) emits a journal record
  before the call's side effect runs. The record carries the same
  `SEEDLING_APP` / `SEEDLING_RESOURCE` / `SEEDLING_INSTANCE` tags as
  the container output, so it appears in the per-resource log stream
  for free, and a structured `SEEDLING_RT_CALL` field naming the
  primitive (`start`, `stop`, `exec`, …) plus call-specific extras.
- **Every systemd unit Seedling creates** logs one Seedling-side
  breadcrumb at unit-create time identifying the rt.* call site that
  produced the unit. So the unit's log starts with
  `seedling: Start(api) at script.rhai:42` instead of just container
  output.
- **Anonymous resources** ride alongside an `anon_descr` payload —
  short summary derived from `image()` + `command()` head, plus the
  `description()` value when the script set one. So
  `tamanu-central-anon-job-310f1f78` becomes recognisable as
  "DB provisioning (image=ghcr.io/.../psql, cmd=sleep infinity)".
- **Script position** is included whenever Rhai surfaces it. The
  rt.* methods receive a `NativeCallContext` (we already use this
  for `Action.invoke`); the context's position is the call-site
  position for the rhai-script invocation. We log
  `script.rhai:<line>:<col>` when available.

## Scope guards

- This is a logging-only feature. No spec change to barrier semantics,
  call_index handling, or replay. Existing tests stay green.
- Breadcrumbs are emitted on the *first* fresh execution of a call,
  not on subsequent replays — to avoid flooding the journal with
  duplicates every time a barrier resumes. Replays surface as a
  single `seedling: replay` line per pass (with operation_id and the
  call_index range covered) so the operator can tell pass boundaries
  without a deluge.
- The breadcrumbs go through `tracing` with a custom event subscriber
  that emits `journald` records (we already wire journald via
  `lloggs`); we don't open journald handles ourselves from rt.* code.

## API surface

A new `crate::system::journal::breadcrumb` module:

```rust
pub struct Breadcrumb<'a> {
    pub app: &'a AppName,
    pub resource: Option<&'a ResourceName>,
    pub instance: Option<&'a ResourceInstance>,
    pub script_position: Option<rhai::Position>,
    pub kind: BreadcrumbKind<'a>,
}

pub enum BreadcrumbKind<'a> {
    Start { resources: &'a [ResourceInstance] },
    Stop { resources: &'a [ResourceInstance] },
    Query { resources: &'a [ResourceInstance] },
    Exec { target: &'a ResourceInstance, argv: &'a [String] },
    Signal { target: &'a ResourceInstance, signal: &'a str },
    Write { target: &'a VolumeWriteTarget, path: &'a str, len: usize },
    WarmCerts { hostnames: &'a [String] },
    WarmImages { refs: &'a [String] },
    SubAction { action_name: &'a ActionName, params: &'a JsonMap },
    UnitCreate { unit: &'a str, source_call: &'a str },
}

pub fn emit(b: &Breadcrumb<'_>);
```

`emit` constructs a `tracing::event!` with the structured fields the
journald layer already maps to `SEEDLING_*` keys plus a new
`SEEDLING_RT_CALL`. No direct journald API use; we reuse the existing
infrastructure.

## Wire-up

For each rt.* method in `runtime/barrier/runtime.rs`:

1. Accept `NativeCallContext` as the first registered-fn arg (we
   already do this for `Action.invoke`; the macro `with_fn` accepts
   an extra leading `NativeCallContext` param for context-aware
   functions, which gives us `ctx.position()`).
2. Construct a `Breadcrumb` with the call-specific kind.
3. Call `breadcrumb::emit(&b)` *before* the side effect runs (and
   before the replay-skip check, so the breadcrumb fires once per
   fresh call).
4. For unit creation in `system/actuator/pod.rs`: after
   `start_transient` returns, emit a one-shot
   `BreadcrumbKind::UnitCreate { unit, source_call }` with the
   systemd unit name and a short string describing the originating
   rt.* call (e.g. `Start(api)` or `Start(<anon-descr>)`).

The "first fresh execution only" rule lives in the rt.* methods'
existing replay-detection blocks: we emit the breadcrumb only when
`is_replaying()` is false. Replay-pass boundaries are logged once at
`run_operation` entry/exit.

## Anonymous-resource descriptions

For unit-create breadcrumbs and the per-call breadcrumb, when the
target is anonymous (`instance.name.is_none()`), prefer in order:

1. The `Resource.description()` set in the script.
2. A composed summary: `image=<short-image> cmd=<argv[0]>` where
   `<short-image>` is the path tail (e.g. `nginx:latest` from
   `docker.io/library/nginx:latest`) and `<argv[0]>` is the first
   command word.
3. The display name as last resort.

Anonymous-resource summary derivation is handled in
`runtime/identity.rs` (next to `display_suffix`) so other surfaces
(connected-clients view, faults view) can reuse it.

## Script positions

Rhai's `NativeCallContext::position()` returns the script position of
the function call. We emit it as `SEEDLING_SCRIPT_POS=<line>:<col>`.
Closure indirection (`setup_db.call(rt)`) shows the call site of the
nested `.call(...)`; for the *outer* invocation context we'd need
the call stack from Rhai which it doesn't expose cheaply. Punt:
single position is enough to navigate to the rt.* call.

## Output format

Operators reading `apps logs` see lines like:

```
2026-04-30 12:23:09  app=tamanu  rt=start          api          (script:362:5)
2026-04-30 12:23:09  app=tamanu  rt=sub-action     warm-images  (script:363:5) {}
2026-04-30 12:23:10  app=tamanu  rt=warm-images    refs=2       (script:348:9)
2026-04-30 12:23:11  app=tamanu  unit-create       tamanu-anon-job-310f1f78  source=Start(<DB provisioning>)
2026-04-30 12:23:12  app=tamanu  rt=exec           target=tamanu-anon-job-310f1f78  argv=[psql, ..., setup.sql] (script:330:5)
2026-04-30 12:23:12  app=tamanu  exec output       DO
2026-04-30 12:23:13  app=tamanu  rt=exec           target=tamanu-anon-job-310f1f78  argv=[psql, ..., createdb.sql] (script:331:5)
2026-04-30 12:23:13  app=tamanu  exec output       psql:... ERROR: database "tamanu" already exists
2026-04-30 12:23:14  app=tamanu  rt=stop           target=tamanu-anon-job-310f1f78  (script:332:5)
```

The "exec output" lines are the existing container output. Everything
else is the new breadcrumbs. The `apps logs` text formatter will
recognise `SEEDLING_RT_CALL` and render those lines distinctly (e.g.
indented, italic) so they read as Seedling-side flow rather than
program output.

## Testing

- Unit: each `BreadcrumbKind` formats as expected.
- Integration: a stub-journald layer captures emitted records during
  a script run; we assert the breadcrumb sequence matches the rt.*
  call sequence in the script.
- Manual: re-install tamanu-central, scan `apps logs tamanu-central`,
  confirm the trace reads top-to-bottom as the install closure flows.

## Sequencing

1. Anonymous-resource summary helper and description() plumbing
   (smallest, isolated change).
2. Breadcrumb module with the enum and `emit` fn.
3. Wire breadcrumbs into rt.start / rt.stop / rt.exec / rt.signal /
   rt.write / rt.warm_certs / rt.warm_images / rt.query.
4. Wire SubAction breadcrumb into `Action.invoke` / `call_action`.
5. Wire unit-create breadcrumb into `pod.rs` / `start_transient`.
6. Update `apps logs` text formatter to render breadcrumb fields
   distinctly.
7. Tests + manual verification + `unplan`.
