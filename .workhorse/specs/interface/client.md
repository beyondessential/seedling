---
id: CLI
---

# Reference client

Applies to the [CLI](#cli), the reference command-line client of the [Operator Interface](../overview.md#operator-interface).

## Terminology

### CLI

`seedling-ctl`, the command-line program through which an [operator](../overview.md#operator) uses the Operator Interface.
Commands in this spec are written without the program name: `apps create` is `seedling-ctl apps create`.

## Client identity key

The CLI authenticates with an Ed25519 key ([RFC 8032](https://www.rfc-editor.org/rfc/rfc8032)) held in a key file as a DER-encoded PKCS#8 private key ([RFC 5958](https://www.rfc-editor.org/rfc/rfc5958), [RFC 8410](https://www.rfc-editor.org/rfc/rfc8410)).

The CLI MUST read the key file from the path given by its `--key-file` option.
Without the option, the CLI MUST use `seedling/client.key` under the user's [state directory](../platform/linux.md#per-user-state).
If the key file does not exist, the CLI MUST generate a key and write it there, creating missing parent directories.

The CLI MUST create the key file readable and writable by its owner only.
The CLI MUST refuse to load a key file that grants any permission to its group or to other users, and MUST report an error naming the file.

> [!NOTE]
> A distinct key file gives a caller a distinct identity, such as a co-located service that needs its own authorised key.
> A key can be generated at a chosen path and authorised before its first use.

## Shutdown

The CLI MUST put the local terminal into raw mode for a shell session, and MUST restore it when the session ends.
In raw mode, the CLI MUST pass every control character typed at the terminal, including the interrupt character, to the remote shell as input.
On SIGTERM during a shell session, the CLI MUST send ETX (`0x03`) on the session stream's stdin direction, then wait up to five seconds for the session to end.
If the session has not ended by then, the CLI MUST close the connection and exit with status 130.
When a shell session ends, the CLI MUST exit with the session's exit code.

On SIGINT or SIGTERM during a [port forward](#port-forward-statistics), the CLI MUST close the forward's control stream and exit.

On SIGINT or SIGTERM during an event subscription, or while following logs, the CLI MUST close the connection and exit.

## Event subscription

When the event stream or its connection is lost, the CLI MUST reconnect and subscribe again.
The CLI MUST wait between connection attempts, starting at one second and doubling after each failed attempt up to thirty seconds.
After a successful connection, the next wait MUST start again at one second.
If no connection succeeds within five minutes of the stream being lost, the CLI MUST exit with a non-zero status.
If the server refuses the subscription, the CLI MUST exit with a non-zero status without retrying.

## Log display

`apps logs` MUST print one line per log entry, holding the entry's timestamp, its most specific source name, and its message.
The source name SHOULD be the first the entry carries of its instance, resource, app and infrastructure component.
An entry recording a runtime call SHOULD be marked by `>` between its source name and its message, and an entry carrying a script position SHOULD end with that position in parentheses.

With `--json`, the CLI MUST print each entry exactly as received, one per line.
The CLI MUST print a line that is not a JSON object exactly as received.

## Port forward statistics

While a port forward is active, the CLI MUST count the bytes relayed in each direction.
For a TCP forward, it MUST count the connections opened and the connections active.
For a UDP forward, it MUST count the datagrams relayed in each direction, and the datagrams dropped for exceeding the [maximum datagram size](port-forwards.md#borrowed-terms).
When the forward ends, the CLI MUST print a summary of these counts to standard error.

## Action, shell and install parameters

`apps action <app> <name> [key[=value]]...` invokes an action, and `apps shell <app> <name> [key[=value]]...` opens a shell, with the parameters given after the name.
In both, a `key=value` argument MUST become the parameter `key` with the string value `value`, split at the first `=`.
A bare `key` MUST become the parameter `key` with the value `true`.

`apps install <app> [key=value]...` invokes the install action with the parameters given after the app.
Install parameters carry values typed by the app's parameter schema, so each argument MUST be a `key=value` pair.
If an argument holds no `=`, the CLI MUST report an error naming that argument and send no request.

`apps cancel-action <app>` MUST [cancel](actions.md#cancellation) the app's lifecycle operation.

## Definition sources

`apps create <app> <source>` and `apps update <app> <source>` MUST each take a definition.
`templates create <name> <source>` MUST take a definition, and `templates update <name> [<source>]` MAY take one, leaving the template's definition unchanged when none is given.
In each, `--ref <reference>` MAY stand in place of the source.
The CLI MUST refuse a command that gives both a source and `--ref`.

The CLI MUST send a definition as [definition sources](definitions.md#definition-sources) define, by the form of the source.

| source | sent as |
| --- | --- |
| a path to a file | the file's text as `script`, or as `body` for a template |
| a path to a folder | the folder as `bundle`, read as [Definition folders](#definition-folders) specifies |
| `https://github.com/<owner>/<repo>/tree/<ref>/<folder>` | the named folder at the named branch, tag or commit as `bundle` |
| `--ref <reference>` | the reference as `reference`, for the server to fetch |

For a GitHub URL, the CLI MUST resolve the branch, tag or commit to a commit, download the folder at that commit itself, and send `origin` with `url` set to the URL as given and `revision` set to the commit's id.
Where the URL's path could split into a ref and a folder in several ways, the CLI MUST try the shortest ref first.
The CLI MAY authenticate to GitHub with the token in the `GITHUB_TOKEN` environment variable.
The CLI MUST report an error and send no request when the folder holds no files, or when the downloaded archive holds an entry that is not a regular file or directory or that lies outside the folder.

> [!NOTE]
> Downloading the folder on the client means the host needs no access to GitHub.

## Definition folders

When reading a folder, the CLI MUST leave out version-control metadata directories, such as `.git` and `.jj`, and anything a `.seedignore` file in the folder or one of its subfolders excludes, read as gitignore patterns.
If a symlink or other non-regular file is not left out, the CLI MUST report an error naming it and send no request.

## Parameter change on update

`apps update` MUST accept one parameter change alongside the definition, as `--set name=value` or `--unset name`, and MUST NOT accept both.
The CLI MUST send the change as `param`, an object whose `name` is the parameter's name and whose `value` is the new value, or `null` for `--unset`.
If `--set` is given an argument without `=`, the CLI MUST report an error and send no request.

## Definition export

`apps export <app> <folder> [--generation <n>]` MUST write the app's [definition bundle](../overview.md#definition-bundle) at generation `n`, or at the current generation, into `folder`, reproducing the bundle's files exactly.
If `folder` exists and is not empty, the CLI MUST report an error and write nothing.
If a path in the bundle is absolute, or has an empty or `..` segment, the CLI MUST report an error and write nothing.

## Registry tags

`registries tags <repository>` MUST list a repository's tags as [repository tags](definitions.md#repository-tags) does.

## Canopy access

`canopy status` MUST show the [Canopy access status](canopy-relay.md#status).
`canopy enable` and `canopy disable` MUST turn Canopy access on and off through the [Canopy settings](canopy-relay.md#settings).

The CLI MUST NOT offer commands that register or withdraw a Canopy offer, or that relay a request to Canopy.

> [!NOTE]
> An offer is made by the client that carries the requests, over its own connection, and an operator has no such connection to present.
> A command relaying an arbitrary request would give every authorised operator the full authority of the offering client's Canopy identity.
