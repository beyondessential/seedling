# Beset Runtime Overview

Beset is a runtime for single-node application orchestration.
A BSL script declares an application's resources (containers, services, ingress, volumes) and defines imperative action closures that control how those resources are brought up, upgraded, and managed.
The runtime takes that declaration, observes the real world, and continuously reconciles the two.

## Core Loop

The runtime operates as a reconciliation loop, conceptually similar to a Kubernetes operator but scoped to a single Linux host.
The loop runs continuously:

1. **Observe**: gather the current state of the world (containers, networks, ingress, volumes, sockets).
2. **Record**: append observations to the world observation history.
3. **Derive**: compute the lifecycle state of each resource from the observation history.
4. **Advance**: if a lifecycle operation is in progress and waiting on a barrier, check whether the derived states satisfy it. If so, resume the action closure, which may mutate the desired state further.
5. **Diff**: compare the desired state against the derived state. Produce a set of intended operations.
6. **Evaluate**: for each intended operation, consult the autonomous operations log. Has this been attempted before? How many times recently? Should the runtime back off, or declare a fault?
7. **Act**: execute the operations that pass evaluation.
8. **Record**: append what was done (and why) to the autonomous operations log.

Steps 1–3 update the runtime's model of the world.
Step 4 advances scripted orchestration.
Steps 5–8 change the world.

## Three Histories

The runtime maintains three distinct categories of persistent records:

### World Observation History

A timeline of observations per resource instance.
Each entry is a timestamped, structured record of something the runtime saw: a container's status, an exit event, a health check result, an ingress responding or not.
These are facts, not decisions.

State derivation uses this history to determine lifecycle states.
Without history, you cannot distinguish "not started yet" from "just exited."
Without timestamps, you cannot implement stabilisation windows (e.g. health checks must pass for N seconds before declaring Ready).

### Autonomous Operations Log

A record of operations the reconciler performed on its own initiative, without any action closure directing it.
Each entry records what was done, when, and the provenance: the observation(s) that triggered it, the rule that applied, and the resource(s) affected.

Examples:
- A container exited and `OnTerminate=Recreate`, so a replacement was started.
- Scale requires 2 replicas but only 1 was observed running, so another was started.
- Caddy became unreachable, so its entire configuration was rebuilt.
- A container has crash-looped 5 times in 60 seconds, so the runtime is backing off.

This log enables:
- **Auditability**: operators can review what the runtime did autonomously.
- **Rate limiting and backoff**: the runtime can detect repeated failures and avoid tight restart loops.
- **Fault detection**: patterns like crash-looping or persistent convergence failures are derived from this log, and result in faults filed for external intervention.

### Action Execution Log

A record of progress through a lifecycle operation's action closure.
Each entry records which `rt.start()`, `rt.stop()`, or `rt.reconcile()` call was made, and which barrier was reached and whether it has been satisfied.

This log enables replay: if the runtime restarts mid-operation, it re-executes the closure from the top, and completed calls are idempotent while satisfied barriers return immediately, effectively fast-forwarding to the point where execution was interrupted.

## Desired State

The desired state is not a single static declaration.
It is a function of two inputs:

1. **The AppDef**: the resource graph produced by evaluating the BSL script (services, deployments, jobs, volumes, ingress, their properties and relationships).
2. **The current lifecycle operation's progress**: which resources have been `rt.start()`ed, `rt.stop()`ed, or `rt.reconcile()`d so far by the action closure.

When no lifecycle operation is active, the desired state is the full AppDef as declared by the script, and the reconciler maintains it autonomously (restarting crashed containers, maintaining scale, etc.).

When a lifecycle operation is in progress, the action closure progressively builds up the desired state through `rt.*` calls.
The reconciler only manages resources that the operation has handed over so far.

## Lifecycle Operations

A lifecycle operation is the top-level unit of scripted orchestration.
There is at most one in progress at any time.

Lifecycle operations are initiated by external events:

| Event | Operation |
|---|---|
| First boot (no prior state) | `install` action |
| Normal boot (prior state, no interrupted operation) | `start` action |
| Version change | `upgrade` action |
| Operator request | named action |

If a lifecycle operation is in progress when a new initiating event arrives, the new event is rejected.
An upgrade during an install is meaningless; an operator action during an upgrade must wait.

### Action Composition

Action closures can invoke other actions.
For example, `on_upgrade` commonly calls the `start` action as a subroutine.
This is not concurrent execution: the called action runs inline within the calling operation, and its barriers are barriers of the overall operation.

Cycle detection prevents an action from invoking itself (directly or transitively).

### Shells

Shell actions are not lifecycle operations.
They are interactive sessions that can run concurrently with a lifecycle operation and with each other.
A shell creates temporary (dynamic) resources that are cleaned up when the session ends.

## Action Closure Suspension and Replay

When an action closure calls a barrier method (`.scheduled()`, `.running()`, `.ready()`, `.terminated()`), execution appears to block from the script's perspective.
What actually happens:

1. The closure runs until it hits a barrier.
2. The runtime records the barrier condition (which resources must reach which state) and the accumulated desired-state mutations to the action execution log.
3. The closure is suspended.
4. The reconciler continues its loop, working to converge the world toward the desired state.
5. When the world observation history shows the barrier condition is met, the runtime resumes the closure.
6. Repeat until the closure completes.

### Replay Across Restarts

If the runtime process restarts (cleanly or due to a crash, including full node power loss), and a lifecycle operation was in progress:

1. The BSL script is re-evaluated to reconstruct the AppDef.
2. The action execution log is read from persistent storage.
3. The action closure is re-executed from the top.
4. `rt.start()`, `rt.stop()`, and `rt.reconcile()` calls that are already recorded in the log are idempotent: they update the desired state but don't duplicate work.
5. Barrier calls whose conditions are already satisfied (according to the current world observation history) return immediately.
6. Execution fast-forwards to the first unsatisfied barrier, where it suspends normally.

This works because BSL closures have no side effects beyond `rt.*` calls: no filesystem access, no network calls, no randomness.
Re-execution is deterministic given the same AppDef and parameters.

## Fault Handling

When the reconciler determines that convergence is impossible or that a persistent failure pattern exists, it files a fault.
Faults are not handled by BSL scripts.
They are surfaced to human or agentic operators through the operator interface (defined in a separate spec).

Examples of faults:
- A barrier deadline expires: the action closure expected a resource to reach a state within N seconds, and it didn't.
- Crash-looping: a container repeatedly exits shortly after starting, and backoff has been exhausted.
- Permanent divergence: a resource cannot be created (e.g. image doesn't exist, port is occupied by an external process).

## Resource Identity

Each BSL resource maps to one or more concrete system primitives.
The runtime assigns stable identities to these primitives so they can be tracked across restarts and reconciliation ticks.

Scaled resources (e.g. a Deployment with `scale(2)`) produce multiple instances of the underlying primitives.
Each instance has a stable ordinal or identifier so the runtime can track "replica 0" vs "replica 1" independently.

The exact mapping from BSL resources to system primitives (containers, networks, configuration entries, filesystem paths) is defined in the runtime spec.