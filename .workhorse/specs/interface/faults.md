---
id: FLTS
---

# Faults and restarts

## Fault Surface

<!-- from interface: Fault Surface -->

> i[fault.record]
> A fault record contains the following fields: `id` (opaque string), `app`, `resource_type`, `resource_name`, `instance_id`, `kind`, `timestamp` (RFC 3339), and `description` (human-readable string).

> i[fault.list]
> `/faults/list { app? }` returns an array of currently active fault records.
> If `app` is provided, only faults for that app are returned; otherwise all active faults across all apps are returned.

> i[fault.derived]
> Faults are derived conditions.
> They clear automatically when the condition that caused them no longer holds.
> No acknowledgement mechanism is provided; fault resolution and incident tracking are left to external consumers of the event feed.

> i[fault.clear-app]
> `/faults/clear { app }` clears every currently active fault for the named app.
> Returns `{ app, cleared }` where `cleared` is the number of fault rows that transitioned from active to cleared.
> Faults derived from observable conditions (e.g. `image_pull_failed`, `health_check_failed`) will be re-filed on the next reconciliation tick if the underlying condition still holds. Hard faults that require operator action (e.g. `health_check_replace_failed`, `script_error`) are cleared definitively until the underlying issue recurs.
> The endpoint is intended for operators stuck behind a fault that the runtime cannot itself resolve, including the case where a not-installed app's faults are blocking a script update.

## Restart Surface

<!-- from interface: Restart Surface -->

> i[restart.record]
> A restart record contains the following fields: `id` (monotonically increasing integer), `app`, `instance_id`, `resource_type`, `resource_name`, `generation` (integer, null when the app had no current generation at the time), `timestamp` (RFC 3339), `cause` (`"recovery"` or `"deliberate"`), `exit_code` (integer, null when unknown), and `exit_kind` (`"exited"`, `"signalled"`, `"dumped"`, or null when unknown).
> `cause` distinguishes recovery from an unexpected exit from a restart the runtime performed on purpose. It describes why the restart happened, not who performed it: which component actions a restart is a platform detail, and on a platform with no service supervisor the runtime performs both kinds.
> For `exit_kind: "exited"`, `exit_code` is the process's exit status; for `"signalled"` and `"dumped"` it is the signal number that terminated it.

> i[restart.list]
> `/restarts/list { app?, instance?, limit? }` returns an array of [restart records](rule:i/restart.record), most recent first.
> `app` restricts the result to one app and `instance` to one instance id; both may be given. `limit` caps the number of records returned, defaulting to 100 and capped at 1000.

> i[restart.settings]
> `/restarts/settings/get` returns `{ threshold, window_secs }` — the number of recovery restarts within `window_secs` seconds that files a `crash_loop` fault (see [autonomous.restart.rate](rule:r/autonomous.restart.rate)).
> `/restarts/settings/set { threshold?, window_secs? }` updates either or both and returns the full settings object. Omitted fields are left unchanged. `threshold` must be at least 2 and `window_secs` at least 60; values outside those bounds are rejected.
