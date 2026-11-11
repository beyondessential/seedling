---
id: AUTO
---

# Autonomous reconciliation

## Autonomous Reconciliation

<!-- from runtime: Autonomous Reconciliation -->

> r[autonomous.restart]
> When a container resource in the desired state reaches the Terminated lifecycle state and its `on_exit` or `on_terminate` policy requires recreation, the reconciler must start a replacement.

> r[autonomous.restart.record]
> The runtime must keep a durable, per-instance record of container restarts. Each record identifies the instance it belongs to, the app generation in force when it was recorded, when the restart happened, the exit status of the run that ended where the platform reports one, and the restart's cause: whether it was recovery from an unexpected exit, or a restart the runtime performed deliberately.
>
> The cause is a statement about why the restart happened, not about which component performed it. Who actions a restart is a platform detail — a platform with a service supervisor leaves recovery to it, and one without leaves the runtime to perform both kinds — so recording the actor would make the record mean different things on different platforms.
>
> Recording must not depend on catching a state transition. A container that restarts and returns to running between two observations must still be recorded, so the count of restarts the runtime holds does not depend on how often it looks.
>
> Rolling updates, [replacements](rule:r/autonomous.healthcheck-replace) and operator-requested restarts are recorded as deliberate and excluded from the crash-loop rate. Otherwise every rolling update reads as a crash burst.

> r[autonomous.restart.rate]
> Crash-loop detection is a function of the recorded restart rate: when the number of recovery restarts recorded for an instance within the configured window reaches the configured threshold, the reconciler must file a `crash_loop` fault against that instance (see [fault.crash-loop](rule:r/fault.crash-loop)).
>
> This is the primary crash-loop trigger. It catches sub-threshold flapping — a container that crashes a few times a day forever, never exhausting the supervisor's own start limit inside its window — which is otherwise invisible to an operator.

> r[autonomous.restart.rate.settings]
> The restart-rate threshold and window must be operator-visible and operator-settable, and must take effect without restarting the runtime.
>
> The default must be loose enough that a slow-failing container (one that takes seconds to crash) gets several chances, and tight enough to catch flapping on a human-meaningful timescale.

> r[autonomous.restart.backoff]
> Where the platform's supervisor actions restarts, per-unit restarts must be paced so that a crash-looping container does not exhaust the supervisor's start-rate limit before the reconciler has a chance to detect the problem. On Linux, container units must specify:
>
> - A non-default `RestartSec` (no shorter than several seconds) so the unit does not retry at the systemd default cadence (~100ms).
> - A `StartLimitIntervalSec` and `StartLimitBurst` that allow several attempts within a window measured in minutes, not seconds, before systemd gives up.
>
> The exact values are an implementation concern — they need only be loose enough that a slow-failing container gets multiple chances, and tight enough that a permanently broken container reaches the start limit on a human-meaningful timescale. These are pacing parameters, not the definition of a crash loop; that is [autonomous.restart.rate](rule:r/autonomous.restart.rate).

> r[autonomous.restart.backoff.undisturbed]
> While the supervisor is itself bringing a unit up — its first start, or the pause before an automatic restart — the reconciler must neither tear the unit down nor start it again.
> It must distinguish a unit the supervisor reports as starting from one the supervisor reports as fully started, and treat only the latter as evidence of a broken unit when no container is running under it.
> A unit whose spec no longer matches the app's definition is exempt: replacing it is the point, and the supervisor has no view of the definition.
>
> A unit the supervisor is pacing is not faulty, it is between attempts, so no unit-failure fault arises from this window.
>
> Replacing a unit mid-backoff restarts the supervisor's restart accounting from zero, so the [start limit](rule:r/autonomous.restart.start-limit-hit) is never reached and the crash-loop escalation that depends on it never happens.
> This is not a narrow race: a container that fails shortly after each start spends most of every restart cycle in this state, so a reconciler that acts on it replaces the unit on nearly every pass and the limit the backoff exists to reach recedes indefinitely.

> r[autonomous.restart.start-limit-hit]
> When a container unit reaches `failed/start-limit-hit` (systemd has refused further restarts because the unit exhausted [`StartLimitBurst`](rule:r/autonomous.restart.backoff)) the reconciler must:
>
> - File a `crash_loop` fault scoped to the offending instance, distinct from `container_start_failed`. This is a secondary trigger: a unit the supervisor has given up on must produce the fault even when the recorded rate has not reached its threshold.
> - Stop attempting to auto-recover the instance (no `reset_failed_unit` + restart cycle) until the fault is cleared. The expected recovery path is operator intervention — fixing the underlying cause, redeploying with new config, or explicitly clearing the fault.
> - Clear the fault automatically if the instance is later observed healthy.

> r[autonomous.job-terminal]
> A Job instance that has naturally reached the Terminated lifecycle state must not be restarted by the reconciler.
> The reconciler must clean up any lingering container or unit state for such an instance but must not start a replacement.
> This applies both while a lifecycle operation is in progress (the job completed during the operation) and in steady state.
> A Job that the reconciler asked systemd to start in a prior tick and that the reconciler currently observes as gone counts as "naturally terminated" for the purposes of this rule, even when no `container_running` observation was ever recorded — short-lived jobs may complete and be auto-removed faster than the observer's poll interval, and forcing a re-start in that case would loop indefinitely.

> r[autonomous.job-terminal.defense]
> The reconciler must remember which Job instances have completed within the current process lifetime.
> If a remembered completed Job instance is subsequently observed running again (e.g. restarted externally), the reconciler must stop it.

> r[autonomous.scale]
> When a Deployment resource's observed running instance count differs from its declared scale, the reconciler must start or stop instances to converge on the declared count.

> r[autonomous.healthcheck-replace]
> When a Deployment instance with [`on_failure: "replace"`](rule:l/deployment.healthcheck.on-failure) is observed unhealthy, the reconciler must:
>
> - Spawn a replacement instance for the same Deployment so that the unhealthy instance has a healthy sibling-in-progress.
> - Inhibit any stop of the unhealthy instance while the replacement is starting; the unhealthy instance must keep its routing-pool membership in degraded mode (per [lifecycle.service.routing-pool](rule:r/lifecycle.service.routing-pool)) until the replacement is observed healthy.
> - Once the replacement is observed healthy, retire the unhealthy instance through the normal scale-down path so the routing pool converges on the healthy backend.
>
> Instances with `on_failure: "monitor"` must not trigger this behaviour. Their routing-pool membership still follows the prefer-healthy rule, but no replacement is spawned.

> r[autonomous.healthcheck-replace.guard]
> A fresh instance brought up by either a [rolling update](rule:r/update.rolling) or by [autonomous.healthcheck-replace](rule:r/autonomous.healthcheck-replace) must itself become healthy within its declared `start_period + retries × interval`. If it does not, the reconciler must:
>
> - Stop the failed instance so it does not accumulate a permanent footprint.
> - File a fault per [fault.healthcheck-replace-failed](rule:r/fault.healthcheck-replace-failed).
> - Refuse to spawn further replacement attempts for this Deployment — whether rolling-driven or healthcheck-driven — until the AppDef generation changes (i.e. the operator pushes new code) or the operator clears the fault.
>
> The `on_failure` policy decides which deployments are replaced, not which are watched.
> A deployment declaring `on_failure: monitor` is never bumped by a replacement, but a rolling update of it brings up a fresh instance like any other, and the guard must evaluate that instance.
> Every deployment that declares a healthcheck therefore has a grace window, whatever its policy.
>
> The pre-existing instance must continue to run in degraded mode (under its old spec hash if it was a rolling-update target). The runtime must not allow the guard to trigger an indefinite churn of replacement attempts; the guard exists precisely to bound the cost of a permanently-broken workload — whether the breakage was introduced by a script change or arose spontaneously.

> r[scaling.decision]
> The runtime maintains a persistent record of operator-chosen scale for each Deployment that has been explicitly scaled.
> When present, this record determines the effective scale used by the reconciler instead of the lower bound default.

> r[scaling.decision.actions]
> A named Deployment named by an action resolves to the replicas its effective scale calls for, the same number the reconciler resolves it to, rather than to the lower bound of its declared range.
> An action that addresses a Deployment addresses the whole of it: a `rt.stop` that reached only part of a scaled deployment would report the workload stopped while the rest of it went on serving, which is the opposite of what a maintenance action that stops a deployment before acting on its data is for.

> r[scaling.decision.unreadable]
> Where the record of operator-chosen scale cannot be read, the runtime must not fall back to a declared bound: how many instances the operator asked for is unknown, not known to be the minimum.
> The app must not be actuated for that pass, so whatever is already running keeps running, and the condition must be visible to an operator for as long as it lasts.
> An operator asking what a deployment's scale is must be told the figure is unavailable rather than given the bound.
>
> The lower bound is the floor of what an operator may choose, so using it here answers the question with the smallest answer it could have had.
> A deployment scaled well above its floor is then converged down to it, tearing down instances on the strength of a question that was never answered, and building them back up on the next pass that can read the record.

> r[scaling.clamp]
> When an app's BSL script is re-evaluated (e.g. via an update), any stored scaling decision for a Deployment whose declared bounds have changed must be clamped to the new bounds.
> If the stored value falls below the new lower bound it is raised; if it exceeds the new upper bound it is lowered.
> Clamping is applied before the reconciler acts on the updated definition.

> r[autonomous.ingress]
> When an Ingress resource's configuration in the ingress controller does not match the desired configuration, the reconciler must update or rebuild the configuration.

> r[autonomous.network]
> When a Service resource's network plumbing is missing or misconfigured, the reconciler must recreate or repair it.

> r[autonomous.provenance-required]
> Every autonomous operation must record [provenance](rule:r/history.operations.provenance) before execution.
