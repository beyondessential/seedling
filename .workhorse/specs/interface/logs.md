---
id: LOG
---

# Log streaming

Applies to the [Operator Interface](../overview.md#operator-interface) method through which an operator reads the logs of an [app](../overview.md#app)'s workloads or of the host's infrastructure.

## Log stream request

`/logs/stream` is a [subscription request](wire.md#subscription-requests) that delivers [log entries](#log-entries).
On success the result is `{}`.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | one of `app` or `infra` | selects the logs of that app's workload containers |
| `resource` | string | no | with `app`, restricts the logs to the resource of that name |
| `instance` | string | no | with `app` and `resource`, restricts the logs to the instance with that display name |
| `infra` | string | one of `app` or `infra` | selects the logs of an infrastructure component, `"proxy"` or `"resolver"` |
| `follow` | boolean | no, default `false` | keeps the stream open for new entries once the history is delivered |
| `tail` | integer | no, default `100` | the number of most recent entries to deliver before any new ones |

The server MUST reject with `requirements_invalid` a request that carries both `app` and `infra`, or neither.
The server MUST reject with `requirements_invalid` a request that carries `resource` or `instance` without `app`, or `instance` without `resource`.
The server MUST reject an `infra` other than `"proxy"` or `"resolver"` with `requirements_invalid`.
The server MUST reject a request naming an app that is not registered with `not_found`.
If the server cannot open the host's log store, it MUST answer with `internal`.

## Entry selection

The server MUST first deliver the last `tail` entries that match the request, oldest first, or every matching entry where fewer exist.
A `tail` of `0` MUST deliver no history.
When `follow` is `false`, the server MUST finish the stream once the history is delivered.
When `follow` is `true`, the server MUST then deliver each new matching entry as it is written, until the client closes the connection.

The server MUST match `resource` and `instance` exactly against the entry's `resource` and `instance`.
An app's log stream MUST include the app's [breadcrumbs](../runtime/actuation.md#breadcrumb) interleaved in time order with its container output.

## Log entries

Each log entry is a JSON object.

| member | type | present | meaning |
| --- | --- | --- | --- |
| `timestamp` | string | always | when the entry was recorded, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp to the microsecond |
| `message` | string | always | the log line |
| `unit` | string | always | the process supervision unit that produced the entry |
| `stream` | string | always | `"stderr"` for an entry logged at error priority or higher, otherwise `"stdout"` |
| `app` | string | app entries | the app's name |
| `resource_kind` | string | app entries naming a resource | the resource's [type name](wire.md#resource-type-names) |
| `resource` | string | app entries naming a resource | the resource's name |
| `instance` | string | app entries naming an instance | the instance's display name |
| `infra` | string | infrastructure entries | `"proxy"` or `"resolver"` |
| `rt_call` | string | breadcrumbs | the `rt.*` method the breadcrumb records, or `unit_create` or `replay` |
| `script_pos` | string | breadcrumbs, where known | the script position of the call |

A client MUST ignore members it does not recognise.
