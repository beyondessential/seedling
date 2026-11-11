---
id: EVT
---

# Event feed

## Event Feed

<!-- from interface: Event Feed -->

> i[event.subscribe]
> `/events/subscribe {}` causes the server to open a server-initiated unidirectional stream and begin pushing events as newline-delimited JSON for the duration of the connection, as defined in [stream.events](rule:i/stream.events).

> i[event.types]
> The following event types are defined.
> Every event object includes a `type` field (string) and a `timestamp` field (RFC 3339).
> When an event is caused by an OI request, the event object also includes an `actor` field containing the actor resolved from that request (see [wire.actor](rule:i/wire.actor)).
> For events triggered autonomously by the runtime — such as `OperationStarted` with `trigger: "boot"` or `trigger: "schedule"` — the `actor` field is absent.
>
> | `type` | Additional fields |
> |---|---|
> | `AppRegistered` | `app`, `generation` |
> | `AppDeregistered` | `app` |
> | `AppUpdated` | `app`, `generation`, `previous_generation`, `definition` (the new definition's [provenance](rule:i/definition.provenance)); and `name`, `previous_value`, `new_value` when a parameter changed alongside |
> | `AppPhaseChanged` | `app`, `phase` |
> | `ParamSet` | `app`, `name`, `previous_value`, `new_value`, `generation`, `previous_generation` |
> | `ParamUnset` | `app`, `name`, `previous_value`, `generation`, `previous_generation` |
> | `OperationStarted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `trigger` |
> | `OperationCompleted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation` |
> | `OperationFailed` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `error` |
> | `FaultFiled` | all fault record fields |
> | `FaultCleared` | `id`, `app`, `kind` |
> | `ResourceStateChanged` | `app`, `resource_type`, `resource_name`, `instance_id`, `state` |
> | `ShellStarted` | `session_id`, `app`, `name` |
> | `ShellExited` | `session_id`, `exit_code` |
> | `ForwardStarted` | `forward_id`, `app`, `service`, `port` |
> | `ForwardStopped` | `forward_id` |
> | `ScaleChanged` | `app`, `deployment`, `scale`, `previous_scale`, `bounds_low`, `bounds_high` |
> | `ServerBusy` | `reason` |
> | `EventsDropped` | `skipped` |
> | `CanopyOffered` | `offer_id`, `agent`, `endpoint` |
> | `CanopyWithdrawn` | `offer_id`, `reason` |
>
> The `trigger` field on `OperationStarted` is a string indicating what caused the operation:
>
> - `"operator"`: manual action or install invocation.
> - `"boot"`: automatic start on runtime startup.
> - `"param_change"`: an `on_change` handler firing.
> - `"schedule"`: a BSL `on_schedule` cron fire.

> i[event.ordering]
> Events on a single connection's event stream are emitted in the order they occur.
> No ordering guarantee is made across separate connections.

> i[event.gap]
> When a subscriber falls far enough behind that the server skips events rather than delivering them, the server writes an `EventsDropped` event to that subscriber's stream at the point of the gap, with `skipped` giving the number of events skipped.
> `EventsDropped` is written only to the subscriber that fell behind.
> A subscriber that receives it treats any state it keeps current from the feed as stale and reads that state afresh.
