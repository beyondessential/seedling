---
id: SUSP
---

# Action closure suspension and replay

## Reconciliation

<!-- from runtime: Reconciliation -->

> r[barrier.replay.positional]
> The action execution log is positional: a replayed call is matched to the committed entry at its own position in the closure's call sequence, never to any entry elsewhere in the log that happens to look similar.
> "At most once" for a call is therefore per call site: two identical calls at different points in a closure are two calls, and a call whose arguments resolve differently between passes is still the same call.
> A replay whose committed entry does not correspond to the call being made must fail rather than guess.

## Action Execution Log

<!-- from runtime: Persistent History / Action Execution Log -->

> r[rt.signal]
> The runtime persists each [`rt.signal`](rule:l/rt.signal) invocation to the action execution log so the call is not re-delivered when an interrupted operation replays. The persisted entry records the target instances and the signal name. On replay, the runtime treats a signal entry as already delivered and does not re-issue it; this is the at-most-once-across-replays guarantee from `l[rt.signal]`.

> r[rt.write]
> The runtime persists each [`rt.write`](rule:l/rt.write) invocation to the action execution log. The persisted entry records the target volume and the write path; the file contents are not stored in the log. On replay, the runtime treats a write entry at the same call site as already applied and does not re-execute it; this is the at-most-once-across-replays guarantee from `l[rt.write]`.
>
> The runtime resolves the target volume to a host filesystem mountpoint using the same mechanism the actuator uses to apply static `Volume.write` content, then writes under the [confinement every path the runtime owns is held to](rule:r/storage.confinement), so a malicious or buggy path cannot escape the volume's root.

> r[rt.exec]
> The runtime persists each [`rt.exec`](rule:l/rt.exec) invocation to the action execution log. The persisted entry records the target container instance and the exit code. The argv and any options (e.g. env) are not stored in the log; only the outcome is needed for replay correctness. On replay, the runtime treats an exec entry at the same call site as already executed and recovers the exit code from the entry rather than re-running the command; this is the at-most-once-across-replays guarantee from `l[rt.exec]`.
>
> The runtime executes the command inside the target's running container via the container runtime (e.g. `podman exec`), inheriting the container's namespaces, working directory, user, and environment. Stdout and stderr are forwarded to the container-log sink for the target instance. The runtime blocks the action closure until the command exits, then returns control with the exit code attached to the `Executed` value.

## Action Closure Suspension

<!-- from runtime: Action Closure Suspension -->

> r[barrier.suspension]
> When an action closure calls a barrier method (`.scheduled()`, `.running()`, `.ready()`, `.ready_eventually()`, `.terminated()`, `.terminated_eventually()`) on a `Started` value, the closure must be suspended.
> Execution appears to block from the script's perspective.

> r[barrier.condition]
> A barrier condition specifies which resource instances must reach which lifecycle state.
> The barrier is satisfied when all specified resources have reached (or passed) the required state, as determined by the [world observation history](rule:r/history.world.state-derivation).

> r[barrier.deadline]
> Each barrier has an optional deadline.
> When a deadline is set and the barrier condition is not satisfied within it, the barrier must throw an exception within the action closure.
> When the deadline is absent (as it is for `.terminated_eventually()` and `.ready_eventually()`), the barrier waits indefinitely; it resumes only when the condition becomes satisfied or when the operation is [cancelled](rule:r/operation.cancel).
> The condition check takes precedence over the deadline: a barrier whose condition is currently satisfied resumes successfully even if the deadline has elapsed since the original suspension. Otherwise, a short-lived resource that completed during a replay window where the satisfied flag never landed in the action log could be spuriously timed out on the next replay.

> r[barrier.suspension.poll-backoff]
> The cadence at which the runtime re-evaluates a suspended barrier (replaying the action closure up to the suspend point) may vary with how long the barrier has been waiting.
> Short initial cadence keeps quick barriers responsive; longer cadence for protracted waits bounds the aggregate replay cost of multi-hour operations.
> The exact schedule is not prescribed by this spec.

> r[barrier.resume]
> When a barrier condition is satisfied, the runtime must resume the suspended action closure.
> The closure continues from the point immediately after the barrier call.

## Replay

<!-- from runtime: Action Closure Suspension / Replay -->

> r[barrier.replay]
> If the runtime restarts while a lifecycle operation is in progress, the operation must be replayed:
>
> 1. The BSL script is re-evaluated to reconstruct the AppDef.
> 2. The [action execution log](rule:r/history.action-log) is read from persistent storage.
> 3. The action closure is re-executed from the beginning.
> 4. `rt.*` calls that are already recorded in the log are idempotent: they produce the same desired state mutations but do not duplicate real-world operations.
> 5. Barrier calls whose conditions are already satisfied (according to the current world observation history) return immediately.
> 6. Execution fast-forwards to the first unsatisfied barrier, where it suspends normally.

> r[barrier.replay.anchor]
> A barrier is anchored to the log entry of the call that produced the `Started` it was called on.
> Whether the barrier has already been satisfied is decided from that entry alone, never from another entry that happens to record the same resources and the same required state.
> A call's entry holds a record for each barrier asserted on it, under that barrier's position among them, counted in the order the closure asserts them.
> A barrier is identified by that position and not by what it waits for: its resources are not usable for the purpose, since a replica may be added or retired between passes and it is still the same barrier, and neither is its required state, since nothing stops a closure asserting the same state twice on one call.
> A barrier on a group that a [Collection](rule:l/collection.interface) method has narrowed is a barrier on that call like any other, and holds its own record there.
> Resources and required state do not identify a barrier: distinct calls within one closure can resolve to the same resource set, and [`rt.stop`](rule:r/barrier.replay.rt-stop) records the same terminal state that a later `.terminated()` barrier waits for.
> A barrier whose anchoring entry records no satisfied barrier for its required state is decided against the [world observation history](rule:r/history.world.state-derivation).

> r[barrier.satisfaction.recorded]
> That a barrier's condition was met is recorded against that barrier's own record on its [anchoring entry](rule:r/barrier.replay.anchor), whether it was met on the pass that first reached the barrier or on a later one.
> Each barrier likewise records when its own wait began, and that start stands until the barrier is satisfied: re-recording it on each pass would restart the wait it measures, and the [deadline](rule:r/barrier.deadline) could never expire.
> Recording against the barrier rather than against the call is what lets a `Started` carry several: `s.running()` then `s.ready()` neither take each other's passing for their own nor displace the start the other is timed from.
>
> A barrier still recorded as waiting counts toward the [poll cadence](rule:r/barrier.suspension.poll-backoff) for as long as the record stands.
> A barrier that has passed but was never recorded as passed therefore goes on lengthening the cadence of every barrier after it, so an operation whose first barrier waited a long time re-polls the next one slowly even though it is ready to proceed.

> r[barrier.replay.abandoned]
> Startup cleanup of resources left by a previous run must spare the resources recorded against the operation it is about to replay, so the replay can adopt them.
> Where the replay does not then start, for any reason, those resources must be torn down and their records forgotten before the runtime goes on serving.
>
> A resource spared for a replay that never happens is recorded against an operation no live operation will ever claim, and the next start spares it again on the same reasoning, so nothing reclaims it for as long as the node lives.
> Sparing a resource is therefore a promise, and every path that declines to replay owes it.
>
> Only resources actually torn down may be forgotten: a record the runtime could not read or act on must stay on the books, since dropping it would report a teardown that did not happen.

> r[barrier.replay.determinism]
> BSL action closures must not have side effects beyond `rt.*` calls.
> Re-execution of a closure given the same AppDef and parameters must produce the same sequence of `rt.*` calls up to any given barrier point.

> r[barrier.replay.rt-stop]
> The `rt.stop()` method also blocks (until resources terminate).
> It must participate in the same suspension and replay mechanism as barrier methods.
