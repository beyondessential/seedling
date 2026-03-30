# Runtime Foundation Implementation Plan

This plan covers the foundational runtime data structures and logic that can be
implemented and fully tested without any system calls (no podman, no systemd,
no network proxy). These pieces form the substrate on which the barrier/suspension
engine (item 7, tracked separately) will be built.

Items are ordered by dependency: each item only depends on items before it.

---

## 1. ResourceInstance — stable identity

**Spec**: `r[identity.stable]`, `r[identity.components]`, `r[identity.scaled]`,
`r[identity.anonymous]`

**Where**: new `src/runtime/identity.rs`

A `ResourceInstance` uniquely identifies one concrete instance of a resource
across reconciliation ticks and restarts. Distinct from `ResourceId` in `defs`,
which is a BSL-level concept (type + name). A `ResourceInstance` adds the
application name and, for scaled resources, an ordinal.

```rust
pub struct ResourceInstance {
    pub app: Arc<String>,
    pub kind: ResourceKind,
    pub name: ResourceName,  // None for anonymous resources
    pub ordinal: u32,        // 0 for non-scaled resources
}
```

Anonymous resources receive a runtime-assigned name (e.g. a UUID or stable hash
of their definition) that is stored in the database so it survives restarts.

**Tests**: roundtrip serialisation; ordinal uniqueness; anonymous resources
never collide with named ones.

---

## 2. LifecycleState — the state enum and transition rules

**Spec**: `r[lifecycle.states]`, `r[lifecycle.transitions]`

**Where**: new `src/runtime/lifecycle.rs`

```rust
pub enum LifecycleState {
    Pending,
    Scheduled,
    Running,
    Ready,
    Terminating,
    Terminated,
    Unscheduled,
}
```

Also define `LifecycleState::can_transition_to(&self, next: LifecycleState) -> bool`
encoding the valid transitions (including the skip cases like Running → Terminated).
This is used by the derivation logic (item 4) to validate that the observation
history doesn't produce impossible sequences, and will be used in tests.

**Tests**: all valid transitions accepted; invalid transitions rejected (e.g.
Unscheduled → Running); skip transitions allowed (Running → Terminated).

---

## 3. Persistent history stores — SQLite schema and Rust types

**Spec**: `r[history.persistence]`, `r[history.storage]`, `r[history.world]`,
`r[history.world.entries]`, `r[history.operations]`,
`r[history.operations.entries]`, `r[history.operations.provenance]`,
`r[history.action-log]`, `r[history.action-log.entries]`,
`r[history.action-log.replay]`

**Where**: new `src/runtime/db.rs` (schema + migrations), new
`src/runtime/history.rs` (typed query/insert API over the DB)

**Dependency**: `rusqlite` (or `sqlx` with the sqlite feature). Prefer
`rusqlite` for simplicity; it is synchronous and has no async overhead, which
suits a reconciliation loop that already controls its own scheduling.

Three tables:

### `world_observations`
```sql
CREATE TABLE world_observations (
    id          INTEGER PRIMARY KEY,
    recorded_at INTEGER NOT NULL,  -- unix timestamp, millisecond precision
    app         TEXT    NOT NULL,
    kind        TEXT    NOT NULL,  -- ResourceKind serialised
    name        TEXT,              -- NULL for anonymous
    ordinal     INTEGER NOT NULL DEFAULT 0,
    obs_kind    TEXT    NOT NULL,  -- e.g. "container_status", "exit_event", "health_check"
    payload     TEXT    NOT NULL   -- JSON
);
```

### `autonomous_operations`
```sql
CREATE TABLE autonomous_operations (
    id            INTEGER PRIMARY KEY,
    recorded_at   INTEGER NOT NULL,
    app           TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    name          TEXT,
    ordinal       INTEGER NOT NULL DEFAULT 0,
    operation     TEXT    NOT NULL,  -- e.g. "start_container", "rebuild_proxy_config"
    provenance    TEXT    NOT NULL,  -- JSON: { observations: [...ids], rule: "..." }
    outcome       TEXT,              -- NULL while in-flight; "ok" or "err:<msg>"
    completed_at  INTEGER
);
```

### `action_log`
```sql
CREATE TABLE action_log (
    id              INTEGER PRIMARY KEY,
    recorded_at     INTEGER NOT NULL,
    operation_id    TEXT    NOT NULL,  -- UUID identifying the lifecycle operation
    app             TEXT    NOT NULL,
    action_name     TEXT    NOT NULL,
    call_kind       TEXT    NOT NULL,  -- "start", "stop", "reconcile"
    resources       TEXT    NOT NULL,  -- JSON array of ResourceInstance
    barrier_state   TEXT,              -- NULL if no barrier; else the LifecycleState waited on
    barrier_deadline INTEGER,          -- seconds
    barrier_satisfied INTEGER          -- NULL or unix timestamp when satisfied
);
```

The `db.rs` module handles connection setup and schema migrations (a simple
version table + sequential migration functions). The `history.rs` module
exposes typed insert and query functions over these tables; it never returns
raw SQL rows to callers.

**Tests**: insert and retrieve round-trips for all three tables; migration is
idempotent (running it twice produces the same schema); queries by resource
identity and time range return correct subsets.

---

## 4. Lifecycle derivation — observation history → LifecycleState

**Spec**: `r[lifecycle.derivation]`, `r[lifecycle.container]`,
`r[lifecycle.service]`, `r[lifecycle.ingress]`, `r[lifecycle.volume]`

**Where**: `src/runtime/lifecycle.rs` (extends item 2)

A function:
```rust
pub fn derive_state(
    resource: &ResourceInstance,
    observations: &[WorldObservation],
) -> LifecycleState
```

Takes the full observation history for one resource instance (already fetched
from the DB, ordered by `recorded_at` ascending) and returns the current
lifecycle state. The derivation is deterministic: the same observation sequence
always produces the same state.

The logic is per-resource-kind (using the semantics from `r[lifecycle.container]`
etc.) and processes observations in order, updating a state variable. This is
intentionally a pure function with no DB access; the caller fetches the
observations and passes them in.

Also define a variant:
```rust
pub fn derive_state_with_transition_time(
    resource: &ResourceInstance,
    observations: &[WorldObservation],
) -> (LifecycleState, Option<SystemTime>)
```
returning both the state and when the last transition occurred, for use in
deadline and backoff calculations.

**Tests**: container that hasn't been observed → Pending; container with
"created" observation → Scheduled; container with "running" → Running; container
with "running" then "health_check_pass" → Ready; container with "running" then
"exited" → Terminated (skip Terminating); container with "running" then
"stop_sent" then "exited" → Terminated (via Terminating). Same suites for
Service, Ingress, Volume.

---

## 5. Desired state computation

**Spec**: `r[desired-state.definition]`, `r[desired-state.steady]`,
`r[desired-state.during-operation]`

**Where**: new `src/runtime/desired.rs`

A data structure representing the desired state:

```rust
pub struct DesiredResource {
    pub instance: ResourceInstance,
    pub desired: LifecycleState,  // practically: Ready or Unscheduled
    pub definition: Resource,     // the BSL resource definition
}

pub struct DesiredState {
    pub resources: Vec<DesiredResource>,
}
```

And a function:
```rust
pub fn compute(
    app_def: &AppDef,
    operation_progress: Option<&OperationProgress>,
) -> DesiredState
```

Where `OperationProgress` is a set of resource instances that the current
lifecycle operation has `rt.start()`ed or `rt.stop()`ed so far (derived from
the action log, item 3). When no operation is in progress, the full AppDef
contributes all static resources at desired state Ready. When an operation is
in progress, only resources that have been explicitly started/stopped by the
closure are included.

**Tests**: no operation → all static resources desired; operation in progress
with no `rt.start()` calls yet → empty desired set; `rt.start()` of one
resource adds it; `rt.stop()` marks it Unscheduled.

---

## 6. Operation scheduler

**Spec**: `r[operation.lifecycle]`, `r[operation.lifecycle.single]`,
`r[operation.lifecycle.single.intra-app]`,
`r[operation.lifecycle.single.inter-app]`, `r[operation.lifecycle.events]`,
`r[operation.lifecycle.param-change]`, `r[operation.lifecycle.completion]`,
`r[operation.composition]`, `r[operation.composition.cycles]`,
`r[history.operations.rate-limiting]`

**Where**: new `src/runtime/scheduler.rs`

A `Scheduler` struct that holds:
- The currently active operation (if any): which app, which action, operation ID
- A queue of pending operations per app (max one per app)
- The action call stack for the current operation (for cycle detection)

```rust
pub enum ScheduleResult {
    Accepted,           // operation started or queued
    Rejected(RejectReason),
}

pub enum RejectReason {
    SameAppOperationInProgress,
    SameAppAlreadyQueued,
}
```

Key methods:
- `request(app, action) -> ScheduleResult` — apply the scheduling rules
- `complete_current() -> Option<QueuedOperation>` — mark current done, dequeue next
- `push_call(action_name) -> Result<(), CycleError>` — push onto call stack, detect cycles
- `pop_call()` — pop when an action closure returns

The scheduler does not execute anything; it only tracks what should run and
enforces the concurrency rules. Actual execution is driven by item 7 (the
barrier engine).

Backoff (from `r[history.operations.rate-limiting]`) lives here as a helper:
```rust
pub fn should_back_off(
    resource: &ResourceInstance,
    operation: &str,
    recent_ops: &[AutonomousOperation],
    now: SystemTime,
) -> Option<Duration>
```
Returns `Some(wait)` if the operation should be deferred, `None` if it should
proceed. Uses exponential backoff capped at a maximum interval, based on the
count and recency of the same operation in the log.

**Tests**: single app, second request rejected; two apps, second queues then
starts after first completes; intra-app second request always rejected even if
different action; cycle detection fires on direct and transitive cycles; backoff
returns increasing durations for repeated failures, resets after a gap.

---

## Notes

- All new code lives under `src/runtime/` as a new module, separate from
  `src/defs/` (which is the BSL language layer).
- The `src/runtime/mod.rs` re-exports the public API used by item 7 and
  eventually the reconciliation loop.
- No async. The reconciliation loop controls its own tick cadence; blocking
  SQLite calls inside a single-threaded loop are fine. If async becomes
  necessary later (e.g. for the operator API), the runtime module can be
  wrapped, but the core logic stays synchronous.
- All types implement `Debug`. History types implement serialisation via
  `serde` + `serde_json` for the JSON payload columns.
- Item 7 (the barrier/suspension engine) will depend on items 1–6 but is
  planned separately.
```

Now let me commit that and start on item 7: