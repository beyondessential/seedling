---
id: WIRE
---

# Streams and wire format

## Streams

<!-- from interface: Streams -->

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
>
> Both ends relay stdin and output independently: output waiting to be delivered never holds up stdin, nor the server noticing that the session has ended.

> i[stream.shell.framing]
> The server-to-client direction of the session stream carries newline-delimited JSON: the `/shells/start` response first, then the exit frame when the session ends.
> Every frame ends with a newline, so a client reading line by line sees each frame as soon as it is written rather than when the stream closes.
> A request that fails before the session opens is answered with an error response in place of the handshake response, and nothing follows it.
> After a handshake response the exit frame is the only frame.
> The client-to-server direction carries a single newline-terminated JSON request followed by raw stdin bytes for the remainder of the stream's lifetime.

> i[stream.forward]
> Each tunneled TCP connection within a port forward uses one client-initiated bidirectional QUIC stream.
> Both directions carry raw TCP bytes after an initial newline-terminated JSON header line: `{ "forward": "<forward_id>" }`.
> Each direction of the stream is finished when the matching direction of the tunneled TCP connection is, and the stream is reset when the tunneled connection fails, as [forward.tunnel.tcp](rule:i/forward.tunnel.tcp) describes.

> i[datagram.forward]
> Each tunneled UDP datagram within a port forward is carried as a QUIC datagram (RFC 9221).
> Every datagram begins with a 2-byte big-endian `forward_key` followed immediately by the UDP payload.
> QUIC datagrams are path-MTU constrained; payloads that exceed the limit are dropped and reported via a status message (see [forward.status](rule:i/forward.status)).

> i[stream.dispatch]
> All client-initiated bidirectional streams begin with a newline-terminated JSON object.
> If that object contains a `"method"` key it is dispatched as a control request per [stream.control](rule:i/stream.control).
> If it contains a `"forward"` key it is dispatched as a port forward data stream per [stream.forward](rule:i/stream.forward).

> i[stream.dispatch.server]
> All server-initiated bidirectional streams likewise begin with a newline-terminated JSON object, dispatched on the key it carries.
> The only key defined is `"canopy"`, which dispatches the stream as a Canopy relay request per [stream.canopy](rule:i/stream.canopy).
> A client that does not recognise the key must reset the stream rather than close it normally, so the server can distinguish an unsupported stream kind from an empty response.
>
> Server-initiated unidirectional streams are not covered by this rule: each is opened in response to a specific request and its content is defined by that request.

> i[stream.canopy]
> Each relayed Canopy request uses one server-initiated bidirectional QUIC stream.
> Both directions carry a newline-terminated JSON header line followed by raw body bytes for the remainder of the stream's lifetime; the stream's end is the body's end, and no additional length framing is used.
> The server writes the request header and body then half-closes its write-end; the client writes the response header and body then closes the stream.

> i[stream.subscribe]
> A subscription-style request — one whose success causes the server to open a server-initiated unidirectional stream carrying the subscribed data — is answered on the bidirectional stream exactly as any other control request, before any unidirectional stream is opened.
> A response carrying an error terminates the request: the server opens no unidirectional stream, and the connection remains usable for further requests.
> The response is bounded by the close of the bidirectional stream, as for any other control request per [stream.control](rule:i/stream.control), and not by a newline: a client must read the bidirectional stream to its end before parsing, and the subsequent data arrives on the unidirectional stream rather than on this one.
> Clients must therefore read and classify the response envelope before waiting for the unidirectional stream, must treat closure of the bidirectional stream without a response envelope as an error, must surface the error envelope's `code` and `message` to their caller rather than discarding them, and must not wait indefinitely for a unidirectional stream that a server which answered successfully may still fail to open.
> The subscription-style requests are [events.subscribe](rule:i/event.subscribe) and [logs.stream](rule:i/logs.stream).

> i[stream.events]
> After a client sends a `/events/subscribe` request, the server opens one server-initiated unidirectional QUIC stream per connection and pushes events as newline-delimited JSON objects for the duration of the connection.

> i[stream.logs]
> After a client sends a `/logs/stream` request, the server opens one server-initiated unidirectional QUIC stream and pushes log entries as newline-delimited JSON objects. When follow mode is not requested, the stream closes after historical entries are exhausted. When follow mode is requested, the stream remains open and new entries are pushed as they appear until the client drops the connection.

> i[stream.concurrency-limit]
> The server enforces a configurable upper bound on the number of concurrently active request/response streams across all connections.
> Long-lived sessions (forwards, shells, event subscriptions, log streams) release the permit after initial dispatch and do not count against the limit for their lifetime; the limit exists to bound the memory used by concurrent request body reads.
> When the limit is reached, the server immediately replies with a `server_busy` error and closes the stream.
> The limit is configurable at startup; the default value is 64.

## Wire Format

<!-- from interface: Wire Format -->

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
> | `operation_in_progress` | A lifecycle operation is running or queued for the app, or a change to it is being applied, and the request conflicts with it; `message` names which. |
> | `already_queued` | An operation is already queued for this app. |
> | `requirements_invalid` | Install requirements failed validation; per-field errors are included in `message`. |
> | `script_error` | The BSL script failed to parse or evaluate; detail is included in `message`. |
> | `validation_failed` | A parameter [validator](rule:i/param.validation) rejected the proposed values; each rejecting parameter and its reason are included in `message`. |
> | `bundle_invalid` | A definition bundle breaks the [bundle rules](rule:i/definition.bundle.limits); the offending path or field is included in `message`. |
> | `unsupported_seedling` | A definition does not [support](rule:i/definition.bundle.seedling-versions) the running Seedling version. |
> | `fetch_failed` | A definition could not be [fetched](rule:i/definition.fetch); the cause is included in `message`. |
> | `registry_not_allowed` | The registry named by a definition reference is not on the [registry allowlist](rule:i/registry.list). |
> | `deregistering` | The app is in the `Deregistering` state. |
> | `server_busy` | The server's stream concurrency limit has been reached; the client should retry after a delay. |
> | `internal` | An unexpected server-side failure (for example a database error) prevented the request from completing. |
> | `canopy_disabled` | Canopy access is turned off for this instance; the request was refused for that reason alone. |
> | `canopy_unavailable` | Canopy access is enabled but no provider is currently offering it, or the offering client could not reach Canopy. |
