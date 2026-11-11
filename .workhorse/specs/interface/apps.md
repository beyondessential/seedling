---
id: APPS
---

# App management

## App Management

<!-- from interface: App Management -->

> i[app.register]
> `/apps/create { app, ... }` evaluates the supplied [definition](rule:i/definition.source).
> On success, the app is added to the managed set in the `NotInstalled` state and an `AppRegistered` event is emitted.
> On script failure, `script_error` is returned and the app is not registered.
> A definition refused while it is fetched or its bundle checked returns the error for that refusal, and the app is not registered.
> A registration that fails for any reason leaves nothing observable behind: the app does not appear in listings, does not survive a restart, and a retried registration of the same name succeeds rather than being rejected as already registered.

> i[app.persist]
> Registered apps, their definitions, and their definitions' provenance are stored durably and reloaded automatically on restart.
> Reloading runs no [validators](rule:i/param.validation).
> A stored definition that fails to evaluate on reload files a `script_error` app-level fault.

> i[app.deregister]
> `/apps/remove { app }` initiates graceful teardown of all of the app's resources and removes the app from the managed set.
> If a lifecycle operation is in progress for the app, the request is rejected with `operation_in_progress`.
> Otherwise the app immediately enters the `Deregistering` state and an `AppDeregistered` event is emitted when teardown completes and the app is fully removed.

> i[app.update]
> `/apps/update { app, ..., param? }` replaces the app's [definition](rule:i/definition.source), optionally changing one parameter in the same step.
> `param`, when present, is an object `{ name, value }`, where `value` is a string to set the parameter to or `null` to unset it.
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`.
>
> The new definition is evaluated with the parameter values the request would produce, and those values are then [validated](rule:i/param.validation) against it.
> If the definition cannot be fetched, its bundle is refused, it fails to parse or evaluate, or validation rejects, the request fails with the corresponding error and nothing observable changes: no bundle or parameter value is stored, the generation is not bumped, no fault is filed or cleared, and the existing AppDef continues running.
> "Continues running" extends to every piece of state derived from the definition: volume data is not held, scaling decisions are not clamped, port forwards are not torn down, and action schedules are not pruned.
> A partially-evaluated definition is never observable: an update that failed part-way through must not be distinguishable, in any state derived from the definition, from an update that was never submitted.
>
> On success, any previously active `script_error` fault for this app is cleared, and a single [generation](rule:r/generation.definition) bump records the new definition and the parameter change together, with a `ScriptUpdate` history entry.
> When the parameter change matches one of the [transitions](rule:l/param.on-change.transitions) defined in the language spec, the new definition registers an `on_change` handler for that parameter, and the app is installed, the handler is scheduled as a lifecycle operation.
> It runs under the new definition, with `old` being the previous generation.
> A definition replaced without a parameter change schedules no handler.
>
> Returns `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }`, as for [param.set](rule:i/param.set).

> i[app.generation]
> Every registered app has a current [generation](rule:r/generation.definition) — a per-app monotonic integer identifying the app's defined state at a point in time.
> The generation is bumped on initial registration, on each successful `/apps/update`, and on each successful parameter set or unset.
> Previous generations are retained durably so that operators and automation can retrieve any historical state of the app.

> i[app.list]
> `/apps/list` returns an array of objects with fields `name`, `status`, and `fault_count` (the number of currently active, uncleared faults filed against the app).

## App Status

<!-- from interface: App Status -->

> i[app.status]
> Every managed app is in exactly one of the following derived states at any time:
>
> - `NotInstalled`: the install action has never completed successfully for this app, and no install is currently in progress.
> - `Installing`: an install operation is in progress for this app. The reconciler actuates resources the install closure has placed into the desired state per [desired-state.during-install](rule:r/desired-state.during-install).
> - `Deregistering`: deregistration was requested and resource teardown is in progress.
> - `Operating`: a lifecycle operation other than `install` is in progress. Includes the field `action_name`, the name of the action that operation runs.
> - `Running`: steady state; no active faults; all resources are at their desired lifecycle states.
> - `Degraded`: steady state, but one or more resources are not at their desired lifecycle state or have an active fault. Resources that are actively being torn down (`Terminating` or `Terminated` lifecycle state) are not counted as degraded — they are transitioning to `Unscheduled` and do not represent a fault condition.
> - `Faulted`: one or more active faults exist and at least one resource has been excluded from active reconciliation.
>
> Outside an operation, a resource's desired lifecycle state is `Ready`, except that a [stopped](rule:i/resource.stop) resource and a Deployment at a scale of zero are desired at `Unscheduled`.
> A Job runs once rather than staying up, so it is at its desired state while `Ready` and once its run has ended in success, whether or not its container has since been removed; success is judged as for [rt.termination.ensure-success](rule:l/rt.termination.ensure-success).
> An external service is provided from outside the runtime, has no lifecycle state, and is always at its desired state.

> i[app.status.priority]
> When multiple conditions apply simultaneously, the state with the highest priority is reported.
> Priority order, highest first: `Deregistering`, `Installing`, `Operating`, `NotInstalled`, `Faulted`, `Degraded`, `Running`.

## App Description

<!-- from interface: App Description -->

> i[app.describe]
> `/apps/show { app }` returns a single object with the following fields:
>
> - `status`: the app's current status as defined in [app.status](rule:i/app.status).
> - `faults`: array of app-level [fault records](rule:i/fault.record) not associated with a specific resource instance (e.g. script evaluation errors). Empty when there are no active app-level faults.
> - `definition`: the [provenance](rule:i/definition.provenance) of the app's current definition.
> - `resources`: array of objects with fields `name`, `type`, `instances`, `faults`, `def`, and for Deployment resources, `scale`.
>   Each instance has fields `id`, `display_name`, `lifecycle`, `transition_time` (RFC 3339, optional), and `restarts`.
>   `restarts` summarises the instance's [restart history](rule:i/restart.record): `{ recent, window_secs, total, last_at, last_exit_code, last_exit_kind }`, where `recent` counts recovery restarts within the current rate window, `total` counts all retained records for the instance, and the `last_*` fields describe the most recent record (null when there is none). It is omitted for resource kinds that have no backing container.
>   Each fault entry is a [fault record](rule:i/fault.record).
>   `def` is an object describing the resource's configuration. The shape varies by `type`:
>   for `ingress`: `{ hostname, port, tls, dtls, http_terminate, redirect }`;
>   for `service`: `{ http, exported, balance, routes }`;
>   for `deployment`: `{ container, pod, scale, on_update, on_terminate }`;
>   for `job`: `{ container, pod, deadline }`;
>   for `volume`: `{ readonly, tmpfs, writes, exported, export_description }`.
>   `container` has fields `image`, `command`, `args`, `env`, `volume_mounts`, `on_exit`, `memory`, `cpus`, `extra_caps`, `writable_rootfs`, `pids_limit`.
>   `pod` has fields `service_mounts`, `http_bindings`, `tcp_bindings`, `udp_bindings` (each an array of strings).
>   `routes` is an array of objects with fields `prefix`, `compress`, `balance`, `rate_limit`, `headers`, and `served`, one per HTTP route the service is served through, sorted by `prefix` for stable diffing. It is `null` for a service with no HTTP surface.
> - `params`: array of objects with fields `name`, `value`, `is_set`, `secret`, `kind`, `required`, `description`, and `default_value`.
>   `is_set` is `true` when the parameter has a stored value.
>   `value` is the string value if the parameter is set and not secret; `null` if the parameter is unset or if it is secret.
>   `secret` is `true` when the parameter's effective secret flag is `true` (see [param.schema.secret](rule:l/param.schema.secret) and [param.schema.secret-from-kind](rule:l/param.schema.secret-from-kind)).
>   The schema fields (`kind`, `required`, `description`, `default_value`) reflect any metadata set via the BSL param builder methods.

> i[app.describe.proxy-settings]
> A `service` def reports the proxy settings in force on it, after the resolution defined in [service.http.proxy-settings.resolution](rule:l/service.http.proxy-settings.resolution).
> Values are fully resolved: a field the app set at neither route nor service level is reported at its default rather than as null or absent, so a reader never has to know the defaults to interpret the response.
>
> The def's own `balance` reports the service-wide policy, which governs every backend pool the service is served through.
> Each entry in `routes` reports the settings for one HTTP route, so a route that overrides the service's values reports what it resolved to rather than what the service declared.
>
> `compress` is `null` when compression is disabled for the route, and otherwise an object with fields `encodings` (array of strings), `minimum_length` (integer), and `content_types` (array of strings).
> `balance` is an object with fields `policy` (string), `try_duration`, and `interval` (the latter two in seconds).
> `rate_limit` is `null` when the route is not rate limited, and otherwise an object with fields `max_events` (integer), `window` (seconds), and `shared` (boolean).
> `shared` is true when the limit was declared on the service, so the route counts against a budget common to every route inheriting it; without it two routes reporting the same numbers could not be told apart from two routes holding one budget each.

> `headers` is an object with fields `request` and `response`, each an object with fields `replace` and `add` (maps of header name to array of values) and `remove` (an array of header names).
> A value is reported as an array whichever form the app declared it in, and a direction with no operations reports its three members empty rather than null, so a reader compares like with like across routes.
> Because header operations resolve [per header name](rule:l/service.http.proxy-settings.resolution), a route reports the operations in force on it whether it or the service declared them.
>
> A service whose backing pods declare no HTTP route bindings reports the single `/` route it is served through, so the array is never empty for an HTTP service.
>
> Each route reports `served`, whether a pod binds its prefix and the proxy is therefore told to serve it. A declared prefix that nothing binds reports its settings but does not carry them: requests under it are answered by whichever bound prefix matches, under that route's settings. Without this a rate limit on an unbound route would read as a control in force.
>
> `redirect` is `null` on a route proxied to a pod, and otherwise an object with fields `to` (string) and `code` (integer), describing a [route redirect](rule:l/service.http.route.redirect).
> `to` is the target as the app declared it, with the parts of the request that carry over still spelled as tokens, since what each expands to is a property of the request rather than of the declaration.
> A redirect route is `served` without a pod binding it, and reports `compress` and `rate_limit` null and its request header operations empty, those being the settings a redirect leaves nothing to act on. Its `balance` is the service's, no pool being selected from.

> i[app.describe.param-secret]
> When a param's effective `secret` flag is `true`, its `value` must be `null` in the response regardless of whether a value is stored. Clients must use `is_set` to distinguish an unset secret from a set-but-redacted secret.
> - `unknown_params`: array of objects with fields `name` and `value`, listing parameters that have a stored value in the database but whose name does not appear in the app's current script evaluation. This is informational only; these values have no effect until the script is updated to reference them.
> - `actions`: array of objects with fields `name`, `description`, `kind`, `params`, and `schedules`.
>   `kind` is one of `action`, `shell`, `install`, or `lifecycle`. The `lifecycle` kind is used for the Start Action, which is driven autonomously and cannot be manually invoked.
>   `params` is an object map of param key to `{ kind, required, description, default_value }`, as defined in the language spec. Empty for actions with no declared param schema.
>   `schedules` is an array of objects with fields `cronexpr`, `last_fired_at`, and `next_fire_at`, listing every cron schedule attached to the action via [action.schedule](rule:l/action.schedule). The array is empty for actions with no declared schedule. `last_fired_at` is the RFC 3339 timestamp at which the runtime last fired this schedule, or `null` if it has never fired (or the app is not yet installed). `next_fire_at` is the RFC 3339 timestamp at which the schedule is next expected to fire, computed from `last_fired_at` (or the current time if never fired); it is `null` only when the cron expression cannot be evaluated.
> - `current_operation`: present when status is `Operating` or `Installing` and the operation is running, and describes that operation.
>   An app being deregistered has no operation and does not report one.
>   Has fields `action_name`, `barrier`, `source_generation`, and `target_generation`.
>   `barrier` is either `null` (operation is running but not yet at a barrier, or the barrier has already been satisfied and the closure is about to resume) or an object with fields `resources`, `required_state`, `deadline_secs`, and `elapsed_secs`.
>   `resources` is an array of resource display names the barrier is awaiting.
>   `required_state` is the awaited lifecycle state (e.g. `"Ready"`, `"Terminated"`).
>   `deadline_secs` is the deadline in seconds, or `null` for a deadline-less barrier (see [rt.started.terminated-eventually](rule:l/rt.started.terminated-eventually) and [rt.started.ready-eventually](rule:l/rt.started.ready-eventually)).
>   `elapsed_secs` is how many seconds have passed since the barrier first suspended.
> - `generation`: the current generation of the app.

> i[action.describe.barrier]
> The `current_operation.barrier` field of `/apps/show` reflects the earliest unsatisfied barrier recorded in the action log for the active operation.
> When the operation's closure is between barriers — actively running, or has had all prior barriers satisfied with no new one queued — the field is `null`.
> `elapsed_secs` is computed as `now - started_at_secs`; it persists across runtime restarts because the barrier record survives in the action log.

## App Script Retrieval

<!-- from interface: App Script Retrieval -->

> i[app.script]
> `/apps/script { app, generation? }` returns the BSL script source text for the specified app: its definition's script files concatenated in order.
> If `generation` is provided, the script that was active at that generation is returned; otherwise the current generation's script is returned.
> The response contains the fields `script` (the source text), `files` (the paths of the script files, in order), and `generation` (the generation of the returned script).
> Multiple consecutive generations may share the same script content (for example, when intermediate generations are parameter changes); this is not surfaced specially in the response.

> i[app.bundle]
> `/apps/bundle { app, generation? }` returns the whole definition bundle of the specified app at `generation`, or at the current generation when it is omitted.
> The response contains the fields `generation`, `provenance` (the definition's [provenance](rule:i/definition.provenance)), and `bundle`, in the same encoding as a pushed [bundle](rule:i/definition.source).
> Every generation's bundle remains retrievable for as long as the app is registered.

## Generation History

<!-- from interface: Generation History -->

> i[generation.history]
> `/apps/generations { app, limit?, before? }` returns the most recent entries of the app's [generation history](rule:r/generation.history), in descending order of generation number.
> `limit` defaults to 50 and is capped at 200. `before`, when provided, restricts the response to entries with generation strictly less than the given value.
>
> Each entry in the response is an object with fields:
>
> - `generation`: the generation number.
> - `timestamp`: RFC 3339 timestamp.
> - `kind`: `"register" | "script_update" | "param_set" | "param_unset"`.
> - `definition`: present for `register` and `script_update`; the [provenance](rule:i/definition.provenance) of the definition this generation installed.
> - `param_name`: present for `param_set` and `param_unset`, and for a `script_update` that changed a parameter alongside its definition.
> - `previous_value`: present whenever `param_name` is; `null` if the parameter was unset before this entry, if the parameter is currently secret, or if the value has been redacted. See `redacted`.
> - `new_value`: present whenever `param_name` is; `null` if the entry unset the parameter, if the parameter is currently secret, or if the value has been redacted. See `redacted`.
> - `redacted`: boolean; `true` when the parameter named by `param_name` is currently secret, in which case `previous_value` and `new_value` are `null` regardless of whether values were recorded.
> - `script_changed`: boolean; `true` for `register` and `script_update`, otherwise `true` only if the script content for this generation differs from the immediately preceding generation. (For `param_set` / `param_unset`, this is always `false`.)
> - `operation_id`: identifier of the lifecycle operation triggered by this change, if any.
> - `outcome`: `"pending" | "succeeded" | "failed"`; `null` if no lifecycle operation was triggered. When `failed`, an `error` field carries a short description.
