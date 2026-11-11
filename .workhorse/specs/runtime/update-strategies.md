---
id: UPD
---

# Update strategies

## Update Strategies

<!-- from runtime: Update Strategies -->

> r[update.spec-hash]
> Each running container carries a spec hash that captures its full configuration at start time.
> The reconciler must compare the observed spec hash of each running instance against the desired spec hash derived from the current definition.
> An instance whose observed hash differs from the desired hash (or whose hash is absent) is _stale_.

> r[update.rolling]
> When a Deployment's update strategy is Rolling and stale instances are detected, the reconciler must rotate instances incrementally:
>
> 1. Temporarily increase the effective instance count by one beyond the current scale.
> 2. Start a new instance with the current definition in the additional slot.
> 3. Wait until the new instance is running and healthy before proceeding.
> 4. Stop one stale instance.
> 5. Repeat from step 2 until no stale instances remain.
> 6. Return the effective instance count to the declared scale.
>
> At no point during a rolling update may the number of healthy instances drop below the Deployment's scale lower bound,
> except when all instances are stale and no healthy replacement exists yet (the initial ramp-up).

> r[update.rolling.over-provision]
> The temporary over-provisioning required by a rolling update must be reflected in the desired state computation
> so that the additional instance is not treated as excess by the scaling machinery.
> The reconciler must track which deployments have an active rolling update and feed this into the effective scale calculation.

> r[update.rolling.restart-resume]
> A rolling update must resume correctly after a runtime restart.
> Because the runtime re-derives rollout state from observed spec hashes on each tick, no explicit rollout-in-progress flag is required in persistent storage.
> If the runtime restarts mid-rollout, it must observe the running containers, detect any remaining stale instances, and continue the rotation from where it left off.

> r[update.rolling.reboot-resume]
> After a full node reboot (no containers surviving), all instances are missing.
> The reconciler must start all instances with the current definition.
> Because no stale containers exist, no rolling update coordination is needed and the deployment converges directly to the desired state.

> r[update.replace]
> When a Deployment's update strategy is Replace and stale instances are detected, the reconciler must stop all stale instances before starting replacements.
> This may temporarily violate the Deployment's scale lower bound.
> New instances are started only after all stale instances have been fully stopped.

> r[update.jobs]
> Jobs do not participate in rolling or replace update coordination.
> A stale Job instance is stopped and restarted immediately.
