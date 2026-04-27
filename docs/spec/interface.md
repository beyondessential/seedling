The Seedling Operator Interface (OI) is the channel through which external actors observe and control a running Seedling instance.

Its consumers are human operators (via CLI or UI), agentic operators, and automation pipelines.
The OI is the exclusive mechanism for registering applications, transitioning them out of the uninstalled state, invoking lifecycle actions, changing parameter values, opening interactive shell sessions, and receiving the fault and event feed.

Absent specification bugs, anything that is not defined here is either defined in another spec document (the language spec, the runtime spec), or is implicitly not allowed.

# Transport

> i[transport.quic]
> The OI uses QUIC as its wire transport protocol.

> i[transport.server-identity]
> The server authenticates using an RFC 7250 raw public key (SPKI).
> The server's key pair is generated at first startup and persisted to the data directory so that clients can pin the SPKI fingerprint across restarts.
> Clients verify the server by its SPKI fingerprint; certificate chain validation is not used.

> i[transport.listen]
> The server may be configured to listen on one or more addresses at startup.
> All configured addresses share the same server identity (key pair and SPKI fingerprint) and the same authorized key set.
> When no addresses are explicitly configured, the server listens on a single loopback address on the default port.

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
> | `already_installed` | `/apps/install/invoke` was called but the app is already `Installed` or `Uninstalling`. |
> | `install_in_progress` | `/apps/install/invoke` was called but an install for this app is already running. |
> | `operation_in_progress` | A lifecycle operation is running and the request conflicts with it. |
> | `already_queued` | An operation is already queued for this app. |
> | `requirements_invalid` | Install requirements failed validation; per-field errors are included in `message`. |
> | `script_error` | The BSL script failed to parse or evaluate; detail is included in `message`. |
> | `deregistering` | The app is in the `Deregistering` state. |
> | `unauthorized` | The client's key is not in the authorized set, or the operation is not permitted. |
> | `server_busy` | The server's stream concurrency limit has been reached; the client should retry after a delay. |

# Status

> i[status.infra]
> `/infra/status` returns the running state of infrastructure components managed by the Seedling daemon.
> The response contains the following fields:
>
> - `proxy`: `"running"` if at least one proxy container slot is running, otherwise `"stopped"`.
> - `resolver`: `"running"` if at least one resolver container slot is running, otherwise `"stopped"`.

> i[status.ping]
> `/server/ping` is a trivial liveness probe. It accepts no params, returns an empty object `{}`, and must never fail except on transport errors. Clients use it to confirm the daemon is reachable without incurring the cost of `/server/status`.

> i[status.get]
> `/server/status` returns a summary of the running Seedling instance.
> It must always succeed and must not perform any expensive computation.
> The response contains the following fields:
>
> - `version`: the Seedling version string.
> - `hostname`: the hostname of the machine running the Seedling instance.
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
> - `NotInstalled`: the install action has never completed successfully for this app, and no install is currently in progress.
> - `Installing`: an install operation is in progress for this app. The reconciler actuates resources the install closure has placed into the desired state per [desired-state.during-install](#r--desired-state.during-install).
> - `Deregistering`: deregistration was requested and resource teardown is in progress.
> - `Operating`: a lifecycle operation other than `install` is in progress. Includes the field `action_name`.
> - `Running`: steady state; no active faults; all resources are at their desired lifecycle states.
> - `Degraded`: steady state, but one or more resources are not at their desired lifecycle state or have an active fault. Resources that are actively being torn down (`Terminating` or `Terminated` lifecycle state) are not counted as degraded — they are transitioning to `Unscheduled` and do not represent a fault condition.
> - `Faulted`: one or more active faults exist and at least one resource has been excluded from active reconciliation.

> i[app.status.priority]
> When multiple conditions apply simultaneously, the state with the highest priority is reported.
> Priority order, highest first: `Deregistering`, `Installing`, `Operating`, `NotInstalled`, `Faulted`, `Degraded`, `Running`.

# App Description

> i[app.describe]
> `/apps/show { app }` returns a single object with the following fields:
>
> - `status`: the app's current status as defined in [app.status](#i--app.status).
> - `faults`: array of app-level [fault records](#i--fault.record) not associated with a specific resource instance (e.g. script evaluation errors). Empty when there are no active app-level faults.
> - `resources`: array of objects with fields `name`, `type`, `instances`, `faults`, `def`, and for Deployment resources, `scale`.
>   Each instance has fields `id`, `display_name`, `lifecycle`, and `transition_time` (RFC 3339, optional).
>   Each fault entry is a [fault record](#i--fault.record).
>   `def` is an object describing the resource's configuration. The shape varies by `type`:
>   for `ingress`: `{ hostname, port, tls, dtls, http_terminate, redirect }`;
>   for `service`: `{ http }`;
>   for `http_service`: `{ service, port }`;
>   for `deployment`: `{ container, pod, scale, on_update, on_terminate }`;
>   for `job`: `{ container, pod, deadline }`;
>   for `volume`: `{ readonly, tmpfs, writes, exported, export_description }`.
>   `container` has fields `image`, `command`, `args`, `env`, `volume_mounts`, `on_exit`, `memory`, `cpus`, `extra_caps`, `writable_rootfs`, `pids_limit`.
>   `pod` has fields `service_mounts`, `http_bindings`, `tcp_bindings`, `udp_bindings` (each an array of strings).
> - `params`: array of objects with fields `name`, `value`, `is_set`, `secret`, `kind`, `required`, `description`, and `default_value`.
>   `is_set` is `true` when the parameter has a stored value.
>   `value` is the string value if the parameter is set and not secret; `null` if the parameter is unset or if it is secret.
>   `secret` is `true` when the parameter's effective secret flag is `true` (see [param.schema.secret](#l--param.schema.secret) and [param.schema.secret-from-kind](#l--param.schema.secret-from-kind)).
>   The schema fields (`kind`, `required`, `description`, `default_value`) reflect any metadata set via the BSL param builder methods.

> i[app.describe.param-secret]
> When a param's effective `secret` flag is `true`, its `value` must be `null` in the response regardless of whether a value is stored. Clients must use `is_set` to distinguish an unset secret from a set-but-redacted secret.
> - `unknown_params`: array of objects with fields `name` and `value`, listing parameters that have a stored value in the database but whose name does not appear in the app's current script evaluation. This is informational only; these values have no effect until the script is updated to reference them.
> - `actions`: array of objects with fields `name`, `description`, `kind`, `params`, and `schedules`.
>   `kind` is one of `action`, `shell`, `install`, or `lifecycle`. The `lifecycle` kind is used for the Start Action, which is driven autonomously and cannot be manually invoked.
>   `params` is an object map of param key to `{ kind, required, description, default_value }`, as defined in the language spec. Empty for actions with no declared param schema.
>   `schedules` is an array of objects with fields `cronexpr`, `last_fired_at`, and `next_fire_at`, listing every cron schedule attached to the action via [action.schedule](language.md#l--action.schedule). The array is empty for actions with no declared schedule. `last_fired_at` is the RFC 3339 timestamp at which the runtime last fired this schedule, or `null` if it has never fired (or the app is not yet installed). `next_fire_at` is the RFC 3339 timestamp at which the schedule is next expected to fire, computed from `last_fired_at` (or the current time if never fired); it is `null` only when the cron expression cannot be evaluated.
> - `current_operation`: present only when status is `Operating`.
>   Has fields `action_name`, `barrier`, `source_generation`, and `target_generation`.
>   `barrier` is either `null` (operation is running but not yet at a barrier, or the barrier has already been satisfied and the closure is about to resume) or an object with fields `resources`, `required_state`, `deadline_secs`, and `elapsed_secs`.
>   `resources` is an array of resource display names the barrier is awaiting.
>   `required_state` is the awaited lifecycle state (e.g. `"Ready"`, `"Terminated"`).
>   `deadline_secs` is the deadline in seconds, or `null` for a deadline-less barrier (see [rt.started.terminated-eventually](language.md#l--rt.started.terminated-eventually) and [rt.started.ready-eventually](language.md#l--rt.started.ready-eventually)).
>   `elapsed_secs` is how many seconds have passed since the barrier first suspended.
> - `generation`: the current generation of the app.

> i[action.describe.barrier]
> The `current_operation.barrier` field of `/apps/show` reflects the earliest unsatisfied barrier recorded in the action log for the active operation.
> When the operation's closure is between barriers — actively running, or has had all prior barriers satisfied with no new one queued — the field is `null`.
> `elapsed_secs` is computed as `now - started_at_secs`; it persists across runtime restarts because the barrier record survives in the action log.

# Scaling

> i[scale.set]
> `/apps/scale { app, deployment, scale }` sets the running scale of a single Deployment within an installed app.
> `scale` is a non-negative integer. The value is clamped to the deployment's declared bounds; requests that would move outside the bounds succeed but stay at the boundary.
> The app must be registered and the named deployment must exist in the current AppDef; otherwise `not_found` is returned.
> On success, the response contains `scale` (the new scale value) and `bounds` with `low` and `high`.

> i[scale.decision-persistence]
> The effective scale chosen by `/apps/scale` is stored durably and survives process restarts.
> On startup, the stored decision is loaded and used as the effective scale for the deployment.

> i[scale.reset-on-uninstall]
> When an app is uninstalled, all stored scaling decisions for that app are discarded.
> After reinstallation, each Deployment's effective scale reverts to its declared lower bound.

> i[scale.describe]
> `/apps/show` includes, for each Deployment resource, a `scale` object with fields `low` (lower bound), `high` (upper bound), and `current` (the effective scale).
> `current` is the stored scaling decision clamped to the declared bounds, or the lower bound if no decision has been stored.

# Deployment Restart

> i[deployment.restart]
> `/apps/restart { app, deployment }` triggers a restart of all running instances of the named Deployment within the installed app, following its configured update strategy (`on_update`: rolling or replace).
> The app must be registered and the named deployment must exist in the current AppDef; otherwise `not_found` is returned.
> A restart does not change the deployment's definition or generation. Running instances are replaced with fresh containers carrying the same configuration.
> The restart is durable: if the process restarts before the reconciler finishes, the replacement will resume on the next startup.
> On success, the response is `{ operation_id }` where `operation_id` is a unique string identifying this restart operation.

# Resource Stop / Unstop

> i[resource.stop]
> `/apps/resource/stop { app, kind, name }` turns off a named resource within an installed app without changing its declared configuration.
> `kind` must be one of `deployment`, `job`, or `ingress`; other kinds (`service`, `volume`, `externalvolume`) return `invalid_request`.
> The app must be registered and the named resource must exist in the current AppDef; otherwise `not_found` is returned.
> Stopping a deployment scales its running instances to zero without modifying the declared scale bounds; unstopping later restores the declared effective scale.
> Stopping a job or ingress marks it as unscheduled without removing its definition.
> A stopped state is durable and survives process restarts.
> On success, the response is `{}`.

> i[resource.unstop]
> `/apps/resource/unstop { app, kind, name }` reverses a previous stop for the named resource.
> If the resource was not stopped, this is a no-op and returns `{}`.
> On success, the response is `{}`.

> i[resource.unstop-all]
> `/apps/unstop { app }` unstops all resources for the named app in one call.
> If no resources are stopped, this is a no-op and returns `{}`.
> On success, the response is `{}`.

> i[resource.stop.no-active-op]
> All three stop/unstop endpoints (`/apps/resource/stop`, `/apps/resource/unstop`, `/apps/unstop`) reject with `operation_in_progress` when a lifecycle operation is active or queued for the target app.
> The operator is expected to cancel the active operation first via [action.cancel](#i--action.cancel); desired-state mutations must not race with a running action closure.

> i[resource.stop.status]
> `/apps/show` includes a `stopped` boolean field on each resource that has been stopped via `/apps/resource/stop`.
> The top-level response includes a `stopped_resources` array of `{ kind, name }` objects listing every resource currently stopped for the app.
> `/apps/list` includes a `has_stopped_resources` boolean field on each app summary indicating whether any resources are currently stopped.

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
> - `previous_value`: present for `param_set` and `param_unset`; `null` if the parameter was unset before this entry, if the parameter is currently secret, or if the value has been redacted. See `redacted`.
> - `new_value`: present for `param_set` and `param_unset`; `null` for `param_unset`, if the parameter is currently secret, or if the value has been redacted. See `redacted`.
> - `redacted`: boolean; `true` when the parameter named by `param_name` is currently secret, in which case `previous_value` and `new_value` are `null` regardless of whether values were recorded.
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

> i[param.store.secret]
> The `set` and `unset` operations must route secret parameter values (those whose effective `secret` flag is `true`) through protected storage as defined in [secret.storage](#r--secret.storage).
> Events emitted for secret parameter changes must never carry the plaintext values.

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
> While an app is `NotInstalled` or `Installing`, all action and shell invocations except `/apps/install/invoke` are rejected with `not_installed`.

> i[action.invoke]
> `/apps/action/invoke { app, name, params? }` schedules the named action as a lifecycle operation.
> `params` is an optional JSON object. Keys ending in `_volume` or `_filename` are reserved for internal use (see [operation.volume-param](runtime.md#r--operation.volume-param)) and must be rejected.
> If the app has an active `script_error` fault, the request is rejected with `script_error`.
> If the action has a declared param schema, schema-defined params are validated and defaults applied before the operation is enqueued; validation failure returns `requirements_invalid`.
> Shell actions must not be invoked via this method; `not_found` is returned if a shell name is provided.
> The Start Action (`name` = `"start"`) must not be invoked via this method; `not_found` is returned.
> Returns `{ "schedule": "accepted", "operation_id": "<string>" }` or `{ "schedule": "queued", "operation_id": "<string>" }` on success, or an error. The `operation_id` is always present and uniquely identifies this operation.

> i[action.cancel]
> `/apps/action/cancel { app }` requests cancellation of the currently-active lifecycle operation for the named app.
> The semantics of cancellation are defined by [operation.cancel](runtime.md#r--operation.cancel): the runtime wakes any suspended barrier and drives the operation to a terminal cancelled state.
> If no operation is active for the app, the request is rejected with `not_found`.
> If a cancel is already pending for the active operation, the request is still accepted and is a no-op.
> On success, the response is `{ "cancelled": true }`.

> i[action.invoke.install]
> `/apps/install/invoke { app, params? }` schedules the install action.
> It is only valid when the app is `NotInstalled`.
> If the app is already `Installing`, the request is rejected with `install_in_progress`.
> If the app is `Installed` or `Uninstalling`, the request is rejected with `already_installed`.
> If the app has an active `script_error` fault, the request is rejected with `script_error`.
> `params` is an optional JSON object of param key to string value. The values are delivered to the install closure as `param`.
> If the app has no explicit install action, `params` must be absent or empty.
> Params are validated before the operation is enqueued; validation failure returns `requirements_invalid`.
> Returns `{ "schedule": "accepted" }` or `{ "schedule": "queued" }` on success, or an error.
> On `accepted`, the app atomically transitions to `Installing`.

> i[action.invoke.install.validation]
> Params are validated according to the kinds defined in the language spec before the operation is enqueued.
> A required field with no provided value and no `default_value` is a validation error.
> The params object is passed to the install action closure and is persisted alongside the operation record (see [operation.params](#r--operation.params)) so that a runtime restart during the install can replay the operation.
> Values of params whose effective `secret` flag is `true` must be protected in persisted form as defined in [secret.storage](#r--secret.storage).
> The persisted params are cleared when the install operation completes, regardless of outcome.

> i[action.invoke.install.completion]
> When an install operation completes successfully, the app transitions `Installing → Installed`.
> When an install operation fails, the app transitions `Installing → NotInstalled` and an `operation_failed` fault is filed carrying the failure detail.
> On subsequent runtime restarts, the runtime will initiate the `start` action for `Installed` apps automatically, as specified in the runtime spec.

# Shell Sessions

> i[shell.open]
> `/shells/start { app, name, rows, cols, params? }` opens an interactive shell session.
> `params` is an optional JSON object. Keys ending in `_volume` or `_filename` are reserved for internal use (see [operation.volume-param](runtime.md#r--operation.volume-param)) and must be rejected.
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

> i[shell.start]
> When a session is registered, a `ShellStarted` event is emitted on the event feed.

> i[shell.exit]
> When a session ends, the server writes a final JSON frame `{ "exit_code": <int> }` to the server-to-client direction of the session stream, then closes its write-end.
> Signal-terminated processes report a negative exit code.
> A `ShellExited` event is also emitted on the event feed.

> i[shell.cleanup]
> Dynamic resources created within a shell session are cleaned up by the runtime when the session ends, as specified in the runtime spec.

> i[shell.concurrent]
> Shell sessions may run concurrently with lifecycle operations and with other shell sessions.

# Volume Shells

> i[volumes.shell]
> `/volumes/shell { volumes, rows, cols, read_only? }` opens an interactive shell session inside an ephemeral Ubuntu container with one or more volumes bind-mounted at `/mnt/{display-name}`.
>
> `volumes` is an array of volume references. Each reference is one of:
> - `{ "kind": "site", "name": "<name>" }` — a site volume.
> - `{ "kind": "app", "app": "<app>", "volume": "<volume>" }` — a volume owned by an app.
>
> The `display-name` for a site volume is its `name`; for an app volume it is `<app>.<volume>`.
> Display names are sanitised to be valid path components before use as mount-point suffixes.
> Each volume is mounted read-only if the volume is inherently read-only (snapshot site volumes); otherwise it is mounted read-write — unless the request sets `read_only: true`, in which case every mount is forced read-only regardless of the underlying volume kind. `read_only` defaults to `false`.
>
> Volume shells use the same three-stream wire protocol as regular shell sessions (see [stream.shell](#i--stream.shell)).
> They are registered in the shell session registry with `app = "_volumes"` and `name` set to the comma-separated list of display names; they appear in `/shells/list`, support `/shells/resize` and `/shells/stop`, and emit `ShellStarted`/`ShellExited` events identically to regular shell sessions.
> Returns `not_found` if any referenced volume does not exist.

> i[volumes.shell.caps]
> The shell container holds enough Linux capabilities for root inside the container to behave as root over the bind-mounted volumes — at minimum `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_CHOWN`, and `CAP_FOWNER`.
> Without them, the seedling-default `--cap-drop=ALL` would leave shell-root unable to traverse or edit files owned by service-specific UIDs (e.g. PostgreSQL's `0700 999:999` data directory), which defeats the point of opening the shell.

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

> i[forward.start]
> When a forward is established, a `ForwardStarted` event is emitted on the event feed.
> When a forward ends for any reason, a `ForwardStopped` event is emitted on the event feed.

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

> i[fault.clear-app]
> `/faults/clear { app }` clears every currently active fault for the named app.
> Returns `{ app, cleared }` where `cleared` is the number of fault rows that transitioned from active to cleared.
> Faults derived from observable conditions (e.g. `image_pull_failed`, `health_check_failed`) will be re-filed on the next reconciliation tick if the underlying condition still holds. Hard faults that require operator action (e.g. `health_check_replace_failed`, `script_error`) are cleared definitively until the underlying issue recurs.
> The endpoint is intended for operators stuck behind a fault that the runtime cannot itself resolve, including the case where a not-installed app's faults are blocking a script update.

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

## Image Management

> i[image.list]
> `/images/list {}` returns the set of locally-present container images.
> Response `result` is an array of objects, each with fields:
>
> - `image_id`: string, the image's content-addressable digest (e.g. `"sha256:..."`).
> - `references`: array of strings; the tags and/or digest references that currently point at this image in local storage. May be empty for dangling images.
> - `size_bytes`: integer.
> - `created_at`: RFC 3339 timestamp, as reported by the container runtime.
> - `last_used_at`: RFC 3339 timestamp — the last time the runtime observed this image being used by a running container, or the time the image was first recorded locally if it has never been in use.
> - `in_use`: boolean; `true` when at least one running container currently uses this image.
> - `pinned_by`: array of app names that currently pin at least one reference resolving to this image.

> i[image.pull]
> `/images/pull { reference, app? }` requests that the named image reference be pulled into local storage through the runtime's standard pull machinery (with retries and back-off).
> `reference` must be a non-empty container image reference (e.g. `"ghcr.io/example/foo:1.2.3"`).
> If `app` is provided, the app must be registered; the pulled reference is additionally [pinned](#r--image.pin) to that app.
> Returns `{ "ok": true }` on accepted pull. Pull failures surface through the existing [image pull fault](#r--fault.image-pull) mechanism rather than the response.

> i[image.remove]
> `/images/remove { reference, force? }` removes the named image from local storage.
> If at least one running container currently uses the image, the request is rejected with `requirements_invalid` unless `force` is `true`.
> Pins targeting `reference` are cleared regardless of outcome.
> Returns `{ "ok": true }` on successful removal; `not_found` if no image resolves to `reference` in local storage.

> i[image.pin.list]
> `/images/pins/list { app? }` returns the set of image pins.
> When `app` is provided, only that app's pins are returned; otherwise pins for all apps are returned.
> Response `result` is an array of objects with fields `app` (string), `reference` (string), `pinned_at` (RFC 3339 timestamp), and `expires_at` (RFC 3339 timestamp or `null`). When `expires_at` is set, the reconciler will delete the pin once it passes (see [`image.pin.expiry`](runtime.md#r--image.pin.expiry)).

> i[image.pin.clear]
> `/images/pins/clear { app, reference? }` clears pins for `app`.
> When `reference` is provided, only the pin matching that reference is cleared (no-op if absent); otherwise all pins for the app are cleared.
> Returns `{ "ok": true }`.

> i[image.discover]
> `/apps/images/discover { app, action_params?, lenient? }` runs the probe execution mode defined in [`image.discover`](runtime.md#r--image.discover) across every handler declared by the named app (`install`, the implicit `start` handler, every explicit action, every shell, and every `on_change` handler), then returns what those handlers might pull.
>
> Params:
>
> - `action_params`: optional object map of handler name → object map of param name → string value. Values supplied here override stored values and defaults for the named handler's probe.
> - `lenient`: boolean, default `false`. When `true`, handlers with unresolved required parameters are reported as skipped and the probe continues; when `false`, the same condition is reported as an error and the other handlers still probe normally.
>
> Response shape:
>
> ```json
> {
>   "per_handler": [
>     {
>       "name": "migrate",
>       "kind": "action",
>       "images": ["ghcr.io/example/foo:1.2.3"],
>       "error": null,
>       "skipped_reason": null
>     },
>     { "name": "install", "kind": "install", "images": [], "error": null, "skipped_reason": null },
>     { "name": "upgrade", "kind": "action", "images": [], "error": null, "skipped_reason": "requires params: old_version" }
>   ],
>   "all_images": ["ghcr.io/example/foo:1.2.3"]
> }
> ```
>
> `kind` is one of `"install" | "start" | "action" | "shell" | "param_change"`. `error` is a human-readable message or `null`. `skipped_reason` is populated only in lenient mode when a handler was not probed. `all_images` is the deduplicated union of `images` across every handler with no `error` and no `skipped_reason`.

## Backup Apps

> i[backup.app.register]
> `/backups/apps/register { app }` opts `app` in to the backup role. The app's BSL script must already declare the `save-snapshot`, `list-snapshots`, and `restore-snapshot` actions; otherwise the request is rejected with `requirements_invalid`.
>
> Returns `{ "registered": true }` on success.

> i[backup.app.deregister]
> `/backups/apps/deregister { app }` removes `app` from the backup role.
> If any backup strategies reference this app via their `via` field, the request is rejected with `backup_app_in_use`.

> i[backup.app.list]
> `/backups/apps/list {}` returns an array of registered backup apps; each entry has a single field `app` naming the BSL app.

> i[backup.app.validation]
> On `/apps/update` for an app that is a registered backup app, the runtime must evaluate the new script and reject the update if the required actions (`save-snapshot`, `list-snapshots`, `restore-snapshot`) are no longer present.
> The same validation must be performed during `/apps/plan` (dry-run) and reported in the diff.

## Backup Strategies

> i[backup.strategy.create]
> `/backups/strategies/create { name, via, schedule, volumes }` creates a named backup strategy.
>
> - `name`: strategy name (follows standard naming rules).
> - `via`: BSL app name of a registered backup app.
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
> The action is expected to filter its output to only snapshots it took for that volume via `save-snapshot`, so that restores cannot accidentally select a snapshot belonging to a different volume that shares the same backend repository. The information needed to filter is delivered via the `backup` param object (see [backup.action.backup-param](#i--backup.action.backup-param)).
>
> Returns the parsed JSON written by the action to the volume delivered under the logical binding key `"output"`, at the filename given by the runtime (see [backup.list](runtime.md#r--backup.list) and [operation.volume-param](runtime.md#r--operation.volume-param)).

> i[backup.action.backup-param]
> When Seedling invokes any of the three required backup actions (`save-snapshot`, `list-snapshots`, `restore-snapshot`) on a registered backup app, the param map passed to the action closure must contain a key `backup` whose value is an object with the following string fields:
>
> - `strategy`: the strategy name that caused this invocation.
> - `app`: the BSL app whose volume is being backed up, or the literal `"_site"` for site-scoped volumes.
> - `volume`: the named volume within `app` (for `"_site"`, the site volume name).
>
> `save-snapshot` is expected to stamp `app` + `volume` (and optionally `strategy`) onto the resulting snapshot via the backend's tagging mechanism. `list-snapshots` is expected to filter its output by the same identity. `restore-snapshot` is expected to verify the requested snapshot matches before writing to the destination.
>
> `restore-snapshot` additionally receives an opaque `snapshot` string param (the identifier previously returned by `list-snapshots`).

> i[backup.restore]
> `/backups/restore { strategy, volume, snapshot }` restores a snapshot to a new site volume.
>
> `strategy` is the strategy name; `volume` is the source volume identifier; `snapshot` is the snapshot identifier as returned by `list-snapshots`.
>
> Returns `{ "site_volume": "<name>" }` — the name of the newly created site volume containing the restored data.

# Templates

> i[template.definition]
> A template is a stored, named BSL script body that is held for reuse.
> Templates are not themselves evaluated by the reconciler: they do not have a generation, phase, resources, parameter values, faults, or any other runtime state.
> A template exists solely as a script body that can be inspected (previewed) and copied wholesale into a new app via `/templates/instantiate`.

> i[template.name]
> Template names follow the same rules as app names — see [bsl.name](language.md#l--bsl.name) — and must not start with an underscore.
> Template names share no namespace with app names; a template and an app may have the same name.

> i[template.create]
> `/templates/create { name, body, description? }` stores a new template.
> The script body is not evaluated at create time; syntactically invalid bodies are accepted and will surface as errors on `/templates/preview` or `/templates/instantiate`.
> If a template with the given name already exists the request is rejected with `requirements_invalid`.
> On success a `TemplateCreated` event is emitted.

> i[template.list]
> `/templates/list {}` returns an array of objects with fields `name`, `description` (string or `null`), and `created_at` (RFC 3339 timestamp).
> The array is ordered by name.

> i[template.show]
> `/templates/show { name }` returns a single object with fields `name`, `body`, `description`, and `created_at`.
> Returns `not_found` if no such template exists.

> i[template.update]
> `/templates/update { name, body?, description? }` updates an existing template.
> Only provided fields are changed: omitting `body` leaves the stored body unchanged, and omitting `description` leaves the stored description unchanged.
> To clear a description, pass `description: null`.
> As with [template.create](#i--template.create), the body is not evaluated at update time.
> Returns `not_found` if no template with the given name exists.
> The template's `created_at` timestamp is not modified by update.
> Apps already instantiated from the template are unaffected; the template's script body is copied at instantiation time and has no link to later edits.
> On success a `TemplateUpdated` event is emitted.

> i[template.remove]
> `/templates/remove { name }` deletes a template.
> Instantiated apps derived from the template are unaffected — their script body was copied at instantiation time.
> Returns `not_found` if no such template exists.
> On success a `TemplateRemoved` event is emitted.

> i[template.preview]
> `/templates/preview { name?, body? }` evaluates a template body and returns a read-only summary of what it declares, without storing or instantiating anything.
> Exactly one of `name` (an existing template) or `body` (raw script text) must be supplied.
> The response contains:
>
> - `resources`: array of objects with fields `name`, `type`, and `def` — the same shapes defined in [app.describe](#i--app.describe). Instance state and faults are not included because a template has none.
> - `params`: array of objects with fields `name`, `kind`, `required`, `description`, `default_value`, and `secret`, matching the param schema defined in [app.describe](#i--app.describe).
> - `actions`: array of objects with fields `name`, `description`, `kind`, `params`, and `schedules`, matching the shape defined in [app.describe](#i--app.describe). For a template preview the timing fields of each schedule (`last_fired_at`, `next_fire_at`) are `null`, because the template is not associated with any registered app.
> - `script_error`: a string describing a script evaluation failure, or `null` when evaluation succeeded. When `script_error` is non-null the other fields reflect whatever partial state the engine produced before the error.

> i[template.instantiate]
> `/templates/instantiate { template, app }` creates a new app whose script body is a verbatim copy of the named template's body.
> The behaviour is equivalent to calling [app.register](#i--app.register) with the template body and `app` as the name: the app starts in the `NotInstalled` state and an `AppRegistered` event is emitted.
> In addition a `TemplateInstantiated` event is emitted referencing both the template name and the new app name.
> After instantiation the app's script is independent of the template — subsequent `/templates/remove` or any future template editing has no effect on the app.
> Returns `not_found` if the template does not exist; `requirements_invalid` if the app name is invalid or already in use; `script_error` if the template body fails to evaluate.

# TLS Certificate Management

The TLS certificate operator surface mirrors the runtime concepts defined in [the runtime spec](runtime.md): named DNS providers, per-hostname policies, and a certificate registry covering manual uploads, CSR-derived certs, and certs the runtime obtained itself via ACME.
This section covers the operator interface for the ACME-DNS strategy, manual cert lifecycle, and policy management.

## DNS Providers

> i[tls.dns-provider.list]
> `/tls/dns-providers/list` returns the configured DNS providers without their credentials.
> Response `result.providers` is an array of objects each with `name` (string), `kind` (string), `created_at`, and `updated_at` (Unix timestamp integers).

> i[tls.dns-provider.upsert]
> `/tls/dns-providers/upsert { name, kind, config }` creates a new provider entry or replaces an existing one with the same name.
> `config` is a provider-specific object whose shape depends on `kind`. For `kind = "route53"` the required fields are `access_key_id`, `secret_access_key`, and an optional `region` (defaults to `us-east-1`).
> The credentials are stored encrypted at rest and never returned through any operator endpoint.
> Returns `requirements_invalid` if `name` is empty or `kind` is unknown.

> i[tls.dns-provider.delete]
> `/tls/dns-providers/delete { name }` removes a DNS provider entry.
> Returns `not_found` if no provider with that name exists.
> Returns `requirements_invalid` if any policy still references the provider; the operator must clear those policies first.

## Policies

> i[tls.policy.list]
> `/tls/policies/list` returns all per-hostname policy rows.
> Response `result.policies` is an array of objects each with `hostname`, `strategy` (`"acme_dns"` or `"manual"`), and either `dns_provider` (for acme-dns) or `cert_id` (for manual), plus `updated_at`.
> Hostnames not in this list use the default ACME-HTTP-01 strategy.

> i[tls.policy.set-acme-dns]
> `/tls/policies/set-acme-dns { hostname, dns_provider, contact_email?, directory_url? }` binds `hostname` to the named DNS provider for ACME-DNS-01 issuance.
> The new policy takes effect on the next reconciliation tick per [tls.policy.apply](runtime.md#r--tls.policy.apply).
> When `contact_email` is supplied and the hostname has no active certificate yet, the runtime fires a single ACME-DNS issuance attempt asynchronously; the response includes `auto_issue_kicked: true` to signal that.
> Subsequent renewals are handled by the autonomous renewal task using the ACME account credentials persisted during first issuance, so `contact_email` is required only at first-issue time.
> When `contact_email` is omitted, no issuance is triggered; the operator must run [`tls.cert.issue-acme-dns`](#i--tls.cert.issue-acme-dns) to acquire the first cert.
> `directory_url` defaults to the Let's Encrypt production directory.

> i[tls.policy.set-manual]
> `/tls/policies/set-manual { hostname, cert_id }` binds `hostname` to a stored certificate row.
> The cert must already exist in the certificate store via manual upload or the CSR flow.
> The new policy takes effect on the next reconciliation tick.

> i[tls.policy.clear]
> `/tls/policies/clear { hostname }` removes any operator policy for `hostname`, returning it to the default ACME-HTTP-01 strategy.
> Returns `not_found` if no policy exists for the hostname.

## Certificates

> i[tls.cert.list]
> `/tls/certificates/list` returns all stored certificates.
> Response `result.certificates` is an array of objects each with `id`, `hostname`, `state` (`"csr_pending"`, `"active"`, `"superseded"`, or `"failed"`), `origin` (`"manual"`, `"csr"`, or `"acme_dns"`), `key_type`, `issuer`, `not_before`, `not_after`, `serial`, `self_signed`, `note`, `acme_account_id`, `created_at`, `updated_at`.
> Private key material is never returned.

> i[tls.cert.issue-acme-dns]
> `/tls/certificates/issue-acme-dns { hostname, contact_email, directory_url? }` synchronously runs the ACME-DNS-01 issuance flow for `hostname`.
> The hostname must already be bound to an `acme_dns` policy with a configured DNS provider; otherwise the call returns `requirements_invalid`.
> `directory_url` defaults to the Let's Encrypt production directory.
> The call blocks for the full duration of the ACME flow (typically tens of seconds) and returns `{ cert_id, not_after }` on success.
> Failure returns `internal` with a message identifying the stage that failed.
> The newly-issued certificate supersedes any prior active certificate for the same hostname.

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

> i[ctl.action.cancel]
> The CLI exposes `ctl apps cancel-action <app>` as a thin wrapper over [action.cancel](#i--action.cancel).

> i[ctl.shell.params]
> The CLI accepts shell params with the same syntax: `ctl apps shell <app> <name> [key[=value]]...`.

> i[ctl.backup.app.hint]
> When `ctl apps create` evaluates a script that declares actions `save-snapshot`, `list-snapshots`, and `restore-snapshot`, the CLI should print an informational message suggesting backup app registration.

> i[ctl.backup.strategy.allow-missing]
> When creating or updating a backup strategy, the CLI checks whether each referenced volume exists. If any volume does not resolve and `--allow-missing` is not passed, the CLI must abort with an error before sending the request.
