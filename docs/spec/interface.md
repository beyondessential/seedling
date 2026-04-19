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

> i[transport.listen]
> The server may be configured to listen on one or more addresses at startup.
> All configured addresses share the same server identity (key pair and SPKI fingerprint) and the same authorized key set.
> When no addresses are explicitly configured, the server listens on a single loopback address on the default port.

> i[transport.remote]
> Remote operation mode — binding to a non-loopback address, with client authentication and PKI — is reserved for a future extension of this spec.
> Authentication and certificate verification requirements for remote mode are not defined here yet.

> i[transport.fingerprint-probe]
> When a client connects to a server whose fingerprint is not yet in its known-hosts store, it must
> first capture the server's SPKI fingerprint without revealing its real identity to the server.
> The probe connection must present a raw public key as its mTLS client certificate, but the key
> used must be a freshly-generated, single-use key that is discarded immediately after the probe.
> The server will reject the probe connection (the ephemeral key is not authorised), but the
> server's SPKI fingerprint is captured during the TLS handshake before that rejection occurs.
> After capturing the fingerprint the client must confirm it with the user before proceeding with
> an authenticated connection using the real client identity.
> The probe connection must be structurally indistinguishable from a normal authenticated
> connection: a network observer or the server itself cannot determine whether a given connection
> is a probe or a real session.

> i[transport.client-auth]
> Every client connection must present a raw public key (RFC 7250 SPKI) as its mTLS certificate.
> The server maintains a set of authorized client SPKI fingerprints in persistent storage.
> A connection whose client certificate fingerprint is not in the authorized set is rejected at the TLS layer.
>
> On startup the server reads `$data_dir/authorized_keys` and imports any entries not already in
> persistent storage. Each non-comment line in that file has the form:
> ```
> <sha256-hex-fingerprint> <label>
> ```
> This file is the initial bootstrap mechanism: an operator with write access to the data directory
> can authorize a client key without requiring a prior authenticated connection.

# Streams

> i[stream.control]
> Each request/response cycle uses one client-initiated bidirectional QUIC stream.
> The client writes the request JSON and half-closes its write-end.
> The server writes the response JSON and closes the stream.
> The stream boundary is the message boundary; no additional length framing is used.

> i[stream.shell]
> Each shell session uses three QUIC streams:
>
> - One client-initiated bidirectional stream (the _session stream_): carries the `/shells/start` handshake and, after the handshake, raw stdin bytes from client to server. The server uses its write-end to deliver the handshake response and a final exit frame when the session ends.
> - One server-initiated unidirectional stream for stdout bytes.
> - One server-initiated unidirectional stream for stderr bytes.

> i[stream.shell.framing]
> The server-to-client direction of the session stream carries newline-delimited JSON: the `/shells/start` response first, then the exit frame when the session ends.
> The client-to-server direction carries a single newline-terminated JSON request followed by raw stdin bytes for the remainder of the stream's lifetime.

> i[stream.forward]
> Each tunneled TCP connection within a port forward uses one client-initiated bidirectional QUIC stream.
> Both directions carry raw TCP bytes after an initial newline-terminated JSON header line: `{ "forward": "<forward_id>" }`.
> The stream closes when the tunneled TCP connection closes.

> i[datagram.forward]
> Each tunneled UDP datagram within a port forward is carried as a QUIC datagram (RFC 9221).
> Every datagram begins with a 2-byte big-endian `forward_key` followed immediately by the UDP payload.
> QUIC datagrams are path-MTU constrained; payloads that exceed the limit are dropped and reported via a status message (see [forward.status](#i--forward.status)).

> i[stream.dispatch]
> All client-initiated bidirectional streams begin with a newline-terminated JSON object.
> If that object contains a `"method"` key it is dispatched as a control request per [stream.control](#i--stream.control).
> If it contains a `"forward"` key it is dispatched as a port forward data stream per [stream.forward](#i--stream.forward).

> i[stream.events]
> After a client sends a `/events/subscribe` request, the server opens one server-initiated unidirectional QUIC stream per connection and pushes events as newline-delimited JSON objects for the duration of the connection.

> i[stream.logs]
> After a client sends a `/logs/stream` request, the server opens one server-initiated unidirectional QUIC stream and pushes log entries as newline-delimited JSON objects. When follow mode is not requested, the stream closes after historical entries are exhausted. When follow mode is requested, the stream remains open and new entries are pushed as they appear until the client drops the connection.

> i[stream.concurrency-limit]
> The server enforces a configurable upper bound on the number of concurrently active request/response streams across all connections.
> Long-lived sessions (forwards, shells, event subscriptions, log streams) release the permit after initial dispatch and do not count against the limit for their lifetime; the limit exists to bound the memory used by concurrent request body reads.
> When the limit is reached, the server immediately replies with a `server_busy` error and closes the stream.
> The limit is configurable at startup; the default value is 64.

# Wire Format

> i[wire.request]
> Every control request has the form:
> ```json
> { "method": "<string>", "params": { } }
> ```

> i[wire.actor]
> Every control request must include a top-level `actor` object identifying the human or system principal that initiated the request:
> ```json
> { "method": "<string>", "actor": { "kind": "<string>", "id": "<string>", "display": "<string>", "session": "<string>" }, "params": { } }
> ```
> All fields within `actor` are optional strings.
> `kind` identifies the authentication mechanism (e.g. `"ctl"`, `"password"`, `"tailscale"`, `"dev"`).
> `id` is a stable identifier for the principal (e.g. an email address or key fingerprint).
> `display` is a human-readable label.
> `session` is an opaque per-session correlator.
>
> When the `actor` field is absent, the server synthesises one from the client's mTLS identity as a fallback: `kind` is `"ctl"`, `id` is the client's SPKI fingerprint, and `display` is the label stored in the authorized keys table for that fingerprint (or the fingerprint itself if no label is stored).
>
> The resolved actor is included in all events emitted as a result of the request and recorded in the audit log.

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
> | `already_installed` | `/apps/install/invoke` was called but the app is not `NotInstalled`. |
> | `operation_in_progress` | A lifecycle operation is running and the request conflicts with it. |
> | `already_queued` | An operation is already queued for this app. |
> | `requirements_invalid` | Install requirements failed validation; per-field errors are included in `message`. |
> | `script_error` | The BSL script failed to parse or evaluate; detail is included in `message`. |
> | `deregistering` | The app is in the `Deregistering` state. |
> | `unauthorized` | The client's key is not in the authorized set, or the operation is not permitted. |
> | `server_busy` | The server's stream concurrency limit has been reached; the client should retry after a delay. |

# Status

> i[status.get]
> `/server/status` returns a summary of the running Seedling instance.
> It must always succeed and must not perform any expensive computation.
> The response contains the following fields:
>
> - `version`: the Seedling version string.
> - `uptime_secs`: the number of seconds since the Seedling process started.
> - `spki_fingerprint`: the SHA-256 fingerprint (hex-encoded) of the server's raw public key, allowing clients to verify or record the identity of the instance they are connected to.
> - `apps_total`: total number of registered apps.
> - `apps_by_status`: an object map of status name to count, covering only statuses with a non-zero count.
> - `active_operations`: number of lifecycle operations currently in progress.
> - `active_faults`: number of currently active faults across all apps.
> - `active_shells`: number of open shell sessions.
> - `active_forwards`: number of active port forwards.

# App Management

> i[app.register]
> `/apps/create { app, script }` evaluates the provided BSL script source text.
> On success, the app is added to the managed set in the `NotInstalled` state and an `AppRegistered` event is emitted.
> On script failure, `script_error` is returned and the app is not registered.

> i[app.persist]
> Registered apps and their BSL scripts are stored durably and reloaded automatically on restart.

> i[app.deregister]
> `/apps/remove { app }` initiates graceful teardown of all of the app's resources and removes the app from the managed set.
> If a lifecycle operation is in progress for the app, the request is rejected with `operation_in_progress`.
> Otherwise the app immediately enters the `Deregistering` state and an `AppDeregistered` event is emitted when teardown completes and the app is fully removed.

> i[app.update]
> `/apps/update { app, script }` re-evaluates the provided BSL script source text.
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`.
> If the script fails to parse or evaluate, a `script_error` app-level fault is filed, the existing AppDef continues running, and the request still succeeds.
> On success, any previously active `script_error` fault for this app is cleared, and the app's [generation](#r--generation.definition) is bumped with a `ScriptUpdate` history entry.

> i[app.generation]
> Every registered app has a current [generation](#r--generation.definition) — a per-app monotonic integer identifying the app's defined state at a point in time.
> The generation is bumped on initial registration, on each successful `/apps/update`, and on each successful parameter set or unset.
> Previous generations are retained durably so that operators and automation can retrieve any historical state of the app.

> i[app.list]
> `/apps/list` returns an array of objects with fields `name` and `status`.

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
> `/apps/show { app }` returns a single object with the following fields:
>
> - `status`: the app's current status as defined in [app.status](#i--app.status).
> - `faults`: array of app-level [fault records](#i--fault.record) not associated with a specific resource instance (e.g. script evaluation errors). Empty when there are no active app-level faults.
> - `resources`: array of objects with fields `name`, `type`, `instances`, `faults`, and for Deployment resources, `scale`.
>   Each instance has fields `id`, `display_name`, `lifecycle`, and `transition_time` (RFC 3339, optional).
>   Each fault entry is a [fault record](#i--fault.record).
> - `params`: array of objects with fields `name` and `value`.
>   `value` is `null` if the param has not been set.
> - `unknown_params`: array of objects with fields `name` and `value`, listing parameters that have a stored value in the database but whose name does not appear in the app's current script evaluation. This is informational only; these values have no effect until the script is updated to reference them.
> - `actions`: array of objects with fields `name`, `description`, and `kind`.
>   `kind` is one of `action`, `shell`, or `install`.
> - `install_requirements`: an object map of requirement key to `{ kind, required, description, default_value }`, as defined in the language spec for install requirements.
>   Empty if the app has no explicit install action.
> - `current_operation`: present only when status is `Operating`.
>   Has fields `action_name`, `barrier`, `source_generation`, and `target_generation`.
>   `barrier` is either `null` (operation is running but not yet at a barrier) or an object with fields `resources`, `required_state`, `deadline_secs`, and `elapsed_secs`.
> - `generation`: the current generation of the app.

# Scaling

> i[scale.set]
> `/apps/scale { app, deployment, scale }` sets the running scale of a single Deployment within an installed app.
> `scale` is a non-negative integer. The value is clamped to the deployment's declared bounds; requests that would move outside the bounds succeed but stay at the boundary.
> The app must be registered and the named deployment must exist in the current AppDef; otherwise `not_found` is returned.
> On success, the response contains `scale` (the new scale value) and `bounds` with `low` and `high`.

> i[scale.decision-persistence]
> The effective scale chosen by `/apps/scale` is stored durably and survives process restarts.
> On startup, the stored decision is loaded and used as the effective scale for the deployment.

> i[scale.describe]
> `/apps/show` includes, for each Deployment resource, a `scale` object with fields `low` (lower bound), `high` (upper bound), and `current` (the effective scale).
> `current` is the stored scaling decision clamped to the declared bounds, or the lower bound if no decision has been stored.

# App Script Retrieval

> i[app.script]
> `/apps/script { app, generation? }` returns the BSL script source text for the specified app.
> If `generation` is provided, the script that was active at that generation is returned; otherwise the current generation's script is returned.
> The response contains the fields `script` (the source text) and `generation` (the generation of the returned script).
> Multiple consecutive generations may share the same script content (for example, when intermediate generations are parameter changes); this is not surfaced specially in the response.

# Generation History

> i[generation.history]
> `/apps/generations { app, limit?, before? }` returns the most recent entries of the app's [generation history](#r--generation.history), in descending order of generation number.
> `limit` defaults to 50 and is capped at 200. `before`, when provided, restricts the response to entries with generation strictly less than the given value.
>
> Each entry in the response is an object with fields:
>
> - `generation`: the generation number.
> - `timestamp`: RFC 3339 timestamp.
> - `kind`: `"register" | "script_update" | "param_set" | "param_unset"`.
> - `param_name`: present for `param_set` and `param_unset`.
> - `previous_value`: present for `param_set` and `param_unset`; `null` if the parameter was unset before this entry, otherwise a string.
> - `new_value`: present for `param_set` and `param_unset`; `null` for `param_unset`, otherwise a string.
> - `script_changed`: boolean; `true` for `register` and `script_update`, otherwise `true` only if the script content for this generation differs from the immediately preceding generation. (For `param_set` / `param_unset`, this is always `false`.)
> - `operation_id`: identifier of the lifecycle operation triggered by this change, if any.
> - `outcome`: `"pending" | "succeeded" | "failed"`; `null` if no lifecycle operation was triggered. When `failed`, an `error` field carries a short description.

# Change Planning

> i[plan.dry-run]
> `/apps/plan { app, proposed_script?, proposed_params? }` evaluates a hypothetical change against the app's current generation and returns a structured diff.
> Neither the script nor the parameter values stored on the server are modified by this call.
>
> Parameters:
>
> - `proposed_script`: optional BSL script source text to evaluate in place of the current script. If omitted, the current script is used.
> - `proposed_params`: optional array of `{ name, value }` objects. `value` is `null` to model an unset; a string to model a set. Parameters not listed are taken from the current parameter map. If both `proposed_script` and `proposed_params` are omitted, the response is an empty diff.
>
> The response contains:
>
> - `diff`: an array of resource diff entries, each with fields `resource_type`, `resource_name`, and `change` (`"added" | "removed" | "modified"`). For `modified` entries, a `fields` array lists the resource attributes that differ between current and proposed.
> - `on_change_would_fire`: an array of parameter names whose `on_change` handlers would be scheduled if the proposed change were committed.
> - `errors`: an array of evaluation errors, if the proposed script fails to evaluate. When present, `diff` and `on_change_would_fire` are absent.
>
> The dry-run does not simulate the execution of `on_change` handlers or any action closures; it reports only the static diff and which handlers would be triggered.

# Param Management

> i[param.store]
> Param values are stored durably, keyed by `(app_name, param_name)`.
> They are restored into the script scope on every script evaluation.
> A param with no stored value is treated as absent.

> i[param.set]
> `/apps/params/set { app, name, value }` persists the value and bumps the app's [generation](#r--generation.definition) with a `ParamSet` history entry. If the change matches one of the [transitions](#l--param.on-change.transitions) defined in the language spec, an `on_change` handler is registered for that param, and the app is installed, the handler is scheduled as a lifecycle operation.
>
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`; neither the value nor the generation is changed.
>
> If the requested value is equal to the current value, the request is a no-op: nothing is persisted, no generation bump occurs, and no handler is scheduled.
>
> The script is re-evaluated after the value is persisted; if evaluation fails, a `script_error` app-level fault is filed and the request still succeeds.
>
> Returns `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }` on success, or an error. `not_scheduled` means the generation was bumped but no `on_change` handler ran (for example, no handler is registered for the parameter, or the app is not installed). The returned `generation` is the app's current generation after the call (unchanged if the call was a no-op).

> i[param.unknown]
> Setting a param whose name does not appear in the app's current script evaluation is permitted.
> The value is stored and will take effect when the script is next evaluated.

> i[param.unset]
> `/apps/params/unset { app, name }` removes the stored value for the named parameter and reloads the script.
>
> If a lifecycle operation is in progress for the app, or one is queued, the request is rejected with `operation_in_progress`; neither the value nor the generation is changed.
>
> If the parameter has no stored value, the request is a no-op: nothing is changed, no generation bump occurs, and no handler is scheduled.
>
> Otherwise the app's [generation](#r--generation.definition) is bumped with a `ParamUnset` history entry. If an `on_change` handler is registered for the parameter and the app is installed, the handler is scheduled as a lifecycle operation.
>
> The script is re-evaluated after the value is removed; if evaluation fails, a `script_error` app-level fault is filed and the request still succeeds.
>
> Returns `{ "schedule": "accepted" | "not_scheduled", "generation": <int> }` on success, or an error.

# Action Invocation

> i[action.not-installed-gate]
> While an app is `NotInstalled`, all action and shell invocations except `/apps/install/invoke` are rejected with `not_installed`.

> i[action.invoke]
> `/apps/action/invoke { app, name, params? }` schedules the named action as a lifecycle operation.
> `params` is an optional JSON object. Keys ending in `_volume` are reserved and must be rejected.
> Shell actions must not be invoked via this method; `not_found` is returned if a shell name is provided.
> Returns `{ "schedule": "accepted", "operation_id": "<string>" }` or `{ "schedule": "queued", "operation_id": "<string>" }` on success, or an error. The `operation_id` is always present and uniquely identifies this operation.

> i[action.invoke.install]
> `/apps/install/invoke { app, requirements? }` schedules the install action.
> It is only valid when the app is `NotInstalled`; otherwise `already_installed` is returned.
> `requirements` is an optional JSON object of requirement key to string value. The values are delivered to the install closure as `param`.
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
> `/shells/start { app, name, rows, cols, params? }` opens an interactive shell session.
> `params` is an optional JSON object. Keys ending in `_volume` are reserved and must be rejected.
> Returns `{ session_id, stdout_stream_id, stderr_stream_id }` as the handshake response on the session stream.
> After the handshake response is written, the server treats subsequent bytes on the session stream's client-to-server direction as raw stdin for the shell's job.

> i[shell.streams]
> Each session uses the three streams defined in [stream.shell](#i--stream.shell).
> `stdout_stream_id` and `stderr_stream_id` in the handshake response identify the server-initiated unidirectional streams the client must read for the session's output.

> i[shell.resize]
> `/shells/resize { session_id, rows, cols }` updates the terminal dimensions for the running session.
> Returns `{}` on success, or `not_found` if the session does not exist.

> i[shell.record]
> A shell record contains the following fields: `session_id`, `app`, `name`, and `opened_at` (RFC 3339).

> i[shell.list]
> `/shells/list { app? }` returns an array of shell records for all currently active shell sessions.
> If `app` is provided, only sessions for that app are returned; otherwise all active sessions across all apps are returned.

> i[shell.stop]
> `/shells/stop { session_id }` forcibly terminates an active shell session.
> Any operator may stop any session regardless of which connection opened it.
> The session ends as per [shell.close](#i--shell.close), with the job terminated and dynamic resources cleaned up.
> Returns `{}` on success, or `not_found` if the session does not exist.

> i[shell.close]
> A session ends when any of the following occur:
>
> - The shell's Job terminates.
> - The client closes its write-end of the session stream (EOF on stdin).
> - The connection is lost.
> - An operator calls `/shells/stop`.
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
> `/forwards/start { app, service, port, proto }` requests a port forward to the named service at the given service-side port number.
> `service` is the name of a Service defined in the app's BSL script.
> `port` is a port number on that Service as defined by `service.port()`.
> `proto` is either `"tcp"` or `"udp"`.
> Returns `{ "forward_id": "<string>", "forward_key": <u16>, "max_udp_payload": <uint> | null }` on success.
> `forward_id` is used for control operations such as `/forwards/stop`.
> `forward_key` is the compact 2-byte identifier used in QUIC datagram headers for UDP forwards (see [datagram.forward](#i--datagram.forward)); it is not used for TCP forwards.
> `max_udp_payload` is the maximum UDP payload the server can forward from the service back to the client, as defined in [forward.mtu](#i--forward.mtu); it is `null` for TCP forwards.
> The control stream that carried the request is kept open for the lifetime of the forward; closing it tears down the forward.

> i[forward.mtu]
> For UDP forwards, `max_udp_payload` is the server's own `max_datagram_size() - 2` — the maximum payload the server can send toward the client (i.e. service responses).
> The client already has its own send limit for the client-to-server direction available locally from its QUIC connection.
> Path MTU may be asymmetric; for bidirectional protocols (e.g. DNS), clients should use the minimum of their local send limit and `max_udp_payload` when configuring application-level buffer sizes such as EDNS0.
> For a TUN-based client, setting the TUN interface MTU to that minimum causes the operating system to enforce the limit before packets reach the forwarding layer.
> This value reflects the path MTU estimate at the time of the request and may change as the estimate is refined.

> i[forward.tunnel.tcp]
> Each individual TCP connection forwarded through a TCP port forward uses a dedicated bidi stream as defined in [stream.forward](#i--stream.forward).
> The server accepts the stream, opens a TCP connection to the target service address and port, and relays bytes bidirectionally until either end closes.

> i[forward.tunnel.udp]
> Each UDP datagram forwarded through a UDP port forward is carried as a QUIC datagram as defined in [datagram.forward](#i--datagram.forward).
> The server extracts the `forward_key`, looks up the target service address and port, and forwards the payload as a UDP datagram.
> Responses from the service are sent back as QUIC datagrams with the same `forward_key` prefix.

> i[forward.key-exhaustion]
> `forward_key` is a 16-bit value; each connection can therefore have at most 65536 concurrent UDP forwards.
> When allocating a new key, the server scans for an unused slot and reuses keys freed by closed forwards.
> If all 65536 keys are in use the request fails with an `"internal"` error.

> i[forward.status]
> After the initial response, the server may send additional newline-terminated JSON objects on the control stream's send half to report runtime conditions.
> Each status message has the shape `{ "status": { "level": "<level>", "message": "<text>" } }` where `level` is `"warn"` or `"error"`.
> Clients should display these messages but must not treat them as fatal; the forward remains active unless the control stream closes.
> Conditions reported include oversized UDP datagrams dropped due to path-MTU limits, relay task failures, and datagram backpressure.

> i[forward.lifetime]
> A port forward remains active until any of the following occur:
>
> - The client closes the control stream.
> - The client sends `/forwards/stop { forward_id }` on a new control stream.
> - The connection is lost.
> - The forwarded service or port is no longer present in the app's AppDef after a script update takes effect (see [forward.script-update](#i--forward.script-update)).

> i[forward.script-update]
> When a new AppDef takes effect for an app (either immediately on `/apps/update` or at the next evaluation boundary if an operation was in progress), the server must check all active forwards for that app.
> Any forward whose target service name or port is no longer declared in the new AppDef must be torn down: all tunneled connections are closed, the control stream is closed, and a `ForwardStopped` event is emitted.
> Forwards whose target service and port still exist in the new AppDef are unaffected.

> i[forward.record]
> A forward record contains the following fields: `forward_id`, `app`, `service`, `port`, `proto`, and `opened_at` (RFC 3339).

> i[forward.list]
> `/forwards/list { app? }` returns an array of forward records for all currently active port forwards.
> If `app` is provided, only forwards for that app are returned; otherwise all active forwards across all apps are returned.

> i[forward.stop]
> `/forwards/stop { forward_id }` explicitly tears down an active port forward, closing all of its tunneled connections.
> Any operator may stop any forward regardless of which connection opened it.
> Returns `{}` on success, or `not_found` if the forward does not exist.

> i[forward.concurrent]
> Multiple port forwards may be active concurrently, including to the same service.

# Log Streaming

> i[logs.stream]
> `/logs/stream` opens a log stream for the specified target. The server acknowledges
> the request on the bidirectional stream and then delivers log entries on a
> server-initiated unidirectional stream as defined in [stream.logs](#i--stream.logs).

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

# Fault Surface

> i[fault.record]
> A fault record contains the following fields: `id` (opaque string), `app`, `resource_type`, `resource_name`, `instance_id`, `kind`, `timestamp` (RFC 3339), and `description` (human-readable string).

> i[fault.list]
> `/faults/list { app? }` returns an array of currently active fault records.
> If `app` is provided, only faults for that app are returned; otherwise all active faults across all apps are returned.

> i[fault.derived]
> Faults are derived conditions.
> They clear automatically when the condition that caused them no longer holds.
> No acknowledgement mechanism is provided; fault resolution and incident tracking are left to external consumers of the event feed.

# Event Feed

> i[event.subscribe]
> `/events/subscribe {}` causes the server to open a server-initiated unidirectional stream and begin pushing events as newline-delimited JSON for the duration of the connection, as defined in [stream.events](#i--stream.events).

> i[event.types]
> The following event types are defined.
> Every event object includes a `type` field (string) and a `timestamp` field (RFC 3339).
> When an event is caused by an OI request, the event object also includes an `actor` field containing the actor resolved from that request (see [wire.actor](#i--wire.actor)).
> For events triggered autonomously by the runtime — such as `OperationStarted` with `trigger: "boot"` or `trigger: "schedule"` — the `actor` field is absent.
>
> | `type` | Additional fields |
> |---|---|
> | `AppRegistered` | `app`, `generation` |
> | `AppDeregistered` | `app` |
> | `AppUpdated` | `app`, `generation`, `previous_generation` |
> | `ParamSet` | `app`, `name`, `previous_value`, `new_value`, `generation`, `previous_generation` |
> | `ParamUnset` | `app`, `name`, `previous_value`, `generation`, `previous_generation` |
> | `OperationStarted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `trigger` |
> | `OperationCompleted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation` |
> | `OperationFailed` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `error` |
> | `FaultFiled` | all fault record fields |
> | `FaultCleared` | `id`, `app` |
> | `ResourceStateChanged` | `app`, `resource_type`, `resource_name`, `instance_id`, `state` |
> | `ShellExited` | `session_id`, `exit_code` |
> | `ForwardStarted` | `forward_id`, `app`, `service`, `port` |
> | `ForwardStopped` | `forward_id` |
> | `ScaleChanged` | `app`, `deployment`, `scale`, `previous_scale`, `bounds_low`, `bounds_high` |
> | `ServerBusy` | `reason` |
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

# Key Management

> i[key.list]
> `/keys/list` returns the list of authorized client keys.
> Response `result` is an array of objects, each with `fingerprint` (string), `label` (string),
> and `added_at` (Unix timestamp integer).

> i[key.authorize]
> `/keys/authorise` adds a client key to the authorized set, or updates its label if the key is
> already authorized.
> Params: `fingerprint` (string, required), `label` (string, required).
> Returns `{}` on success. The original `added_at` timestamp is preserved on label update.

> i[key.revoke]
> `/keys/revoke` removes a client key from the authorized set.
> Params: `fingerprint` (string, required).
> Returns `{}` on success. Returns `not_found` if the fingerprint is not known.

> i[key.client.file-permissions]
> The client identity key file must be created with owner-read/write-only permissions.
> Seedling-ctl must refuse to load a client identity key file whose group or world
> permission bits are set, and must report an error.

## Registry Allowlist

> i[registry.list]
> `/registries/list` returns the current registry allowlist.

> i[registry.add]
> `/registries/add { registry }` adds a registry hostname to the allowlist.
> Adding a registry that is already present is a no-op.

> i[registry.remove]
> `/registries/remove { registry }` removes a registry hostname from the allowlist.
> All registered apps are re-evaluated after a registry is removed; any app whose images reference the removed registry will receive a `disallowed_registry` fault.

## Backup Apps

> i[backup.app.register]
> `/backups/apps/register { name, app }` registers the named app as a backup app under the given backup-app name. The app must declare actions `save-snapshot`, `list-snapshots`, and `restore-snapshot`. If validation fails, the request is rejected.
>
> Returns `{ "registered": true }` on success.

> i[backup.app.deregister]
> `/backups/apps/deregister { name }` removes a backup app registration.
> If any backup strategies reference this backup app, the request is rejected with `backup_app_in_use`.

> i[backup.app.list]
> `/backups/apps/list {}` returns an array of registered backup apps with fields `name` and `app`.

> i[backup.app.validation]
> On `/apps/update` for an app that is a registered backup app, the runtime must evaluate the new script and reject the update if the required actions (`save-snapshot`, `list-snapshots`, `restore-snapshot`) are no longer present.
> The same validation must be performed during `/apps/plan` (dry-run) and reported in the diff.

## Backup Strategies

> i[backup.strategy.create]
> `/backups/strategies/create { name, via, schedule, volumes }` creates a named backup strategy.
>
> - `name`: strategy name (follows standard naming rules).
> - `via`: name of a registered backup app.
> - `schedule`: one of `"every hour"`, `"twice a day"`, `"every day"`.
> - `volumes`: array of source volume identifiers (`"<app>/<volume>"` or `"_site/<volume>"`).
>
> Returns `{ "created": true }` on success.

> i[backup.strategy.list]
> `/backups/strategies/list {}` returns an array of strategy objects with fields `name`, `via`, `schedule`, and `volumes`.

> i[backup.strategy.show]
> `/backups/strategies/show { name }` returns the strategy object.

> i[backup.strategy.update]
> `/backups/strategies/update { name, via?, schedule?, volumes? }` updates a strategy. All fields except `name` are optional; only provided fields are changed.

> i[backup.strategy.delete]
> `/backups/strategies/delete { name }` deletes a strategy.

> i[backup.run]
> `/backups/run { strategy }` triggers an immediate backup for the named strategy, backing up all volumes in the strategy without a random delay.
>
> Returns `[{ "volume": "<vol>", "operation_id": "<id>" }, ...]` — one entry per volume in the strategy, in declaration order. The backup runs asynchronously; the operation IDs can be used to track progress via events.

> i[backup.snapshots.list]
> `/backups/snapshots/list { strategy, volume }` synchronously invokes the backup app's `list-snapshots` action and returns the output.
>
> `strategy` is the strategy name; `volume` is the volume identifier within the strategy (e.g. `"myapp/data"` or `"_site/vol"`).
>
> Returns the parsed JSON written by the action to its `"output"` volume as `snapshots.json`.

> i[backup.restore]
> `/backups/restore { strategy, volume, snapshot }` restores a snapshot to a new site volume.
>
> `strategy` is the strategy name; `volume` is the source volume identifier; `snapshot` is the snapshot identifier as returned by `list-snapshots`.
>
> Returns `{ "site_volume": "<name>" }` — the name of the newly created site volume containing the restored data.

# Client Behaviour

> i[ctl.graceful-shutdown]
> Long-running sessions must shut down gracefully on the appropriate termination signal:
>
> - **Shell sessions:** the terminal is in raw mode, so SIGINT (`Ctrl+C`) and all other control characters flow through the stdin relay to the remote shell naturally — no special handling is needed. Only SIGTERM triggers client-side shutdown: the client sends ETX (`0x03`) over the session stream's stdin direction, then waits up to five seconds for the session to end normally. If the session does not end within that window the client closes the connection.
> - **Port forwards:** on SIGINT or SIGTERM the client closes the control stream and exits.
> - **Event subscriptions:** on SIGINT or SIGTERM the client closes the connection and exits.

> i[ctl.subscribe.reconnect]
> When the event stream or connection is lost, the client must automatically reconnect and re-subscribe.
> Reconnection attempts use exponential backoff starting at one second, doubling up to a maximum interval of thirty seconds.
> If no connection can be established within five minutes of continuous retrying the client must exit with an error.

> i[ctl.logs.display]
> By default the CLI formats log entries as human-readable text: each line shows the
> timestamp, unit name, and message. When `--json` is passed, entries are printed as
> raw JSON objects (one per line) exactly as received from the server.

> i[ctl.logs.follow-interrupt]
> In follow mode, the client must exit cleanly on SIGINT or SIGTERM by closing the
> connection.

> i[ctl.forward.stats]
> While a port forward is active, the client must track:
>
> - Total bytes relayed in each direction (client-to-service and service-to-client).
> - For TCP forwards: the number of connections opened and currently active.
> - For UDP forwards: the number of datagrams relayed in each direction.
>
> On exit, the client prints a summary of these counters to stderr.

> i[ctl.action.params]
> The CLI accepts action params as positional arguments after the action name: `ctl apps action <app> <name> [key[=value]]...`.
> A bare key (no `=`) maps to `key: true`. A `key=value` pair maps to `key: "value"`.

> i[ctl.shell.params]
> The CLI accepts shell params with the same syntax: `ctl apps shell <app> <name> [key[=value]]...`.

> i[ctl.backup.app.hint]
> When `ctl apps create` evaluates a script that declares actions `save-snapshot`, `list-snapshots`, and `restore-snapshot`, the CLI should print an informational message suggesting backup app registration.

> i[ctl.backup.strategy.allow-missing]
> When creating or updating a backup strategy, the CLI checks whether each referenced volume exists. If any volume does not resolve and `--allow-missing` is not passed, the CLI must abort with an error before sending the request.
