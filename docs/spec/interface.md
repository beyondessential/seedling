The Seedling Operator Interface (OI) is the channel through which external actors observe and control a running Seedling instance.

Its consumers are human operators (via CLI or UI), agentic operators, and automation pipelines.
The OI is the exclusive mechanism for registering applications, transitioning them out of the uninstalled state, invoking lifecycle actions, changing parameter values, opening interactive shell sessions, and receiving the fault and event feed.

Absent specification bugs, anything that is not defined here is either defined in another spec document (the language spec, the runtime spec), or is implicitly not allowed.

# Transport

> i[transport.quic]
> The OI uses QUIC as its wire transport protocol.

> i[transport.local]
> In local operation mode, the OI endpoint listens on a loopback address configured at startup.
> The server authenticates using an RFC 7250 raw public key (SPKI).
> The server's key pair is generated at first startup and persisted to the data directory so that clients can pin the SPKI fingerprint across restarts.
> Clients verify the server by its SPKI fingerprint; certificate chain validation is not used.

> i[transport.remote]
> Remote operation mode — binding to a non-loopback address, with client authentication and PKI — is reserved for a future extension of this spec.
> Authentication and certificate verification requirements for remote mode are not defined here yet.

# Streams

> i[stream.control]
> Each request/response cycle uses one client-initiated bidirectional QUIC stream.
> The client writes the request JSON and half-closes its write-end.
> The server writes the response JSON and closes the stream.
> The stream boundary is the message boundary; no additional length framing is used.

> i[stream.shell]
> Each shell session uses three QUIC streams:
>
> - One client-initiated bidirectional stream (the _session stream_): carries the `OpenShell` handshake and, after the handshake, raw stdin bytes from client to server. The server uses its write-end to deliver the handshake response and a final exit frame when the session ends.
> - One server-initiated unidirectional stream for stdout bytes.
> - One server-initiated unidirectional stream for stderr bytes.

> i[stream.shell.framing]
> The server-to-client direction of the session stream carries newline-delimited JSON: the `OpenShell` response first, then the exit frame when the session ends.
> The client-to-server direction carries a single newline-terminated JSON request followed by raw stdin bytes for the remainder of the stream's lifetime.

> i[stream.forward]
> Each tunneled TCP connection within a port forward uses one client-initiated bidirectional QUIC stream.
> Both directions carry raw TCP bytes after an initial newline-terminated JSON header line: `{ "forward": "<forward_id>" }`.
> The stream closes when the tunneled TCP connection closes.

> i[datagram.forward]
> Each tunneled UDP datagram within a port forward is carried as a QUIC datagram (RFC 9221).
> Every datagram begins with a 2-byte big-endian `forward_key` followed immediately by the UDP payload.
> QUIC datagrams are path-MTU constrained; payloads that exceed the limit reported by `max_datagram_size()` cannot be delivered and are silently dropped.

> i[stream.dispatch]
> All client-initiated bidirectional streams begin with a newline-terminated JSON object.
> If that object contains a `"method"` key it is dispatched as a control request per [stream.control](#i--stream.control).
> If it contains a `"forward"` key it is dispatched as a port forward data stream per [stream.forward](#i--stream.forward).

> i[stream.events]
> After a client sends a `Subscribe` request, the server opens one server-initiated unidirectional QUIC stream per connection and pushes events as newline-delimited JSON objects for the duration of the connection.

# Wire Format

> i[wire.request]
> Every control request has the form:
> ```json
> { "method": "<string>", "params": { } }
> ```

> i[wire.response.ok]
> A successful response has the form:
> ```json
> { "result": { } }
> ```

> i[wire.response.error]
> An error response has the form:
> ```json
> { "error": { "code": "<string>", "message": "<string>" } }
> ```

> i[wire.error-codes]
> The following error code values are defined:
>
> | Code | Meaning |
> |---|---|
> | `not_found` | The referenced app, action, shell, session, or param does not exist. |
> | `not_installed` | The app is `NotInstalled` and the requested operation requires it to be installed. |
> | `already_installed` | `InvokeInstall` was called but the app is not `NotInstalled`. |
> | `operation_in_progress` | A lifecycle operation is running and the request conflicts with it. |
> | `already_queued` | An operation is already queued for this app. |
> | `requirements_invalid` | Install requirements failed validation; per-field errors are included in `message`. |
> | `script_error` | The BSL script failed to parse or evaluate; detail is included in `message`. |
> | `deregistering` | The app is in the `Deregistering` state. |

# App Management

> i[app.register]
> `RegisterApp { name, script }` evaluates the provided BSL script source text.
> On success, the app is added to the managed set in the `NotInstalled` state and an `AppRegistered` event is emitted.
> On script failure, `script_error` is returned and the app is not registered.

> i[app.deregister]
> `DeregisterApp { name }` initiates graceful teardown of all of the app's resources and removes the app from the managed set.
> If a lifecycle operation is in progress for the app, the request is rejected with `operation_in_progress`.
> Otherwise the app immediately enters the `Deregistering` state and an `AppDeregistered` event is emitted when teardown completes and the app is fully removed.

> i[app.update]
> `UpdateApp { name, script }` re-evaluates the provided BSL script source text.
> If a lifecycle operation is in progress, the new AppDef takes effect at the next evaluation boundary after the operation completes.
> If the script fails to parse or evaluate, a fault is filed, the existing AppDef continues running, and `FaultFiled` is emitted.
> On success, `AppUpdated` is emitted.

> i[app.list]
> `ListApps` returns an array of objects with fields `name` and `status`.

# App Status

> i[app.status]
> Every managed app is in exactly one of the following derived states at any time:
>
> - `NotInstalled`: the install action has never completed successfully for this app.
> - `Deregistering`: deregistration was requested and resource teardown is in progress.
> - `Operating`: a lifecycle operation is in progress. Includes the field `action_name`.
> - `Running`: steady state; no active faults; all resources are at their desired lifecycle states.
> - `Degraded`: steady state, but one or more resources are not at their desired lifecycle state or have an active fault.
> - `Faulted`: one or more active faults exist and at least one resource has been excluded from active reconciliation.

> i[app.status.priority]
> When multiple conditions apply simultaneously, the state with the highest priority is reported.
> Priority order, highest first: `Deregistering`, `Operating`, `NotInstalled`, `Faulted`, `Degraded`, `Running`.

# App Description

> i[app.describe]
> `DescribeApp { name }` returns a single object with the following fields:
>
> - `status`: the app's current status as defined in [app.status](#i--app.status).
> - `resources`: array of objects with fields `name`, `type`, `instances`, and `faults`.
>   Each instance has fields `id`, `display_name`, and `lifecycle_state`.
>   Each fault entry is a [fault record](#i--fault.record).
> - `params`: array of objects with fields `name` and `value`.
>   `value` is `null` if the param has not been set.
> - `actions`: array of objects with fields `name`, `description`, and `kind`.
>   `kind` is one of `action`, `shell`, or `install`.
> - `install_requirements`: an object map of requirement key to `{ kind, required, description, default_value }`, as defined in the language spec for install requirements.
>   Empty if the app has no explicit install action.
> - `current_operation`: present only when status is `Operating`.
>   Has fields `action_name` and `barrier`.
>   `barrier` is either `null` (operation is running but not yet at a barrier) or an object with fields `resources`, `required_state`, `deadline_secs`, and `elapsed_secs`.

# Param Management

> i[param.store]
> Param values are stored durably, keyed by `(app_name, param_name)`.
> They are restored into the script scope on every script evaluation.
> A param with no stored value is treated as absent.

> i[param.set]
> `SetParam { app, name, value }` persists the value and, if an `on_change` handler is registered for that param, schedules it as a lifecycle operation subject to the same concurrency rules as all lifecycle operations.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.

> i[param.unknown]
> Setting a param whose name does not appear in the app's current script evaluation is permitted.
> The value is stored and will take effect when the script is next evaluated.

# Action Invocation

> i[action.not-installed-gate]
> While an app is `NotInstalled`, all action and shell invocations except `InvokeInstall` are rejected with `not_installed`.

> i[action.invoke]
> `InvokeAction { app, name }` schedules the named action as a lifecycle operation.
> Shell actions must not be invoked via this method; `not_found` is returned if a shell name is provided.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.

> i[action.invoke.install]
> `InvokeInstall { app, requirements }` schedules the install action.
> It is only valid when the app is `NotInstalled`; otherwise `already_installed` is returned.
> `requirements` is an object map of requirement key to string value.
> If the app has no explicit install action, `requirements` must be absent or empty.
> Requirements are validated before the operation is enqueued; validation failure returns `requirements_invalid`.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.

> i[action.invoke.install.validation]
> Requirements are validated according to the kinds defined in the language spec before the operation is enqueued.
> A required field with no provided value and no `default_value` is a validation error.
> The requirements object is passed to the install action closure and discarded when the install operation completes; it is never persisted.

> i[action.invoke.install.completion]
> When an install operation completes successfully, the app transitions out of `NotInstalled`.
> On subsequent runtime restarts, the runtime will initiate the `start` action for this app automatically, as specified in the runtime spec.

# Shell Sessions

> i[shell.open]
> `OpenShell { app, name, rows, cols }` opens an interactive shell session.
> Returns `{ session_id, stdout_stream_id, stderr_stream_id }` as the handshake response on the session stream.
> After the handshake response is written, the server treats subsequent bytes on the session stream's client-to-server direction as raw stdin for the shell's job.

> i[shell.streams]
> Each session uses the three streams defined in [stream.shell](#i--stream.shell).
> `stdout_stream_id` and `stderr_stream_id` in the handshake response identify the server-initiated unidirectional streams the client must read for the session's output.

> i[shell.resize]
> `ResizeShell { session_id, rows, cols }` updates the terminal dimensions for the running session.
> Returns `{}` on success, or `not_found` if the session does not exist.

> i[shell.close]
> A session ends when any of the following occur:
>
> - The shell's Job terminates.
> - The client closes its write-end of the session stream (EOF on stdin).
> - The connection is lost.
>
> On session end, the server closes its write-ends of the stdout and stderr streams.

> i[shell.exit]
> When a session ends, the server writes a final JSON frame `{ "exit_code": <int> }` to the server-to-client direction of the session stream, then closes its write-end.
> Signal-terminated processes report a negative exit code.
> A `ShellExited` event is also emitted on the event feed.

> i[shell.cleanup]
> Dynamic resources created within a shell session are cleaned up by the runtime when the session ends, as specified in the runtime spec.

> i[shell.concurrent]
> Shell sessions may run concurrently with lifecycle operations and with other shell sessions.

# Port Forwards

> i[forward.request]
> `ForwardPort { app, service, port, proto }` requests a port forward to the named service at the given service-side port number.
> `service` is the name of a Service defined in the app's BSL script.
> `port` is a port number on that Service as defined by `service.port()`.
> `proto` is either `"tcp"` or `"udp"`.
> Returns `{ "forward_id": "<string>", "forward_key": <u16>, "max_udp_payload": <uint> | null }` on success.
> `forward_id` is used for control operations such as `StopForward`.
> `forward_key` is the compact 2-byte identifier used in QUIC datagram headers for UDP forwards (see [datagram.forward](#i--datagram.forward)); it is not used for TCP forwards.
> `max_udp_payload` is the maximum UDP payload size in bytes that can be delivered through this forward at the time of the request, as defined in [forward.mtu](#i--forward.mtu); it is `null` for TCP forwards.
> The control stream that carried the request is kept open for the lifetime of the forward; closing it tears down the forward.

> i[forward.mtu]
> For UDP forwards, `max_udp_payload` is computed as the connection's maximum datagram size minus the 2-byte `forward_key` prefix.
> Payloads exceeding this value cannot be delivered and are silently dropped.
> This value reflects the path MTU estimate at the time of the request; it may differ if queried again later as the estimate is refined.
> Clients may use this value to configure application-level buffer sizes (for example, DNS EDNS0 advertised payload size) or, for a TUN-based client, to set the TUN interface MTU so that the operating system enforces the limit transparently.

> i[forward.tunnel.tcp]
> Each individual TCP connection forwarded through a TCP port forward uses a dedicated bidi stream as defined in [stream.forward](#i--stream.forward).
> The server accepts the stream, opens a TCP connection to the target service address and port, and relays bytes bidirectionally until either end closes.

> i[forward.tunnel.udp]
> Each UDP datagram forwarded through a UDP port forward is carried as a QUIC datagram as defined in [datagram.forward](#i--datagram.forward).
> The server extracts the `forward_key`, looks up the target service address and port, and forwards the payload as a UDP datagram.
> Responses from the service are sent back as QUIC datagrams with the same `forward_key` prefix.

> i[forward.lifetime]
> A port forward remains active until any of the following occur:
>
> - The client closes the control stream.
> - The client sends `StopForward { forward_id }` on a new control stream.
> - The connection is lost.

> i[forward.stop]
> `StopForward { forward_id }` explicitly tears down an active port forward, closing all of its tunneled connections.
> Returns `{}` on success, or `not_found` if the forward does not exist.

> i[forward.concurrent]
> Multiple port forwards may be active concurrently, including to the same service.

# Fault Surface

> i[fault.record]
> A fault record contains the following fields: `id` (opaque string), `app`, `resource_type`, `resource_name`, `instance_id`, `kind`, `timestamp` (RFC 3339), and `description` (human-readable string).

> i[fault.list]
> `ListFaults { app? }` returns an array of currently active fault records.
> If `app` is provided, only faults for that app are returned; otherwise all active faults across all apps are returned.

> i[fault.derived]
> Faults are derived conditions.
> They clear automatically when the condition that caused them no longer holds.
> No acknowledgement mechanism is provided; fault resolution and incident tracking are left to external consumers of the event feed.

# Event Feed

> i[event.subscribe]
> `Subscribe {}` causes the server to open a server-initiated unidirectional stream and begin pushing events as newline-delimited JSON for the duration of the connection, as defined in [stream.events](#i--stream.events).

> i[event.types]
> The following event types are defined.
> Every event object includes a `type` field (string) and a `timestamp` field (RFC 3339).
>
> | `type` | Additional fields |
> |---|---|
> | `AppRegistered` | `app` |
> | `AppDeregistered` | `app` |
> | `AppUpdated` | `app` |
> | `OperationStarted` | `app`, `action_name`, `operation_id` |
> | `OperationCompleted` | `app`, `action_name`, `operation_id` |
> | `OperationFailed` | `app`, `action_name`, `operation_id`, `error` |
> | `FaultFiled` | all fault record fields |
> | `FaultCleared` | `id`, `app` |
> | `ResourceStateChanged` | `app`, `resource_type`, `resource_name`, `instance_id`, `state` |
> | `ShellExited` | `session_id`, `exit_code` |
> | `ForwardStarted` | `forward_id`, `app`, `service`, `port` |
> | `ForwardStopped` | `forward_id` |

> i[event.ordering]
> Events on a single connection's event stream are emitted in the order they occur.
> No ordering guarantee is made across separate connections.
