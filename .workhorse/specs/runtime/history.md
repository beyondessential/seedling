---
id: HIST
---

# Persistent history

## Reconciliation

<!-- from runtime: Reconciliation -->

> r[history.persist.partial-update]
> A writer that updates a row shared with another writer must not reset columns it does not own.
> This holds as columns are added: a write that names its own columns explicitly must continue to leave the rest untouched when the row gains a new one.

## Persistent History

<!-- from runtime: Persistent History -->

> r[history.persistence]
> All history records must be stored durably and must survive runtime restarts, including unexpected termination and node power loss.

> r[history.storage]
> The storage mechanism must support transactional writes and efficient queries by resource identity and time range.

## World Observation History

<!-- from runtime: Persistent History / World Observation History -->

> r[history.world]
> The world observation history is a timeline of observations per resource instance.
> Each entry is a timestamped, structured record of something the runtime observed about the real world.

> r[history.world.entries]
> Each observation entry must contain:
>
> - A timestamp.
> - The resource identity (type, name, and instance identifier if scaled).
> - The observation kind (e.g. container status, exit event, health check result, network reachability).
> - The observed value or payload.

> r[history.world.source]
> Observations come from the runtime's interaction with the underlying system.
> The runtime must not fabricate observations; every entry must correspond to a real check or event.

> r[history.world.state-derivation]
> The runtime must be able to derive a resource's current [lifecycle state](rule:r/lifecycle.states) and the time of its last state transition from the world observation history alone.

> r[history.world.ordering]
> When multiple observations are emitted within the same tick they may share a timestamp. Queries that return observations for derivation must use a stable secondary ordering — insertion order — so that consumers see them in the order they were emitted, not in storage-engine-defined order.
> Without this, lifecycle derivation becomes non-deterministic for any resource that produces multiple observations per tick (e.g. a container that observes both `stop_sent` and `container_removed` in the same reconciliation pass).

## Autonomous Operations Log

<!-- from runtime: Persistent History / Autonomous Operations Log -->

> r[history.operations]
> The autonomous operations log records operations the reconciler performed without direction from an action closure.

> r[history.operations.entries]
> Each autonomous operation entry must contain:
>
> - A timestamp.
> - The resource identity affected.
> - The operation performed (e.g. start container, rebuild ingress config).
> - The provenance.
> - The outcome (success or failure, and details if failure).

> r[history.operations.provenance]
> Every autonomous operation must record its provenance: the specific observation(s) that triggered it and the rule that applied.
>
> Examples of provenance:
> - "Container exited (observation at T), OnTerminate policy is Recreate."
> - "Observed 1 running instance, scale requires 2."
> - "Ingress controller unreachable (observation at T), rebuilding configuration."

> r[history.operations.interrupted]
> An entry whose operation was still in progress when the runtime stopped is completed when the runtime next starts, before the reconciler performs any operation, with an outcome recording that it was interrupted.
> An interrupted operation's real outcome is unknown, so the entry records it as neither a success nor a failure.

> r[history.operations.rate-limiting]
> The runtime must use the autonomous operations log to detect repeated operations on the same resource and apply backoff.
> The backoff strategy must prevent tight loops (e.g. a container that exits immediately after starting must not be restarted indefinitely at full speed).

## Action Execution Log

<!-- from runtime: Persistent History / Action Execution Log -->

> r[history.action-log]
> The action execution log records the progress of a [lifecycle operation](rule:r/operation.lifecycle) through its action closure.

> r[history.action-log.entries]
> Each action execution log entry must contain:
>
> - A timestamp.
> - The lifecycle operation identity.
> - The `rt.*` call that was made (e.g. start, stop, warm_certs), and on which resources.
> - The barrier condition, if a barrier was reached (which resources, which state, what deadline).
> - Whether the barrier has been satisfied.
>
> The runtime must additionally emit a `SubActionInvoked` entry immediately before each [`Action.invoke()`](rule:l/action.call) runs the called closure. The entry records the called action's name and the validated params map. On replay, the entry is treated as already-emitted: the runtime re-enters the FnPtr but recovers the params from the log rather than re-validating, so a schema change between operation start and replay does not desync the call. The `SubActionInvoked` entries are exposed in `apps history` so operators can inspect the nested call chain.

> r[history.action-log.replay]
> The action execution log must contain enough information to replay an interrupted lifecycle operation from the beginning and fast-forward to the interruption point.
