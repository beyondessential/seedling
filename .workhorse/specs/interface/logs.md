---
id: LOG
---

# Log streaming

## Log Streaming

<!-- from interface: Log Streaming -->

> i[logs.stream]
> `/logs/stream` opens a log stream for the specified target. The server acknowledges
> the request on the bidirectional stream and then delivers log entries on a
> server-initiated unidirectional stream as defined in [stream.logs](rule:i/stream.logs).

> i[logs.target]
> Exactly one of the following target selectors must be present in the request params:
>
> - `app` (string) — stream logs from workload containers belonging to the named app.
>   May be combined with `resource` (string) to restrict to a single resource name,
>   and further with `instance` (string) to restrict to a single instance display-name
>   suffix.
> - `infra` (string) — stream logs from an infrastructure component. Accepted values
>   are `"proxy"` and `"resolver"`.
>
> Providing both `app` and `infra`, or neither, is an error (`requirements_invalid`).
> Providing `resource` or `instance` without `app` is an error.
> Providing `instance` without `resource` is an error.

> i[logs.follow]
> The boolean param `follow` (default `false`) controls whether the stream remains
> open after historical entries are exhausted. When `true`, new log entries are pushed
> as they are written.

> i[logs.tail]
> The integer param `tail` (default `100`) controls how many historical log entries
> are delivered before switching to live entries (or closing the stream when follow
> is `false`). A value of `0` skips history entirely.

> i[logs.entry]
> Each log entry is a JSON object with at least the following fields:
>
> | Field | Type | Description |
> |---|---|---|
> | `timestamp` | string (RFC 3339, microsecond precision) | When the entry was recorded |
> | `message` | string | The log line content |
> | `unit` | string | The process supervision unit that produced the entry |
> | `stream` | string | `"stdout"` or `"stderr"` |
>
> For workload container logs the entry additionally includes `app` (string),
> `resource_kind` (string), `resource` (string), and `instance` (string).
> For infrastructure logs the entry includes `infra` (string).

> i[logs.not-found]
> If the `app` named in the request is not registered, the server returns `not_found`.
