---
id: GC
---

# Garbage collection

## Garbage Collection

<!-- from runtime: Garbage Collection -->

> r[gc.background]
> The runtime must periodically remove stale rows from operational database tables to bound storage growth. Garbage collection must run as a background task on a configurable interval, defaulting to one hour.

> r[gc.action-log]
> Completed lifecycle operation entries in the action log must be deleted after a configurable retention period (default: 24 hours). Entries belonging to the current in-flight operation must never be deleted.

> r[gc.faults]
> Cleared fault records must be deleted after a configurable retention period (default: 7 days).

> r[gc.observations]
> World observation rows whose instance identity no longer appears in the resource instance registry must be deleted.

> r[gc.autonomous-operations]
> Completed autonomous operation records must be deleted after a configurable retention period (default: 7 days).
> Autonomous operation records whose instance identity no longer appears in the resource instance registry must be deleted.

> r[gc.restarts]
> [Restart records](rule:r/autonomous.restart.record) must be bounded per instance: only a configurable number of the most recent records for an instance are retained (default: 50), and older records for that instance are deleted.
> The bound is per instance rather than global, and applies to storage rather than to recording: a crash loop produces records fastest exactly when the per-attempt exit statuses are the diagnostic, so no restart may go unrecorded merely because the instance is restarting quickly.
> Restart records whose instance identity no longer appears in the resource instance registry must be deleted.

> r[gc.instances]
> Resource instance records that have remained in the Unscheduled lifecycle state for longer than a configurable retention period (default: 10 minutes) must be deleted, along with their associated world observation rows.
> Instances that are part of the active desired state (i.e. in the `keep` set of a scaled group or a singleton) must never be deleted regardless of their lifecycle state.
> A stopped singleton remains part of the active desired state.
> Membership is judged against the desired state the reconciler computed from the app's definition, so an instance belonging to an app whose desired state is not known must not be deleted: while a lifecycle operation is in progress for the app, while it is installing or uninstalling, when its desired state could not be computed, or when the runtime could not load the app.

> r[gc.instances.operation-keep-set]
> An action starting a scaled group takes only instances the observation history does not already show torn down, allocating fresh ones in place of those it leaves out.
> The instances it leaves out are the ones retirement deletes, so what an action starts and what the reconciler may delete never name the same instance, and an action that starts between the reconciler reading desired state and acting on it cannot have an instance deleted under it.
>
> This holds for starting a group and not for addressing one. An action that stops, queries, signals or warms a deployment names the instances that deployment has, torn down or not: resolving those to fresh instances instead would have it address replicas that do not exist and never will, while the ones it meant to address carried on.
>
> It holds for actions and not for the reconciler, whose steady-state desired state keeps a scaled instance whatever its containers have done, because re-identifying it would discard the [restart record](rule:r/autonomous.restart.record) and observation history held against its identity.

> r[gc.instances.atomic]
> The deletion of an instance's records must be atomic across the world observation rows, fault rows, autonomous operation rows, and the resource instance row.
> A partial deletion that leaves any of these out of sync with the others is a defect: it allows orphan faults and registry rows to linger indefinitely, since subsequent garbage-collection passes select instances by observation history that no longer exists.

> r[gc.instances.never-actuated]
> A scaled instance that the desired state has demoted to Unscheduled, or a singleton left behind by a deployment that has since become scaled, must be retired immediately by the reconciler in any of the following cases:
>
> - The instance's lifecycle state, as derived from observations, is Unscheduled.
> - The instance has no observations at all (it was never actuated).
> - The instance has at least one terminal observation (`container_removed` or `network_cleaned_up`) in its history, even if the derived lifecycle state is something else.
>
> The third case is required because multiple observations recorded within the same tick may share a timestamp, and the lifecycle derivation is order-sensitive: a non-deterministic ordering can leave the derived state at `Pending` even though terminal observations exist. The presence of any terminal observation is sufficient evidence that the actuator successfully tore the instance down at some point, and waiting for the lifecycle derivation to "agree" would leave the registry slot occupied indefinitely.
> Without this rule the registry slot lingers forever, along with any faults filed against the instance during its brief actuation attempt.
