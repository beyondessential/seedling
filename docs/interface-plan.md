# Operator Interface Implementation Plan

## Context

The operator interface (OI) is the missing piece that enables faults to be surfaced, actions
to be triggered, params to be changed, and shells to be opened. This plan covers the
implementation in seven phases. Phase 0 is an architectural prerequisite; the remaining
phases can proceed in order once it is done.

The spec is in docs/spec/interface.md. The rule prefix is i[...].

---

## Phase 0 — Multi-app architecture

Currently `main.rs` manages a single hardcoded BSL script. The OI requires multiple apps to
be registered and managed by one running process. This phase is a prerequisite for everything
else.

### Behavioural change: no auto-start

The existing `find_or_create_operation` logic in `main.rs` auto-runs the `start` action on
first boot. Under the new model, every app begins as `NotInstalled` and waits for
`InvokeInstall` via the OI. Remove the auto-start fallback.

### New: `AppEntry` and `AppRegistry`

Create `src/runtime/apps.rs`.

`AppEntry` holds all per-app mutable state:
- `name: String`
- `app: App`
- `ast: Arc<AST>` — the last successfully compiled script
- `scope: Scope<'static>` — the Rhai scope for this app
- `installed: bool` — set to true when an install operation completes successfully

`AppRegistry` holds a `HashMap<String, AppEntry>` and exposes:
- `register(name, script: String) -> Result<(), ScriptError>`
- `deregister(name)`
- `reload(name, script: String) -> Result<(), ScriptError>` — re-evaluates; updates `ast`
  and `app` on success; leaves existing state intact on failure
- `get(name) -> Option<&AppEntry>`
- `list() -> Vec<(String, AppStatus)>`

Persist registered apps to a `registered_apps(name TEXT, script TEXT)` DB table so that
registration survives restarts. Load all registered apps at startup before opening the OI
listener.

### Rhai threading constraint

`Scope` and `AST` are `!Send`. All Rhai evaluation and action execution must remain on a
single thread. Use `tokio::task::spawn_blocking` or `block_in_place` as is already done for
`run_operation`. Each app effectively needs its own "Rhai thread" when running an operation.

Consider: one dedicated OS thread per registered app (spawned with `std::thread::spawn`,
communicating with the async runtime via channels), or a shared Rhai thread pool. The
simplest correct approach is one thread per active operation; idle apps have no thread cost.

### Shared scheduler

The `Scheduler` in `src/runtime/scheduler.rs` enforces the global single-operation
constraint. Wrap it in `Arc<Mutex<Scheduler>>` and share it across all per-app operation
runners and the OI request handler.

### Generalise the reconciler

`Reconciler` currently takes a single `app_name` and `App`. Options:

- One `Reconciler` per registered app, each running its own tick loop — simplest, isolates
  failure well.
- One top-level reconciler that iterates over the registry — more complex, but a single tick
  timer.

Prefer one reconciler per app. The OI handler spawns a new reconciler tokio task on
`RegisterApp` and cancels it (and awaits clean shutdown) on `DeregisterApp`.

### Refactor `main.rs`

After this phase, `main.rs` should:
1. Open the database.
2. Set up system backends (as today).
3. Load registered apps from DB and evaluate their scripts.
4. Start reconciler tasks for each installed app.
5. Open the OI QUIC listener (Phase 1).
6. Block forever (signal handler for clean shutdown).

The single-script CLI argument should be removed.

---

## Phase 1 — QUIC server skeleton

Add `quinn` to `Cargo.toml` via `cargo add quinn` (record the full version).
`rustls` is a transitive dependency of `quinn`; confirm it is accessible directly and at a
version that includes `AlwaysResolvesServerRawPublicKeys` (rustls 0.23+).

### Module layout

Create `src/oi/` with:
- `src/oi.rs` — module declaration
- `src/oi/server.rs` — QUIC listener, connection accept loop, stream dispatch
- `src/oi/handler.rs` — JSON method router and response helpers
- `src/oi/error.rs` — OI error type mapping to `wire.error-codes`

### Listener

Bind a QUIC endpoint on `[::1]` at a configurable port (default TBD; pick something in the
ephemeral range and document it as the default).

Configure the server to use an RFC 7250 raw public key via
`rustls::server::AlwaysResolvesServerRawPublicKeys`. On first startup, generate a key pair
(Ed25519 or ECDSA P-256) and persist it to `<data_dir>/oi.key`. On subsequent startups, load
the existing key. Print the SPKI fingerprint (SHA-256, hex-encoded) to stderr at startup so
operators can pin it in their clients.

Implement a client-side `rustls::client::ServerCertVerifier` that accepts a raw public key
and verifies it matches a configured SPKI fingerprint. For the initial dev/test path, also
provide a verifier that accepts any raw public key without checking the fingerprint, gated
behind an explicit opt-in flag.

Accept connections in a loop. For each connection, spawn a task that reads incoming streams.
Identify stream type:
- Client-initiated bidirectional: route to the JSON method router.
- Other stream types: ignore for now (used in later phases for event feed and shells).

### Method router

Parse each bidi stream as a single JSON `{ method, params }` object (read until the
client half-closes). Dispatch to a handler function by `method` string. Serialize the result
as `{ result }` or `{ error: { code, message } }` and write it, then close the stream.

### Initial endpoints (read-only, to validate the stack)

Implement `ListApps` and `DescribeApp`. These require only read access to the `AppRegistry`
and can be done without any of the write-path work from later phases. Use them to confirm
that the QUIC plumbing, JSON framing, and registry reads are all working end-to-end.

---

## Phase 2 — App management

Implement `RegisterApp`, `DeregisterApp`, and `UpdateApp`.

### `RegisterApp`

1. Validate that the name conforms to `bsl.name` rules.
2. Check the name is not already registered.
3. Evaluate the script content. On failure, return `script_error`.
4. Persist name and script content to `registered_apps` table.
5. Add to in-memory `AppRegistry`.
6. Spawn reconciler task for the new app (it will be in `NotInstalled` so it runs in
   steady-state with nothing desired).
7. Emit `AppRegistered` event (wire up once Phase 7 is done; stub the call here).

### `DeregisterApp`

1. Reject with `operation_in_progress` if the scheduler has an active or queued operation
   for this app.
2. Transition app to `Deregistering` status in the registry.
3. Cancel the reconciler task and wait for it to acknowledge.
4. Actuate a full teardown (equivalent to stopping all resources in the desired state).
5. Remove from DB and in-memory registry.
6. Emit `AppDeregistered`.

### `UpdateApp`

1. Compile and evaluate the provided script content.
2. On failure: file a fault (wire fault table once Phase 6 is done; stub for now), emit
   `FaultFiled`, return `script_error`.
3. On success: update the stored script content in `registered_apps`. If an operation is in
   progress, store the new AST and App as "pending reload" — apply at the next evaluation
   boundary. Otherwise apply immediately, notify the reconciler tick. Emit `AppUpdated`.

---

## Phase 3 — Param management

### DB schema

Add table: `params(app_name TEXT, param_name TEXT, value TEXT, PRIMARY KEY (app_name, param_name))`.

### Load params on script evaluation

After compiling a script, before running it, query `params` for the app and set each value
into the `Scope` before `engine.run_ast_with_scope`. The `param.store` spec rule requires
this to happen on every evaluation, including reload.

### `SetParam`

1. Validate the app exists and is not `Deregistering`.
2. Upsert into `params` table.
3. Re-evaluate the script with the new param value in scope (same as a reload, but triggered
   by a param change rather than a file change).
4. If the param has an `on_change` handler in the new AppDef, schedule it as a lifecycle
   operation via the scheduler. Return `accepted` or `queued` accordingly.
5. If no `on_change` handler, the new value takes effect on the next script evaluation; return
   `accepted`.

Note: `SetParam` is rejected with `not_installed` while the app is `NotInstalled`.

---

## Phase 4 — Action invocation

### `NotInstalled` gate

Add a helper that returns `not_installed` for any action invocation method when
`app.installed == false`, except `InvokeInstall`.

### `InvokeAction`

1. Look up the action by name in the AppDef. Return `not_found` if absent, or if the name
   belongs to a shell action.
2. Submit to the scheduler. Map `ScheduleResult` to `accepted`/`queued`/`rejected`.
3. If accepted: spawn the operation runner on the app's Rhai thread. Emit `OperationStarted`.
4. Wire the operation runner's completion to emit `OperationCompleted` or `OperationFailed`.

### `InvokeInstall`

1. Reject with `already_installed` if `app.installed == true`.
2. Look up install requirements schema from AppDef (`action.install.requirements`).
3. Validate the submitted requirements map:
   - For each required field with no `default_value`: value must be present and non-empty.
   - For `kind: "email"`: apply basic format validation.
   - For `kind: "password"`: apply strength check (use `zxcvbn` crate or equivalent).
   - For unknown kind: `on_install()` should already have thrown at script eval time; treat
     as `text` defensively.
   - Collect all errors; return `requirements_invalid` with per-field messages if any.
4. If valid: submit install operation to scheduler. Requirements object is passed as an
   argument to the install action closure and discarded after the operation completes.
5. On install operation completion: set `app.installed = true`, persist an `installed_apps`
   record to the DB.

---

## Phase 5 — Shell sessions

### Session registry

Create `src/oi/shells.rs`. Maintain a `HashMap<SessionId, ShellSession>` behind an
`Arc<Mutex<...>>`. `ShellSession` holds the session ID, app name, shell name, and channel
handles for stdin/stdout/stderr routing.

### `OpenShell`

1. Look up the shell action in the AppDef. Return `not_found` if absent.
2. Allocate a `SessionId` (random UUID).
3. Set up the three-stream model:
   - The bidi stream that carried the `OpenShell` request becomes the session stream.
     After writing the response, read raw bytes from it and forward as stdin.
   - Open two server-initiated unidirectional streams for stdout and stderr. Include their
     stream IDs in the response.
4. Evaluate the shell action closure:
   - Form 1 (returns Job): `rt.start()` the job, wait until `running()`, then call attach
     which routes job stdout/stderr → OI streams and OI stdin → job stdin.
   - Form 2 (explicit `attach` arg): the closure manages setup/teardown; `attach.call(job)`
     triggers the same routing.
5. Register the session in the session registry.
6. When the job terminates: write `{ "exit_code": N }` as the final JSON frame on the
   session stream's server-to-client direction, close all three streams. Emit `ShellExited`.
   Clean up dynamic resources as specified by the runtime spec.

### `ResizeShell`

Look up the session by ID. Forward the new dimensions to the job's PTY via `TIOCSWINSZ`
ioctl (or equivalent). Return `not_found` if session does not exist.

### `PodmanRuntime::exec`

The existing `todo!()` stub in `PodmanRuntime::exec` must be implemented here. This is what
backs the shell attach. It must:
- Execute the container's process with a PTY allocated.
- Return handles for stdin, stdout, and stderr that the OI layer can route.

---

## Phase 6 — Fault surface

### DB schema

Add table:
```
faults(
    id          TEXT PRIMARY KEY,
    app         TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_name TEXT,
    instance_id TEXT,
    kind        TEXT NOT NULL,
    timestamp   TEXT NOT NULL,
    description TEXT NOT NULL,
    cleared_at  TEXT
)
```

### Filing faults

The reconciler already detects fault conditions (barrier deadline, crash-loop, permanent
divergence) per `r[fault.detection]`. Add a `file_fault(...)` call at each detection site
that inserts into the `faults` table. Each fault gets a random UUID.

### Auto-clearing

At the end of each reconcile tick, re-evaluate which faults are still applicable. For each
active fault (no `cleared_at`) whose condition no longer holds, set `cleared_at = now()` and
emit `FaultCleared`.

### `ListFaults`

Query `faults` where `cleared_at IS NULL`, optionally filtered by `app`. Return as an array
of fault records.

---

## Phase 7 — Event feed

### Broadcast channel

Add a `tokio::sync::broadcast::Sender<OiEvent>` to the shared server state. Each phase that
emits events (2–6) sends to this channel. Stub the send calls in earlier phases as
`let _ = event_tx.send(...)` so they compile without a subscriber yet.

### `Subscribe`

On receipt, open a server-initiated unidirectional QUIC stream on the connection. Spawn a
task that `recv()`s from a `broadcast::Receiver` cloned from the sender and writes each event
as a newline-terminated JSON object to the stream. Stop when the connection closes or the
stream errors.

### Event types

Implement serialization for all event types defined in `i[event.types]`. Each event is a
JSON object with `type` and `timestamp` fields plus type-specific fields.

### `ResourceStateChanged`

The reconciler's observation loop derives lifecycle states each tick. After deriving states,
compare against the previous tick's states (held in memory per app). For any resource whose
state changed, emit `ResourceStateChanged`. This requires the reconciler to have access to
the event broadcast sender, passed in at construction time.

---

## Cross-cutting notes

- Every new DB table must be created in the existing migration infrastructure. Check how
  `Db::open` applies migrations before adding tables.
- Rhai `!Send` constraint: never attempt to move a `Scope` or `AST` across thread boundaries.
  Keep them on the thread they were created on. Use channels to communicate results back to
  async tasks.
- The existing `Actuator::update` stub (`todo!()`) is not required by this plan; shells use
  `exec`, not `update`. Leave it for a separate plan.
- Run `cargo clippy`, `cargo fmt`, and `tracey query status` after each phase before
  committing.
