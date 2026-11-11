---
id: SHL
---

# Shell sessions

Applies to interactive terminal sessions that operators open through the [Operator Interface](../overview.md#operator-interface), on an [app](../overview.md#app)'s shells and on volumes.

## Borrowed terms

| term | meaning |
| --- | --- |
| bidirectional stream, unidirectional stream, stream ID | as defined by QUIC, [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) section 2 |

## Terminology

### Shell session

An interactive terminal an operator holds open on a job the server runs for it, from the request that opens it until it ends.

### Session stream

The client-initiated bidirectional stream that carries a shell session's opening request, its stdin, its handshake response and its exit frame.

### Volume reference

A JSON object naming one volume on the host by its `kind`.

| `kind` | other members | names |
| --- | --- | --- |
| `"site"` | `name` (string) | the site volume of that name |
| `"app"` | `app` (string), `volume` (string) | the volume `volume` declared by the app `app` |
| `"held"` | `id` (string) | the [held volume](../runtime/actuation.md#held-volume) with that identifier |

## Opening an app shell

A client opens a [shell session](#shell-session) on one of an app's [shells](rule:l/action.shell) by sending `/shells/start` on a new [session stream](#session-stream), framed as [shell stream framing](rule:i/stream.shell.framing) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `name` | string | yes | the shell's name |
| `rows` | integer, 0 to 65535 | yes | the terminal's initial height in rows |
| `cols` | integer, 0 to 65535 | yes | the terminal's initial width in columns |
| `params` | object of strings | no | parameter values for the shell, keyed by parameter name |

The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `not_installed` unless the app is `Installed`.
The server MUST reject a request naming a shell the app does not declare with `not_found`.
A `params` key ending in `_volume` or `_filename` is [reserved](rule:r/operation.volume-param.reserved), and the server MUST reject a request carrying one with `requirements_invalid`.
Where the shell declares parameters, the server MUST validate `params` and fill in defaults as for [install parameters](actions.md#install-parameters).

On success the handshake response's result is:

| member | type | meaning |
| --- | --- | --- |
| `session_id` | string | the session's identifier, a UUID in its hyphenated text form |
| `stdout_stream_id` | integer | the stdout stream's QUIC stream ID shifted right by two bits |
| `stderr_stream_id` | integer | the stderr stream's QUIC stream ID shifted right by two bits |

> [!NOTE]
> A stream ID shifted right by two bits is the stream's sequence number among the server-initiated unidirectional streams of its connection.

The server MUST open the stdout and stderr streams as server-initiated unidirectional streams before it writes the handshake response.
The server MUST write the session terminal's output to the stdout stream.
The server MAY write its own diagnostics about the session to the stderr stream.

The server MUST treat every byte the client sends on the session stream after the request line as stdin for the session's job, including bytes that arrived in the same read as the request line.

The server MUST NOT write any other frame on the session stream between the handshake response and the [exit frame](#session-end).

## Session startup

The server MUST register a session under its `session_id` before it writes the handshake response carrying that identifier.
The session MUST stay registered while its shell starts, which can include running the shell's script, pulling its image and creating its network.

From the time the client holds the identifier, the session MUST appear in [`/shells/list`](#listing-sessions), and [`/shells/stop`](#stopping-a-session) MUST end it without waiting for the shell to start.
A [`/shells/resize`](#resizing-a-terminal) received before the terminal exists MUST take effect as soon as the terminal exists.

A session stopped before its job runs MUST end without the job having started, and the server MUST remove every container and network the session had created.
A session stopped while the shell's script runs MUST end only once the script has stopped, so that the script creates nothing after the session has ended.

A session that fails after its handshake response, for example because the shell's script fails, the script calls `shell.error`, the image cannot be pulled or the job cannot be started, MUST end with an `exit_code` of `-1`.
The server MUST write the reason for such a failure to the stderr stream before closing it.

## Session end

A session ends when any of the following occurs:

- the session's job exits;
- the client finishes its sending side of the session stream;
- the stdout stream cannot be written;
- the connection is lost;
- an operator stops the session with `/shells/stop`.

When a session ends, the server MUST write the exit frame `{ "exit_code": <integer> }` to the session stream and then finish its sending side of the session stream.
The server MUST finish the stdout and stderr streams when the session ends.

Where the job exited with a status, `exit_code` MUST be that status.
Where the job was terminated by a signal, `exit_code` MUST be negative.

However the session ends, the server MUST terminate the session's job and remove the network created for it, including when the stdin sent with the request could not be delivered to the job.
The runtime MUST clean up resources created within the session as [shell resources](rule:r/operation.shell.resources) specifies.

## Session events

When the server has written a session's handshake response, it MUST emit a `ShellStarted` event on the [event feed](events.md).
Every session that emitted `ShellStarted` MUST emit a `ShellExited` event when it ends, including a session that ends before its job has started.

## Resizing a terminal

`/shells/resize` changes a running session's terminal dimensions.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `session_id` | string | yes | the session's identifier |
| `rows` | integer, 0 to 65535 | yes | the terminal's height in rows |
| `cols` | integer, 0 to 65535 | yes | the terminal's width in columns |

On success the result is `{}`.
The server MUST reject a `session_id` that is not a UUID with `requirements_invalid`.
The server MUST reject a `session_id` that names no registered session with `not_found`.

## Listing sessions

`/shells/list` lists the registered shell sessions.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's sessions |

The result is `{ "shells": [ ... ] }`, an array of shell records.
Without `app` the server MUST list every registered session on the host.

| member | type | meaning |
| --- | --- | --- |
| `session_id` | string | the session's identifier |
| `app` | string | the app the session belongs to, or `"_volumes"` for a [volume shell](#volume-shells) |
| `name` | string | the shell's name, or for a volume shell its mounts' display names |
| `opened_at` | string | when the session was opened, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `actor` | object or null | the [actor](rule:i/wire.actor) the opening request carried |

## Stopping a session

`/shells/stop` ends a registered session.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `session_id` | string | yes | the session's identifier |

On success the result is `{}`.
The server MUST let any operator stop any session, whichever connection opened it.
The server MUST reject a `session_id` that is not a UUID with `requirements_invalid`.
The server MUST reject a `session_id` that names no registered session with `not_found`.
A stopped session MUST end as [session end](#session-end) specifies.

## Concurrency

The server MUST allow shell sessions to run concurrently with lifecycle operations and with other shell sessions.

## Volume shells

`/volumes/shell` opens a shell session in a container with one or more volumes mounted, on a new session stream framed as for `/shells/start`.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `volumes` | array of [volume references](#volume-reference) | yes | the volumes to mount |
| `rows` | integer, 0 to 65535 | yes | the terminal's initial height in rows |
| `cols` | integer, 0 to 65535 | yes | the terminal's initial width in columns |
| `read_only` | boolean | no, default `false` | when `true`, every volume is mounted read-only |

The server MUST reject an empty `volumes` with `requirements_invalid`.
The server MUST reject a request referencing a volume that does not exist, or whose storage does not exist, with `not_found`.
If the server cannot prepare the volume shell's image or network, it MUST answer with an error response in place of the handshake response.

The server MUST mount each volume at `/mnt/<display name>`.
A site volume's display name is its `name`.
An app volume's display name is `<app>.<volume>`.
A held volume's display name is `held-<app>-<volume>`, naming the app and volume it was held from, or `held-<id>` where those are unknown.
Before use, the server MUST replace each character of a display name that is not an ASCII letter, ASCII digit, `-` or `.` with `_`, and then remove leading and trailing `-` and `.` characters.

The server MUST mount a snapshot site volume read-only.
The server MUST mount every other volume read-write unless `read_only` is `true`.
The session's working directory SHOULD be the mount point when one volume is mounted, and `/mnt` when several are.

A volume shell is a shell session in every other respect: its handshake response, streams, startup, end, `ShellStarted` and `ShellExited` events, listing, resizing and stopping are as for an app shell.
The server MUST register a volume shell under the app name `"_volumes"`, which no app can hold, with the display names of its mounts joined by `", "` as its name.
A volume shell's `ShellStarted` event MUST carry `app` `"_volumes"` and `name` `"volume-shell"`.

### Volume shell privileges

The root user in a volume shell MUST be able to read, traverse, modify, and change the ownership and mode of every file on a writable mounted volume, whichever user owns it.

> [!NOTE]
> Workload containers drop every Linux capability by default, which leaves root inside them subject to file permission checks.
> A volume shell restores `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_CHOWN` and `CAP_FOWNER`; without them root cannot enter a directory such as a database's `0700` data directory owned by another user.

### Volume shell image

The volume shell's image MUST provide an interactive shell, terminal text editors, file-transfer and archive utilities, and a pager.
The server MAY build the image on the host when it is first needed rather than pull it from a registry.
