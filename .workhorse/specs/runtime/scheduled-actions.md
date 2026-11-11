---
id: SCHED
---

# Scheduled actions

## Scheduled Actions

<!-- from runtime: Scheduled Actions -->

> r[schedule.tick]
> The reconciliation loop must check for due scheduled actions at least once per minute. This check is performed as part of the normal reconciliation tick.

> r[schedule.fire]
> For each `(app, action, cronexpr)` tuple, the runtime computes the next fire time from the stored `last_fired_at` and the cron expression. If the next fire time falls within the last 59 seconds, the action is fired as a lifecycle operation with an empty param map and the `"schedule"` trigger.

> r[schedule.fire.installed-only]
> Schedules must only fire for apps in the `Installed` [phase](rule:r/lifecycle.app). Schedules attached to apps that are `NotInstalled`, `Installing`, or `Uninstalling` must be skipped. This prevents a permanently-failing app's schedule from monopolising the per-app scheduler slot and blocking operator updates: a script with a faulty install whose every cancel is followed by a fresh schedule fire would otherwise be unable to ever accept a corrected script.

> r[schedule.state]
> The runtime stores `(app_name, action_name, cronexpr, last_fired_at)` tuples durably. `last_fired_at` is updated on each successful fire.

> r[schedule.catch-up]
> If the runtime detects that a scheduled fire time has already passed (for example because the daemon was not running at the moment of the scheduled fire), the schedule fires exactly once to catch up, regardless of how many scheduled boundaries were missed. The schedule's `last_fired_at` is then set to the catch-up fire time, so subsequent cron boundaries are evaluated from there.

> r[schedule.prune]
> When a BSL script is evaluated, the runtime must prune schedule state rows that no longer match any `(action, cronexpr)` pair declared in the script.

> r[schedule.audit]
> Scheduled action fires must be recorded in the audit log as lifecycle operations with the `"schedule"` trigger.

> r[schedule.start-reject]
> Calling `on_schedule` on the Start Action (action name `"start"`) must throw at script evaluation time.
