---
id: CLI
---

# Client behaviour

## Client Behaviour

<!-- from interface: Client Behaviour -->

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
> The CLI exposes `ctl apps cancel-action <app>` as a thin wrapper over [action.cancel](rule:i/action.cancel).

> i[ctl.shell.params]
> The CLI accepts shell params with the same syntax: `ctl apps shell <app> <name> [key[=value]]...`.

> i[ctl.install.params]
> The CLI accepts install params as positional arguments: `ctl apps install <app> [key=value]...`.
> Install params carry values typed by the application's parameter schema rather than free-form JSON, so every argument must be a `key=value` pair.
> An argument without `=` must be reported as an error naming that argument, and no request may be sent.

> i[ctl.definition.source]
> `ctl apps create`, `ctl apps update`, `ctl templates create`, and `ctl templates update` take the definition as one of:
>
> - a path to a file, pushed as a script;
> - a path to a folder, pushed as a bundle;
> - a GitHub URL naming a folder at a branch, tag, or commit, which the CLI resolves to a commit and downloads, then pushes as a bundle with that URL and commit as its reported origin, so the host needs no access to GitHub;
> - `--ref <reference>`, which has the daemon [fetch](rule:i/definition.fetch) it.

> i[ctl.definition.folder]
> When pushing a folder, the CLI leaves out version-control metadata directories and whatever the folder's `.seedignore` file excludes, if it has one, using gitignore pattern syntax.
> A symlink or other non-regular file that is not left out is reported as an error naming it, and no request is sent.

> i[ctl.definition.param]
> `ctl apps update` accepts one parameter change alongside the definition, as `--set name=value` or `--unset name`.

> i[ctl.definition.export]
> `ctl apps export <app> <folder> [--generation <n>]` writes the app's definition bundle at that generation, or the current one, into a new folder, reproducing the bundle's files exactly.
> A folder that already exists and is not empty is reported as an error, and nothing is written.

> i[ctl.registries.tags]
> `ctl registries tags <repository>` lists a repository's tags, as a thin wrapper over [definition.tags](rule:i/definition.tags).

> i[ctl.canopy]
> The CLI exposes the operator-facing side of the Canopy relay: `ctl canopy status`, `ctl canopy enable`, and `ctl canopy disable`, as thin wrappers over [canopy.status](rule:i/canopy.status) and [canopy.settings](rule:i/canopy.settings).
> Registering and withdrawing offers is not exposed: those are made by the client that carries the requests, not by an operator.
> Nor is issuing a request: the relay exists to carry what the runtime itself needs, and an interface for relaying an arbitrary request would hand every authorised operator the full authority of the carrying client's Canopy identity.
