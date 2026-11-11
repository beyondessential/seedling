---
id: SCHED
---

# Scheduled actions

Applies to the firing of the actions an [app](../overview.md#app) declares with [`on_schedule`](../language/action.md#scheduled-actions).

## Schedule records

The runtime MUST hold a durable record for each schedule an app declares, made up of the app, the action, the cron expression, and the time the schedule last fired.

When it evaluates an app's definition successfully, the runtime MUST add a record for each declared schedule it lacks, keeping the last fire time of the records it already has.
It MUST delete each of the app's records whose action and cron expression the definition no longer declares.
The runtime MUST NOT change the app's records on the strength of a definition that fails to evaluate.

## Firing

The runtime MUST check for due schedules at least once a minute, as part of the [reconciliation loop](reconciliation.md#reconciliation-loop).

A schedule is due when the first boundary of its cron expression after its last fire time is at or before the time of the check.
The runtime SHOULD treat a schedule with no recorded fire time as having last fired five minutes before the check.

The runtime MUST fire a due schedule only for an app in the `Installed` [install phase](../overview.md#install-phase), and MUST skip it for an app that is `NotInstalled`, `Installing` or `Uninstalling`.

> [!NOTE]
> A schedule that fired for an app whose install keeps failing would hold the app's one operation slot and keep a corrected definition from ever being applied.

The runtime MUST fire a due schedule by requesting its action as a [lifecycle operation](../overview.md#lifecycle-operation) with no supplied params and the trigger `schedule`, subject to the [concurrency rules](operations.md#concurrency).
A request that is rejected leaves the schedule due, and a later check requests it again.

## Fire times

The runtime MUST record a fire, by setting the schedule's last fire time to the time of the check, when the operation it requested starts, or when the run [fails param validation](../language/action.md#scheduled-actions).
The runtime MUST NOT record a fire before either.
The runtime MUST record a queued fire when its operation leaves the queue and starts.

> [!NOTE]
> A run that fails validation is not requested again until the schedule's next boundary.

The runtime MUST fire a due schedule once, however many of its boundaries passed since it last fired.
Once it has fired, the runtime evaluates the next boundary from the new fire time.
