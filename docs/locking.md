# Lock hierarchy and ordering

Seedling daemon uses several shared locks. Acquiring them in the wrong order
causes deadlocks. This document records every lock, the required acquisition
order, and patterns that are forbidden.

## Locks in `OiState`

| Field | Type | Protects |
|---|---|---|
| `registry` | `Arc<RwLock<AppRegistry>>` | In-memory app registry (phases, generations, per-app sub-locks) |
| `db` | `Arc<Mutex<Db>>` | Main SQLite connection shared by OI handlers and the schedule ticker |
| `scheduler` | `Arc<Mutex<Scheduler>>` | In-memory scheduler queue and active operation slot |
| `forwards` | `Arc<Mutex<ForwardRegistry>>` | Port-forward configuration |
| `trusted_keys` | `Arc<RwLock<HashSet<String>>>` | Trusted public keys for authentication |

The reconciler task (`SystemReconciler`) holds its own `self.db`
(`Arc<Mutex<Db>>`), which is a **separate** SQLite connection and a separate
Rust-level mutex. It never contends with `state.db`.

In `daemon/src/main.rs`, `schedule_db = Arc::clone(&db)` — this is the
**same** `Arc<Mutex<Db>>` as `state.db`. The schedule ticker therefore
contends directly with OI handlers for that mutex.

## Required acquisition order

```
registry → db → scheduler
```

Never acquire a lock that is earlier in this chain while already holding one
that is later. For example:

- Holding `db.lock()` then acquiring `registry.read()` — **forbidden**
- Holding `registry.write()` then acquiring `db.lock()` — **forbidden**
- Holding `db.lock()` then acquiring `scheduler.lock()` — allowed
- Holding `registry.read()` then acquiring `db.lock()` — allowed

`trusted_keys` and `forwards` are independent of this chain. They may be
acquired in any order relative to each other, but do not hold them together
with `registry`/`db`/`scheduler` unless you have audited the call graph for
cycles.

## Why this ordering matters: write-preference RwLock

`parking_lot::RwLock` is write-preferring: once a write lock request is
queued, new read lock requests block behind it rather than being granted
immediately. This means:

- A thread holding `registry.read()` is safe only if it never then tries to
  acquire `db.lock()` while another thread holds `db.lock()` and is waiting
  for `registry.read()`.
- The practical effect: if the schedule ticker holds `db.lock()` and then
  calls `registry.read()`, and a writer is pending on `registry`, the ticker
  blocks. Any thread holding `registry.write()` that then tries `db.lock()`,
  which is held by the ticker, completes the cycle.

## Historical deadlock (fixed 2026-04)

The three-party cycle that caused full event-loop starvation:

1. Schedule ticker: holds `db.lock()`, then calls `registry.read()` — blocks
   because a write is pending.
2. `finalize_install` / `invoke_install` immediate path: holds
   `registry.write()`, then calls `db.lock()` — blocks because the ticker
   holds it.
3. tokio worker threads all exhausted in `futex_wait`; QUIC packet handling
   can no longer be scheduled; server appears dead and ignores SIGTERM.

The fix: consistently acquire `registry` first, release it, then acquire `db`.
When both are needed, never hold `registry.write()` across a `db.lock()` call.

Example of the correct pattern (from `finalize_install`):

```rust
// Phase update: write lock, then release.
{
    let mut reg = state.registry.write();
    if let Some(entry) = reg.get_mut(app_name) {
        *entry.phase.lock() = AppPhase::Installed;
    }
}
// Persistence: read lock is fine; db acquired after registry is released.
{
    let reg = state.registry.read();
    if let Some(entry) = reg.get(app_name) {
        let db = state.db.lock();
        AppRegistry::persist_app(&db, entry)?;
    }
}
```

## Async context: avoid holding locks across `.await`

Never hold a `parking_lot` mutex or RwLock across an `.await` point.
`parking_lot` locks are not `Send`-safe across yield points and will block the
tokio worker thread. For any operation that must hold a lock and then do async
work, either:

- Complete the lock-protected work synchronously, drop the guard, then do the
  async work.
- Use `tokio::sync::Mutex` if the lock genuinely needs to span a yield point
  (rare; prefer restructuring).

## Deadlock detection in debug builds

`parking_lot` is compiled with `features = ["deadlock_detection"]`. A
background thread in `daemon/src/main.rs` (gated on `#[cfg(debug_assertions)]`)
calls `parking_lot::deadlock::check_deadlock()` every 5 seconds and logs any
detected cycles at ERROR level with thread IDs and backtraces.
