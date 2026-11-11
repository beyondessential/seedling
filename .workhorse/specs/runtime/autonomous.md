---
id: AUTO
---

# Autonomous reconciliation

Applies to the operations the [runtime](../overview.md#runtime) performs without direction to hold an [app](../overview.md#app) at its [desired state](../overview.md#desired-state).

## Terminology

### Recovery restart

A restart of a container instance that brings it back after it exited unexpectedly, whichever component performs it, including the runtime starting a replacement for an instance whose run ended in failure.

### Deliberate restart

A restart of a container instance that the runtime performs on purpose: a rolling update, a [health-check replacement](#health-check-replacement), or an operator-requested restart.

### Effective scale

The number of instances a Deployment is held at: the scale an operator chose for it, clamped to its declared bounds, or the lower bound where no operator has chosen one.

## Operation provenance

The runtime MUST record each autonomous operation, with its [operation provenance](history.md#operation-provenance), in the [autonomous operations log](../overview.md#autonomous-operations-log) before performing it.

## Restarts

When a container instance in the desired state reaches the `Terminated` lifecycle state and its [`on_exit`](../language/container.md#exit-strategy) or [`on_terminate`](../language/deployment.md#update-and-termination-strategies) strategy calls for it to be recreated, the runtime MUST start a replacement.

### Restart record

The runtime MUST keep a durable record of every restart of a container instance.
Each record MUST identify the instance, the app's generation when the record was made, the time of the restart, and whether the restart was a recovery restart or a deliberate restart.
Where the platform reports the exit status of the run that ended, the record MUST also hold that exit status.

The runtime MUST classify a restart as a recovery restart or a deliberate restart by its cause, whichever component performed it.

> [!NOTE]
> On a platform with a service supervisor the supervisor performs recovery restarts and the runtime deliberate ones, while on a platform without one the runtime performs both.
> A record of the actor would mean different things on different platforms.

The runtime MUST record a restart even when the instance restarted and returned to running between two observations, so that the number recorded does not depend on how often the runtime observes.

### Crash-loop detection

When the number of recovery restarts recorded for an instance within the restart window reaches the restart threshold, the runtime MUST file a [`crash_loop` fault](faults.md#workload-faults) against the instance.
The runtime MUST NOT count deliberate restarts towards the threshold.

> [!NOTE]
> Counting deliberate restarts would make every rolling update read as a burst of crashes.
> The rate catches a container that crashes a few times a day indefinitely without ever exhausting a supervisor's start limit.

The runtime MUST let an operator read and set the threshold and the window through the [restart settings](../interface/faults.md#crash-loop-settings), and MUST apply a change without restarting.

| setting | default |
| --- | --- |
| threshold | 5 recovery restarts |
| window | 1800 seconds |

### Restart pacing

Where a supervisor performs recovery restarts, the runtime MUST configure it to pace them so that a crash-looping container does not exhaust the supervisor's start limit before the runtime can detect the loop.
The runtime SHOULD configure the supervisor to wait 5 seconds between attempts, and to give up after 10 starts within 600 seconds.

While the supervisor is bringing an instance up, whether for its first start or in the pause before an automatic restart, the runtime MUST NOT tear the instance down or start it again.
The runtime MUST distinguish a unit the supervisor reports as starting from one it reports as started.
The runtime MUST treat a unit the supervisor reports as started, with no running container, as failed.
Unless the instance has an active `crash_loop` fault, the runtime MUST tear down a failed unit of an instance the desired state holds running, so that a later iteration starts the instance afresh.
The runtime MUST NOT file a [`container_start_failed`](faults.md#workload-faults) fault for a unit the supervisor is bringing up.
The runtime MUST NOT hold back from replacing a unit whose specification no longer matches the app's definition.

> [!NOTE]
> Replacing a unit while the supervisor paces it resets the supervisor's restart accounting, so the start limit is never reached.
> A container that fails shortly after each start spends most of each cycle being paced, so a runtime that acts during that window replaces the unit on nearly every iteration.

When the supervisor has given up restarting an instance because it reached its start limit, the runtime MUST file a [`crash_loop` fault](faults.md#workload-faults) against the instance even if the recorded rate is below the threshold.

While an instance has an active `crash_loop` fault, whichever condition filed it, the runtime MUST prevent every automatic restart of the instance, whether the runtime or a supervisor would perform it.
The runtime MUST still tear the instance down when it leaves the desired state.

## Job completion

The runtime MUST NOT restart a Job instance that has terminated of its own accord.
It MUST clean up the instance's remaining container and unit state, and MUST NOT start a replacement, both during a lifecycle operation and in steady state.

A Job instance that the runtime started on an earlier iteration and that a successful observation now shows to be gone has terminated of its own accord, even if it was never observed running.

> [!NOTE]
> A short-lived Job can run, exit and be removed between two observations.
> Starting it again would repeat it indefinitely.

The runtime MUST remember which Job instances have completed for as long as it runs.
If a Job instance it remembers as completed is observed running again, the runtime MUST stop it.

## Scale

When the number of running instances of a Deployment differs from its effective scale, the runtime MUST start or stop instances to reach the effective scale.

The runtime MUST keep a durable record of the scale an operator chooses for each Deployment the operator has scaled.

When an action names a Deployment, the runtime MUST resolve it to the instances its effective scale calls for, the same set reconciliation holds it at.

> [!NOTE]
> An `rt.stop` that reached only the lower bound's worth of instances of a scaled Deployment would report the workload stopped while the rest went on serving.

### Unreadable scale

If the runtime cannot read the record of operator-chosen scale, it MUST NOT substitute a declared bound.
It MUST NOT actuate in that iteration any app whose effective scale it could not read, so that the instances already running keep running.
It MUST hold a [`scaling_decisions_unreadable`](faults.md#host-faults) fault for as long as the record cannot be read.

The runtime MUST throw from an `rt.*` call that resolves a named Deployment whose effective scale it cannot read.
The runtime MUST report a Deployment's scale as unavailable to an operator who asks for it while it cannot be read.

> [!NOTE]
> The lower bound is the smallest answer the question could have.
> Converging a Deployment scaled above its floor to that bound tears down instances on the strength of an unanswered question, and rebuilds them once the record can be read.

### Clamping

When an app's definition is re-evaluated and a Deployment's declared bounds change, the runtime MUST clamp the stored scale for that Deployment to the new bounds, raising it to a new lower bound or lowering it to a new upper bound.
The runtime MUST delete the stored scale of a Deployment the definition no longer declares.
The runtime MUST clamp before reconciliation acts on the re-evaluated definition.

## Health-check replacement

When an instance of a Deployment whose healthcheck declares [`on_failure: "replace"`](../language/deployment.md#failure-response) is observed unhealthy, the runtime MUST:

1. Start one instance beyond the Deployment's effective scale as a replacement.
2. Keep the unhealthy instance running, in the [routing pool in degraded mode](lifecycle.md#routing-pool), while the replacement starts.
3. Once the replacement is observed healthy, retire the unhealthy instance as a [scale](#scale) reduction would.

The runtime MUST NOT start a replacement for an instance of a Deployment that declares `on_failure: "monitor"`.

### Replacement guard

The runtime MUST require a fresh instance brought up by a [rolling update](update-strategies.md#rolling-updates) or by a health-check replacement to become healthy within its grace window of `start_period + retries × interval`, from its declared healthcheck.
If it does not, the runtime MUST stop it, file a [`health_check_replace_failed` fault](faults.md#workload-faults), and start no further fresh instances of the Deployment, by rolling update or by replacement, while the fault is active.
The runtime MUST keep the instance it was to replace running in degraded mode.

The runtime MUST apply the guard to every Deployment that declares a healthcheck, whatever its `on_failure` policy.

> [!NOTE]
> `on_failure` decides which Deployments are replaced, not which are watched: a rolling update of a `monitor` Deployment brings up a fresh instance like any other.
> Without the guard, a workload that cannot become healthy churns through replacement instances indefinitely.

## Ingress and network repair

When an ingress's configuration in the proxy does not match its desired configuration, the runtime MUST update or rebuild the configuration.

When a Service's network configuration is missing or wrong, the runtime MUST recreate or repair it.
