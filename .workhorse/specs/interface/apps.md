---
id: APPS
---

# App management

Applies to the methods by which operators register, inspect, update, uninstall and remove [apps](../overview.md#app), and retrieve their definitions and history.

## Borrowed terms

| term | meaning |
| --- | --- |
| timestamp | a date and time in the `date-time` format of [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) section 5.6, in UTC |

## Terminology

### App status

The single summary state the server reports for a registered app.

## Registration

`/apps/create` registers an app.
Its parameters are `app`, the app's name, and the members of a [definition source](definitions.md#definition-sources).

The server MUST refuse the request with `requirements_invalid` if `app` breaks the [naming rules](../language/overview.md#names), is a [reserved name](../runtime/identity.md#reserved-names), or names an app already registered.
The server MUST refuse a definition that cannot be fetched, breaks the bundle rules, or does not support the running Seedling version, with the error [definitions](definitions.md) gives for it.

Otherwise the server MUST register the app with the status `not_installed` and the [generation](../overview.md#generation) 1, recorded with a `register` [history entry](#generation-history), and MUST emit an `AppRegistered` [event](events.md#event-types).
The result is `{ "generation": 1 }`.

The server MUST register an app whose definition fails to parse or evaluate, and MUST file a [`script_error`](../runtime/faults.md#app-faults) fault against it.

A registration that fails for any reason MUST leave nothing observable behind.
The app MUST NOT appear in listings, MUST NOT reappear after the daemon restarts, and a later registration under the same name MUST NOT be refused as a duplicate.

## Persistence

The runtime MUST store every registered app, with each of its definitions and their [definition provenance](definitions.md#provenance-reporting), durably, and MUST load them again when the daemon starts.
If a stored definition fails to evaluate when it is loaded, the runtime MUST file a `script_error` fault against the app.

## Generations

Every registered app has a current generation.
The runtime MUST bump it on each successful `/apps/update`, and on each [parameter set](params.md#parameter-set) or [unset](params.md#parameter-unset) that is not a no-op.
The runtime MUST keep each generation's definition and parameter change durably for as long as the app is registered, as [generation history](../runtime/generation.md#history-entries) specifies.

## Update

`/apps/update` replaces an app's definition, and MAY change one parameter in the same step.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `script`, `bundle`, `reference`, `origin` | | | the new definition, as [definition sources](definitions.md#definition-sources) specifies |
| `param` | object | no | `{ "name": <string>, "value": <string or null> }`, where a string sets the parameter and `null` unsets it |

The result is `{ "schedule": "accepted" | "not_scheduled", "generation": <integer> }`, as for a [parameter set](params.md#parameter-set).

The server MUST refuse the request with `not_found` if the app is not registered.
It MUST refuse it with `operation_in_progress` if a lifecycle operation is running or queued for the app, an install of the app is in progress, or another change to the app is being applied.

The server MUST evaluate the new definition with the [proposed values](params.md#proposed-values), then validate those values against it.
If `param` gives the parameter's stored value, the update changes no parameter.

If the definition cannot be fetched, breaks the bundle rules, does not support the running Seedling version, fails to parse or evaluate, or fails validation, the server MUST refuse the request with the matching error and MUST leave everything observable as it was.
It MUST store no bundle and no parameter value, MUST NOT bump the generation, MUST NOT file or clear any fault, and the app MUST keep running its existing [resource graph](../overview.md#resource-graph).
Every piece of state derived from the definition MUST stay as it was, so that a refused update cannot be told apart from one never submitted.

> [!NOTE]
> State derived from the definition includes held volume data, clamped scaling decisions, port forwards to the app's services, and action schedules.

On success the server MUST record the new definition and the parameter change together in a single generation bump, with a `script_update` history entry.
It MUST then emit an `AppUpdated` [event](events.md#app-events).

If the update changes a parameter, the app is installed, and the new definition registers an `on_change` handler for the parameter that the change's [transition](../language/parameter.md#change-handlers) triggers, the server MUST schedule the handler as a [lifecycle operation](../overview.md#lifecycle-operation).
The handler MUST run under the new definition, with `old` being the previous generation.
An update that changes no parameter MUST NOT schedule a handler.

## Uninstall

`/apps/uninstall` tears down every resource of an installed app, as [ACI](actions.md#uninstall) specifies.

## Removal

`/apps/remove` takes an `app` parameter and removes a registered app that is not installed.

The server MUST refuse the request with `not_found` if the app is not registered.
It MUST refuse it with `operation_in_progress` if a lifecycle operation is running or queued for the app, an install of it is in progress, or a change to it is being applied.
It MUST refuse it with `requirements_invalid` if the app is installed or uninstalling, with a `message` saying which.

On success the server MUST discard the app's state as [deregistration](../runtime/app-teardown.md#deregistration) requires, MUST remove the app, and MUST emit an `AppDeregistered` event.
The result is `{}`.

## Listing

`/apps/list` takes no parameters and returns an array with one object per registered app.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the app's name |
| `status` | string | the app's [app status](#status-derivation) |
| `action_name` | string | present only when `status` is `operating`: the name of the action the running operation runs |
| `fault_count` | integer | the number of active faults filed against the app |
| `priority` | string | the app's [app priority](app-control.md#app-priority) |
| `has_stopped_resources` | boolean | whether any of the app's resources is [stopped](app-control.md#resource-stop) |
| `description` | string or null | the [app description](../language/app.md#app-description) its definition declares |

If the server cannot read the state any member reports, it MUST refuse the request with `internal`.

## Status derivation

An app's status is the first of these that applies:

1. `uninstalling`: the app is being torn down after an uninstall.
2. `installing`: an install of the app is in progress.
3. `operating`: the lifecycle operation running on the host is one of the app's; a queued operation does not count.
4. `not_installed`: the app is not installed.
5. `faulted`: at least one active [withholding fault](../runtime/faults.md#withholding-faults) is filed against the app.
6. `degraded`: at least one active fault is filed against the app, or at least one of the app's resources is not at its desired [lifecycle state](../overview.md#lifecycle-state).
7. `running`: none of the above.

A resource's desired lifecycle state is `Ready`, except that a [stopped](app-control.md#resource-stop) resource, and a deployment whose effective scale is zero, are desired at `Unscheduled`.

A resource desired at `Ready` is at its desired state when at least one of its instances is `Ready` and none is `Pending`, `Scheduled` or `Running`.
A resource desired at `Unscheduled` is at its desired state when none of its instances is `Pending`, `Scheduled`, `Running` or `Ready`.
A job is at its desired state when none of its instances is `Pending`, `Scheduled` or `Running`, none has run and failed, and at least one is `Ready` or has run and succeeded, success being judged as [ensured termination](../language/rt.md#termination) judges it.
An instance that is `Terminating`, `Terminated` or `Unscheduled` otherwise counts neither for nor against its resource.
A resource kind without a lifecycle state, such as an external service, is always at its desired state.

If the server cannot read the faults, stops, scaling decisions or observations an app's status rests on, it MUST NOT report a status for the app.

## Description

`/apps/show` takes an `app` parameter and describes that app.
The server MUST refuse it with `not_found` if the app is not registered.

| member | type | meaning |
| --- | --- | --- |
| `status` | string | the app's [app status](#status-derivation) |
| `generation` | integer | the app's current generation |
| `definition` | object | the [definition provenance](definitions.md#provenance-reporting) of the current definition |
| `description` | string or null | the app description its definition declares |
| `canopy_type` | string or null | the [Canopy application type](../language/app.md#canopy-type) its definition declares |
| `priority` | string | the app's [app priority](app-control.md#app-priority) |
| `faults` | array | the [fault records](faults.md#fault-records) of the app's active faults that concern no resource, such as a script error |
| `resources` | array | the app's declared [resources](#resources) |
| `dynamic_resources` | array | the [dynamic resources](#dynamic-resources) its lifecycle operations created |
| `stopped_resources` | array | the app's stopped resources, as [resource stop](app-control.md#resource-stop) specifies |
| `params` | array | the app's declared [parameters](#parameters) |
| `unknown_params` | array | its [unknown parameters](params.md#unknown-parameter) |
| `actions` | array | its [actions](#actions) |
| `current_operation` | object | present only while an operation runs for the app, as [current operation](#current-operation) specifies |

If the server cannot read the state a member reports, it MUST refuse the request with `internal`, rather than report the member empty, absent or `null`, except where this specification gives the member a value for that case.

### Resources

Each entry of `resources` has these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the resource's name |
| `type` | string | the resource's [type name](wire.md#resource-type-names) |
| `def` | object | the resource's declared configuration, below |
| `instances` | array | the resource's instances, below |
| `faults` | array | the fault records of the active faults concerning the resource |
| `stopped` | boolean | whether the resource is stopped |
| `scale` | object | for a deployment only, as [scaling](app-control.md#scaling) specifies |
| `priority` | string | for a deployment only, as [app priority](app-control.md#app-priority) specifies |
| `export` | object | for an exported volume only: `{ "exported": true }`, with a `description` member where the export declares one |

The server MUST report `instances` empty for an app whose status is `not_installed`.

Each instance has these members.

| member | type | meaning |
| --- | --- | --- |
| `id` | string | the instance's identity, in hexadecimal |
| `display_name` | string | the instance's name, for a person to read |
| `lifecycle` | string | its lifecycle state: `Pending`, `Scheduled`, `Running`, `Ready`, `Terminating`, `Terminated` or `Unscheduled` |
| `transition_time` | string or null | the timestamp at which it entered that state |
| `restarts` | object or null | a summary of its [restart history](faults.md#restart-records), or `null` where it has none or has no container |
| `health` | string or null | its health check state: `starting`, `passing` or `failing`, as below |

`restarts` has the members `recent`, the number of recovery restarts within the current [rate window](faults.md#crash-loop-settings); `window_secs`, that window's length in seconds; `total`, the number of retained restart records; and `last_at`, `last_exit_code` and `last_exit_kind`, describing the most recent record.

`health` reports the state of the [health check](../language/deployment.md#health-checks) of a Deployment instance whose Deployment declares one.
It is `failing` once the check has failed `retries` consecutive times outside its `start_period`, until the check next passes.
Otherwise it is `passing` once a check has passed since the instance's container started, and `starting` before then.
`health` MUST be `null` for an instance of any other resource, for an instance with no running container, and for an instance whose health check has not been observed since its container started.
Whenever an instance's `health` changes, the runtime MUST emit an `InstanceHealthChanged` [event](events.md#fault-and-resource-events).

`def` has a `kind` member, and the other members its kind gives.

| kind | members |
| --- | --- |
| `service` | `http` (boolean), `exported` (boolean), `export_description` (string or null), `description` (string or null), and the [proxy settings](#proxy-settings) `balance` and `routes` |
| `ingress` | `service` (string), `hostname` (string), `port` (integer), `tls` (boolean), `dtls` (boolean), `http_terminate` (`"http1"`, `"http2"` or null), `redirect` (`{ "port": <integer>, "code": <integer> }` or null), `description` |
| `deployment` | `container`, `pod`, `scale` (`{ "low": <integer>, "high": <integer> }`), `on_update` (`"rolling"` or `"replace"`), `on_terminate` (`"recreate"`), `priority` (string), `description` |
| `job` | `container`, `pod`, `deadline` (seconds, or null), `description` |
| `volume` | `readonly` (boolean), `tmpfs` (boolean), `writes` (object of path to contents), `exported` (boolean), `export_description` (string or null), `description` |
| `external_volume` | `description` |
| `external_service` | `description` |

A `container` object has these members.

| member | type | meaning |
| --- | --- | --- |
| `image` | string or null | the image reference |
| `command`, `args` | array of strings or null | the entrypoint and its arguments |
| `env` | object | each environment variable's name and value, or `null` for a [redacted](#redacted-environment-values) value |
| `volume_mounts` | object | for each mount path, `{ "kind": "volume", "name": <string or null> }` or `{ "kind": "external_volume", "name": <string> }` |
| `on_exit` | string | `"default"`, `"restart"`, `"terminate"` or `"restart_on_failure"` |
| `memory` | string or null | the memory limit |
| `cpus` | number or null | the CPU limit |
| `extra_caps` | array of strings | added capabilities |
| `writable_rootfs` | boolean | whether the root filesystem is writable |
| `pids_limit` | integer or null | the process limit |
| `workdir`, `user` | string or null | the working directory and user |
| `healthcheck` | object or null | `kind` (`"command"`), `cmd` (array of strings), `interval_secs`, `timeout_secs`, `retries`, `start_period_secs`, and `on_failure` (`"replace"` or `"monitor"`) |
| `stop_signal` | string or null | the stop signal |
| `stop_timeout_secs` | integer or null | the stop timeout |

A `pod` object has the members `service_mounts`, each `"<service>:<port>"`; `http_bindings`, each `"<pod port> -> <service> <prefix>"`; and `tcp_bindings` and `udp_bindings`, each `"<pod port> -> <service>:<port>"`.
Each is an array of strings in ascending order.

### Redacted environment values

The server MUST report as `null` the value of an environment variable whose value contains the stored value of a parameter whose effective [secret flag](../language/parameter.md#secret-parameters) is true.

### Proxy settings

A service's `balance` and `routes` report the proxy settings in force on it, after the [resolution](../language/http-service.md#resolution) of route-level settings against service-level ones.
The server MUST report every setting resolved, so that a setting the app declared at neither level is reported at its default rather than as `null` or absent.

`balance` reports the service-wide balancing policy, which governs every backend pool the service is served through.
It is an object with members `policy` (string), `try_duration` and `interval`, both in seconds.

`routes` is `null` for a service without HTTP.
Otherwise it holds one entry per HTTP route the service is served through, in ascending order of prefix, each reporting the settings that route resolved to.
A service that declares no route MUST report one route with the prefix `/`, so that `routes` is never empty for an HTTP service.

| member | type | meaning |
| --- | --- | --- |
| `prefix` | string | the route's path prefix |
| `served` | boolean | whether the proxy is told to serve the route |
| `compress` | object or null | `null` when compression is off for the route; otherwise `encodings` (array of strings), `minimum_length` (integer) and `content_types` (array of strings) |
| `balance` | object | as the service's `balance` |
| `rate_limit` | object or null | `null` when the route is not rate limited; otherwise `max_events` (integer), `window` (seconds) and `shared` (boolean) |
| `headers` | object | `request` and `response`, each with `replace` and `add` (objects of header name to array of values) and `remove` (array of header names) |
| `redirect` | object or null | `null` for a route proxied to a pod; otherwise `to` (string) and `code` (integer), describing a [route redirect](../language/http-service.md#route-redirects) |

A route is `served` when a pod binds its prefix, when it is a redirect, or when no pod binds any prefix of the service.

`shared` MUST be true when the rate limit was declared on the service, so that the route counts against a budget common to every route inheriting it.

The server MUST report every header value as an array, whichever form the app declared it in.
A direction with no operations MUST report its three members empty, not `null`.
Header operations resolve per header name, so a route reports the operations in force on it whether the route or the service declared them.

A redirect's `to` is the target as the app declared it, with the parts of the request that carry over still written as tokens.
A redirect route MUST report `compress` and `rate_limit` as `null` and its request header operations empty.
Its response header operations MUST NOT include `Location`, which the redirect sets itself.
Its `balance` is the service's.

> [!NOTE]
> A declared prefix that no pod binds reports its settings without carrying them.
> Requests under it are answered by whichever served prefix matches, under that route's settings, so a rate limit on an unserved route is not in force.

### Dynamic resources

A dynamic resource is one a lifecycle operation's action closure created, rather than one the definition declares.
The server MUST report dynamic resources only for an app whose status is not `not_installed`.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the resource's name, or its instance's display name where it is anonymous |
| `type` | string | as for a declared resource |
| `anonymous` | boolean | whether the closure created it without a name |
| `operation_id` | string | the operation that created it |
| `description` | string or null | the [description](../language/overview.md#descriptions) the closure gave it |
| `instances` | array | its single instance, with the members of a declared resource's instance except `restarts` |
| `faults` | array | the fault records of the active faults concerning its instance |

### Parameters

Each entry of `params` has these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the parameter's name |
| `kind` | string | `text`, `multiline`, `email`, `password`, `weak-password`, `random` or `volume` |
| `required` | boolean | whether the parameter is required |
| `description` | string or null | its description |
| `default_value` | string or null | its default value |
| `secret` | boolean | its effective [secret flag](../language/parameter.md#secret-parameters) |
| `is_set` | boolean | whether it has a stored value |
| `value` | string or null | its stored value, or `null` where it is unset or secret |

When a parameter's effective secret flag is true, its `value` MUST be `null` whether or not a value is stored.
A client MUST use `is_set` to tell an unset secret from a set one.

Each entry of `unknown_params` has the members `name` and `value`.
`value` MUST be `null` for a value held in secret storage.
An unknown parameter's value has no effect until the definition declares the parameter.

### Actions

Each entry of `actions` has these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the action's name |
| `description` | string or null | its description |
| `kind` | string | `action`, `lifecycle`, `shell` or `install` |
| `params` | object | for each parameter its schema declares, `{ "kind", "required", "description", "default_value", "secret" }` as in `params` above |
| `schedules` | array | one `{ "cronexpr", "last_fired_at", "next_fire_at" }` for each [schedule](../language/action.md#scheduled-actions) attached to the action |

The start action has the kind `lifecycle`, since the runtime drives it and an operator cannot invoke it.
A definition's install action is reported with the name `install`, the kind `install`, a `null` description, and its install requirements as `params`.
Shells and the install action have no schedules.

`last_fired_at` is the timestamp at which the runtime last fired the schedule, or `null` if it has never fired.
`next_fire_at` is the timestamp at which the schedule is next due, computed from `last_fired_at` or, where that is `null`, from the current time.
`next_fire_at` MUST be `null` only when the schedule's expression cannot be evaluated.

### Current operation

`current_operation` MUST be present when the app's status is `operating` or `installing` and an operation is running for it, and absent otherwise.
An app being uninstalled has no operation to report.

| member | type | meaning |
| --- | --- | --- |
| `action_name` | string | the name of the action the operation runs |
| `barrier` | object or null | the barrier the operation is waiting on, or `null` while its closure runs |
| `source_generation` | integer | the generation the operation started from |
| `target_generation` | integer | the generation it moves the app to |

`barrier` MUST describe the earliest unsatisfied barrier recorded in the operation's action log.
It MUST be `null` while the closure runs between barriers.

| member | type | meaning |
| --- | --- | --- |
| `resources` | array of strings | the display names of the resources the barrier awaits |
| `required_state` | string | the lifecycle state it awaits, such as `Ready` or `Terminated` |
| `deadline_secs` | integer or null | its deadline in seconds, or `null` for a barrier [without one](../language/rt.md#barriers) |
| `elapsed_secs` | integer | the seconds since the barrier first suspended the operation |

`elapsed_secs` MUST be measured from the time the barrier record holds, so that it counts across daemon restarts.

## Definition retrieval

`/apps/script` returns an app's script.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `generation` | integer | no | the generation whose script to return; the current generation when absent |

The result has the members `script`, the app's script [formed](../language/bundle.md#script-formation) from that generation's definition; `files`, the bundle paths of its script files in order; and `generation`.
The server MUST refuse the request with `not_found` if the app is not registered or has no such generation, and with `bundle_invalid` if the definition's script files break the bundle rules.

`/apps/bundle` takes the same parameters and returns an app's whole definition bundle.
The result has the members `generation`; `provenance`, the definition provenance; and `bundle`, in the form of a pushed `bundle` [definition source](definitions.md#definition-sources).
Every generation's bundle MUST remain retrievable for as long as the app is registered.

## Generation history

`/apps/generations` lists entries of an app's generation history, newest first.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `limit` | integer | no | the most entries to return: 50 when absent, and clamped to the range 1 to 200 |
| `before` | integer | no | return only entries whose generation is less than this |

The server MUST refuse the request with `not_found` if the app is not registered.

| member | type | meaning |
| --- | --- | --- |
| `generation` | integer | the generation |
| `timestamp` | string | the timestamp of the change |
| `kind` | string | `register`, `script_update`, `param_set` or `param_unset` |
| `definition` | object | present for `register` and `script_update`: the definition provenance of the definition the generation installed |
| `param_name` | string | present for `param_set` and `param_unset`, and for a `script_update` that changed a parameter |
| `previous_value` | string or null | present when `param_name` is and `redacted` is not: the value before the change, or `null` where the parameter was unset |
| `new_value` | string or null | present when `param_name` is and `redacted` is not: the value after the change, or `null` where the change unset it |
| `redacted` | boolean | present, and true, when the values are withheld |
| `script_changed` | boolean | true for `register` and `script_update`, false otherwise |
| `operation_id` | string or null | the lifecycle operation the change triggered, if any |
| `outcome` | string or null | `pending`, `succeeded`, `cancelled` or `failed` for a triggered operation, `null` where none was triggered |
| `error` | string | present when `outcome` is `failed` and a cause was recorded: a short description of it |

The server MUST withhold both values of an entry whose parameter is secret under the current definition, or whose value was held in secret storage when the change was recorded.
