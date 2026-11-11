---
id: FLT
---

# Faults

## Fault Handling

<!-- from runtime: Fault Handling -->

> r[fault.definition]
> A fault is a condition where the runtime determines that convergence is impossible or that a persistent failure pattern exists.
> Faults are not handled by BSL scripts.

> r[fault.detection]
> The runtime must detect faults from patterns in the [world observation history](rule:r/history.world) and [autonomous operations log](rule:r/history.operations).
>
> Examples of fault conditions:
> - A [barrier deadline](rule:r/barrier.deadline) expires.
> - A container repeatedly terminates shortly after starting and backoff has been exhausted.
> - A resource cannot be created due to a persistent external condition.

> r[fault.image-pull]
> When the reconciler fails to pull a container image required by a resource instance, it must file a fault of kind `image_pull_failed` associated with that instance.
> The fault is cleared automatically when a subsequent pull of the same image succeeds.

> r[fault.container-start]
> When the reconciler observes that a resource instance's backing unit is in a failed state while the desired state is active, it must file a fault of kind `container_start_failed` associated with that instance.
> The fault is cleared automatically when the unit is subsequently observed in an active or activating state.

> r[fault.crash-loop]
> The reconciler must file a fault of kind `crash_loop` associated with a resource instance, distinct from `container_start_failed`, when either:
>
> - the instance's recorded restart rate reaches the configured threshold (per [autonomous.restart.rate](rule:r/autonomous.restart.rate)), or
> - its backing unit has reached the start-limit-hit terminal state (per [autonomous.restart.start-limit-hit](rule:r/autonomous.restart.start-limit-hit)).
>
> The fault must identify which of the two conditions filed it, so that an operator can tell a rate-derived crash loop from one the supervisor has already given up on.
> The fault is cleared automatically when the instance is subsequently observed healthy. While the fault is active, the reconciler must not auto-restart the affected instance.

> r[fault.external-volume-unmapped]
> When a deployment or job instance requires an external volume that has no mapping in the external volume mapping table, the reconciler must not start the instance and must file a fault of kind `external_volume_not_mapped` against that instance, identifying the missing volume name.
> The fault is cleared automatically when a subsequent reconciliation tick is able to start the instance (i.e. the mapping has been added and the start succeeds).

> r[fault.cert-acquisition]
> When TLS certificate acquisition for an ingress hostname fails persistently (after the proxy's own retry policy has been exhausted), the runtime must file a fault of kind `cert_acquisition_failed` associated with that ingress, identifying the hostname and the most recent acquisition error.
> The fault is cleared automatically when a subsequent acquisition for the same hostname is observed as `valid`.

> r[fault.proxy-apply-failed]
> When applying the runtime-built proxy configuration to the network proxy fails on a reconciliation tick, the runtime must file a fault of kind `proxy_apply_failed` against every ingress resource (app and site) whose data was included in that configuration. Each fault must identify the offending ingress and carry the most recent apply error so that operators can see which ingresses are currently degraded via the operator interface (`ListFaults`, `GetStatus`, `DescribeApp`).
> The faults are cleared automatically when a subsequent apply for the same configuration set succeeds.

> r[fault.healthcheck]
> When a container instance has a declared [healthcheck](rule:l/deployment.healthcheck) and has been continuously unhealthy for longer than its grace window (`start_period` plus `retries × interval`), the runtime must file a fault of kind `health_check_failed` associated with that instance.
> The fault is cleared automatically when the instance is next observed as healthy, or when the instance is unscheduled.
>
> A container without a declared healthcheck, or one still within its grace window, must not cause this fault to be filed.

> r[fault.healthcheck-replace-failed]
> When the replace-loop guard ([autonomous.healthcheck-replace.guard](rule:r/autonomous.healthcheck-replace.guard)) trips for a Deployment, the runtime must file a fault of kind `health_check_replace_failed` associated with that Deployment, identifying the failed replacement instance and the persistently-unhealthy original.
> The fault is cleared automatically when the AppDef generation changes for the app, or manually by the operator.
> While the fault is active, the runtime must not spawn further replacements for this Deployment.

> r[fault.service-degraded]
> When a Service's routing pool contains only unhealthy backends (i.e. the prefer-healthy rule has fallen back to "anything running" per [lifecycle.service.routing-pool](rule:r/lifecycle.service.routing-pool)), the runtime must file a fault of kind `service_degraded` associated with that Service.
> The fault is cleared automatically when at least one backend in the pool becomes healthy or when the Service is unscheduled.

> r[fault.definition-source-moved]
> While a [re-check](rule:r/definition.recheck) finds that the reference recorded in an app's fetched definition selects a digest other than the one the app is running, the runtime must hold an app-level fault of kind `definition_source_moved` against the app, identifying the reference and both digests.
> The fault is cleared when a re-check selects the running digest again, when the app's definition is replaced, or when the app is deregistered.
> A failed re-check neither files nor clears the fault: a registry that could not be reached shows neither that the tag moved nor that it did not.
> The fault is held durably, so it persists across a daemon restart until one of its clearing conditions occurs.

> r[fault.definition-unsupported]
> While the running Seedling does not satisfy the [Seedling version requirement](rule:l/bsl.bundle.seedling-versions) declared by an app's current definition, the runtime must hold an app-level fault of kind `definition_unsupported` against the app, identifying the requirement and the running version.
> The definition is evaluated and maintained as usual while the fault is held.
> The fault is cleared when the app's definition is replaced by one the running Seedling satisfies, when the runtime starts as a version that satisfies it, or when the app is deregistered.

> r[fault.lifecycle]
> Every fault kind defines both the condition under which it is filed and the condition under which it clears; a kind with no clearing condition is not permitted.
> A fault identifies the thing that is faulty, not merely the app it belongs to, and at most one fault is active for a given (app, kind, subject) at a time.
> Clearing is keyed no more broadly than filing: an event affecting one subject must not clear a fault held by another.
> A fault whose filing condition is a state of the world ("this is true right now") is active exactly while that condition holds, including across daemon restarts — its clearing must not depend on state held only in memory.
> A fault that is deliberately retained after its trigger has passed must name the lifecycle event that clears it.

> r[fault.surfacing]
> Faults must be surfaced to operators through the operator interface (defined in a separate spec).
> The runtime must not silently discard faults.

> r[fault.non-blocking]
> A fault on one resource must not prevent the reconciler from continuing to manage other resources.
> The faulted resource is excluded from active reconciliation until the fault is resolved.
