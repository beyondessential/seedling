---
id: FLTS
---

# Faults and restarts

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an operator reads and clears [faults](../overview.md#fault), and reads container restarts and the crash-loop settings.

## Fault records

A fault record is a JSON object.

| member | type | meaning |
| --- | --- | --- |
| `id` | string | the fault's identifier, opaque to the client |
| `app` | string | the app the fault belongs to |
| `resource_type` | string or null | the kind of thing the fault is about, where it is about less than the whole app |
| `resource_name` | string or null | the name of that thing |
| `instance_id` | string or null | the resource instance the fault is about, where it is about one instance |
| `kind` | string | the fault's [kind](../runtime/faults.md#fault-records) |
| `timestamp` | string | when the fault was filed, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `description` | string | text for the operator |

## Listing faults

`/faults/list` lists the active faults.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's faults |

The result is an array of [fault records](#fault-records).
Without `app` the server MUST list every active fault on the host.

## Clearing faults

`/faults/clear` clears every active fault of one app.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app whose faults to clear |

The result is `{ "app": <string>, "cleared": <integer> }`, where `cleared` is the number of faults that went from active to cleared.
The server MUST emit a `FaultCleared` event for each fault it clears.
Clearing a fault MUST NOT stop the runtime filing it again while its [filing condition](../runtime/faults.md#filing-and-clearing) holds.

## Restart records

A restart record is a JSON object describing one restart of a resource instance's container.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer | the record's identifier, increasing with each record |
| `app` | string | the app the instance belongs to |
| `instance_id` | string | the instance restarted |
| `resource_type` | string or null | the instance's resource kind |
| `resource_name` | string or null | the instance's resource name |
| `generation` | integer or null | the app's [generation](../overview.md#generation) at the time, or `null` where the app had none |
| `timestamp` | string | when the restart happened, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `cause` | string | `"recovery"` or `"deliberate"`, as [restart records](../runtime/autonomous.md#restart-record) define them |
| `exit_code` | integer or null | how the previous run ended, as `exit_kind` qualifies it, or `null` where unknown |
| `exit_kind` | string or null | `"exited"`, `"signalled"` or `"dumped"`, or `null` where unknown |

For `exit_kind` `"exited"`, `exit_code` MUST be the process's exit status.
For `"signalled"` and `"dumped"`, `exit_code` MUST be the number of the signal that ended the process.

## Listing restarts

`/restarts/list` lists restart records, most recent first.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's restarts |
| `instance` | string | no | restricts the list to that instance's restarts |
| `limit` | integer | no, default `100` | the most records to return |

The result is an array of [restart records](#restart-records).
The server MUST treat a `limit` below 1 as 1, and a `limit` above 1000 as 1000.

## Crash-loop settings

`/restarts/settings/get` reads, and `/restarts/settings/set` changes, the [crash-loop rate](../runtime/autonomous.md#crash-loop-detection) settings.

| member | type | meaning |
| --- | --- | --- |
| `threshold` | integer, at least 2 | the number of recovery restarts within the window that files a `crash_loop` fault |
| `window_secs` | integer, at least 60 | the window's length in seconds |

`/restarts/settings/get` takes no parameters, and its result is the settings object.
`/restarts/settings/set` takes either or both members, and its result is the whole settings object after the change.
The server MUST leave an omitted member unchanged.
The server MUST reject a `threshold` below 2 or a `window_secs` below 60 with `requirements_invalid`, and change neither setting.
