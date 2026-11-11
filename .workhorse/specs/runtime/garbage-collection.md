---
id: GC
---

# Garbage collection

Applies to the [runtime](../overview.md#runtime)'s deletion of stale records from its operational store on a [host](../overview.md#host).

## Collection

The runtime MUST periodically delete stale operational records, so that the store's size stays bounded.
The runtime MUST run a collection pass once per collection interval, in the background.
A failure to delete one kind of record MUST NOT prevent the pass from deleting the others.

| setting | default | daemon option |
| --- | --- | --- |
| collection interval | 3600 seconds (one hour) | `--gc-interval-secs` |
| action log retention | 86400 seconds (24 hours) | `--gc-retain-action-log-secs` |
| cleared fault retention | 604800 seconds (7 days) | `--gc-retain-cleared-faults-secs` |
| completed autonomous operation retention | 604800 seconds (7 days) | `--gc-retain-completed-operations-secs` |
| unscheduled instance retention | 600 seconds (10 minutes) | `--gc-retain-unscheduled-instances-secs` |
| restart records kept per instance | 50 | `--gc-retain-restart-records` |

An operator MAY override each setting with its daemon option when the daemon starts.

## Retention

The runtime MUST delete each [action execution log](history.md#action-execution-log) entry recorded longer ago than the action log retention, unless it belongs to the [lifecycle operation](../overview.md#lifecycle-operation) in progress.

The runtime MUST delete each [fault](../overview.md#fault) record cleared longer ago than the cleared fault retention.

The runtime MUST delete each [autonomous operations log](history.md#autonomous-operations-log) entry completed longer ago than the completed autonomous operation retention.

The runtime MUST delete every observation, autonomous operations log entry and [restart record](autonomous.md#restart-record) of a resource instance for which it no longer holds an instance record.

## Restart records

The runtime MUST keep the configured number of most recent restart records of each resource instance, and MUST delete that instance's older ones.
The runtime MUST record every restart, however quickly an instance is restarting.

> [!NOTE]
> A crash loop produces restart records fastest exactly when each attempt's exit status is the diagnostic, so the bound is per instance and applies to storage, not to recording.

## Instance retirement

Retiring a resource instance deletes its instance record together with its observations, its faults and its autonomous operations log entries.
The runtime MUST retire an instance in one atomic step, leaving none of those records behind if any of them is deleted.

## Unscheduled instances

The runtime MUST retire each resource instance that has been `Unscheduled` for longer than the unscheduled instance retention, unless the instance is in its app's active desired state.
The runtime SHOULD look for such instances once per collection interval.

An app's active desired state is the [desired state](reconciliation.md#desired-state) the runtime computed from the app's definition in [steady state](reconciliation.md#steady-state): each singleton instance, and each instance a scaled group keeps.
A [stopped](../interface/app-control.md#resource-stop) singleton MUST count as part of its app's active desired state.
The active desired state of an app that is not installed, and has no lifecycle operation in progress, is empty.

The runtime MUST NOT retire an instance of an app whose active desired state it does not know.
Such apps include an app with a lifecycle operation in progress, an app that is installing or uninstalling, an app whose desired state could not be computed, and an app the runtime could not load.

## Demoted instances

When the desired state demotes a scaled instance to `Unscheduled`, or a deployment that has become scaled leaves a singleton instance behind, the runtime MUST retire that instance as soon as any of the following holds:

- it is [torn down](#instances-started-by-actions);
- it has no observations.

A failure to read the instance's observation history MUST NOT count as the instance having no observations.

## Instances started by actions

When an action starts a scaled group, the runtime MUST leave out of the group every instance whose observation history shows it torn down, and MUST allocate a fresh instance in place of each one left out.
An instance is torn down when its lifecycle state is `Unscheduled`, or when its most recent observation is one that moves an instance to `Unscheduled`.

When an action stops, queries, signals or warms a deployment, the runtime MUST address the instances the deployment has, whether torn down or not.

In steady state, the runtime MUST keep each scaled instance in its group, whatever its containers have done.

> [!NOTE]
> The instances an action leaves out are the ones retirement deletes, so an action's started instances and the runtime's retirements never name the same instance, and an action that starts between the runtime reading desired state and acting on it cannot have an instance deleted under it.
> In steady state, re-identifying a scaled instance would discard the restart records and observation history held against its identity.
