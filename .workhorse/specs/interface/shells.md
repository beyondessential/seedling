---
id: SHL
---

# Shell sessions

## Shell Sessions

<!-- from interface: Shell Sessions -->

> i[shell.open]
> `/shells/start { app, name, rows, cols, params? }` opens an interactive shell session.
> `params` is an optional JSON object. Keys ending in `_volume` or `_filename` are reserved for internal use (see [operation.volume-param](rule:r/operation.volume-param)) and must be rejected.
> Returns `{ session_id, stdout_stream_id, stderr_stream_id }` as the handshake response on the session stream.
> After the handshake response is written, the server treats subsequent bytes on the session stream's client-to-server direction as raw stdin for the shell's job.

> i[shell.startup]
> A session is registered under its `session_id` before the handshake response carrying that id is written, and stays registered while its shell starts, which can include running the shell's script, pulling its image and creating its network.
> From the moment the client holds the id, the session appears in `/shells/list`, `/shells/resize` records the dimensions to apply as soon as the terminal exists, and `/shells/stop` ends the session without waiting for the shell to start.
> A session stopped before its job is running ends without the job having started, and anything it had already created, such as its pod network, is removed.
A session stopped while the shell's script is running ends only once the script has stopped, so the script creates nothing after the session is over.

> i[shell.streams]
> Each session uses the three streams defined in [stream.shell](rule:i/stream.shell).
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
> The session ends as per [shell.close](rule:i/shell.close), with the job terminated and dynamic resources cleaned up.
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
> When the handshake response has been written, a `ShellStarted` event is emitted on the event feed.
> Every session that emits it ends with a `ShellExited` event, including one that ends before its job has started.

> i[shell.exit]
> When a session ends, the server writes a final JSON frame `{ "exit_code": <int> }` to the server-to-client direction of the session stream, then closes its write-end.
> Signal-terminated processes report a negative exit code.
> A session that fails after its handshake response, such as when the shell's script fails, its image cannot be pulled, or its job cannot be started, ends the same way with an `exit_code` of `-1`.
> The reason for such a failure is written to the stderr stream before it closes.
> A `ShellExited` event is also emitted on the event feed.

> i[shell.cleanup]
> Dynamic resources created within a shell session are cleaned up by the runtime when the session ends, as specified in the runtime spec.
> The session's job is terminated and its pod network removed however the session ends, including when the job could not be given the stdin the client sent with its request.

> i[shell.concurrent]
> Shell sessions may run concurrently with lifecycle operations and with other shell sessions.

## Volume Shells

<!-- from interface: Volume Shells -->

> i[volumes.shell]
> `/volumes/shell { volumes, rows, cols, read_only? }` opens an interactive shell session inside an ephemeral container with one or more volumes bind-mounted at `/mnt/{display-name}`.
>
> `volumes` is an array of volume references. Each reference is one of:
> - `{ "kind": "site", "name": "<name>" }` — a site volume.
> - `{ "kind": "app", "app": "<app>", "volume": "<volume>" }` — a volume owned by an app.
>
> The `display-name` for a site volume is its `name`; for an app volume it is `<app>.<volume>`.
> Display names are sanitised to be valid path components before use as mount-point suffixes.
> Each volume is mounted read-only if the volume is inherently read-only (snapshot site volumes); otherwise it is mounted read-write — unless the request sets `read_only: true`, in which case every mount is forced read-only regardless of the underlying volume kind. `read_only` defaults to `false`.
>
> Volume shells use the same three-stream wire protocol as regular shell sessions (see [stream.shell](rule:i/stream.shell)).
> They are registered in the shell session registry with `app = "_volumes"` and `name` set to the comma-separated list of display names; they appear in `/shells/list`, support `/shells/resize` and `/shells/stop`, and emit `ShellStarted`/`ShellExited` events identically to regular shell sessions.
> Returns `not_found` if any referenced volume does not exist.

> i[volumes.shell.caps]
> The shell container holds enough Linux capabilities for root inside the container to behave as root over the bind-mounted volumes — at minimum `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_CHOWN`, and `CAP_FOWNER`.
> Without them, the seedling-default `--cap-drop=ALL` would leave shell-root unable to traverse or edit files owned by service-specific UIDs (e.g. PostgreSQL's `0700 999:999` data directory), which defeats the point of opening the shell.

> i[volumes.shell.image]
> The volume-shell container image must include, at minimum, an interactive shell, terminal-based text editors, file-transfer / archive utilities, and a pager. Operators reach for these tools when poking at volume contents, and the image is the only context Seedling provides for that work.
>
> The image is a runtime concern, not a contract: implementations may build it locally on first use rather than pull it from a registry.
