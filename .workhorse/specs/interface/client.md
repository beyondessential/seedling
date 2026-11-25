---
id: CLI
---

# Reference client

Applies to the [CLI](#cli), the reference command-line client of the [Operator Interface](../overview.md#operator-interface).

## Borrowed terms

| term | meaning |
| --- | --- |
| gitignore pattern | a pattern selecting files to leave out, as defined in [git's gitignore documentation](https://git-scm.com/docs/gitignore) |

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

## Server trust

The CLI MUST connect to the server at the address its `--endpoint` option gives, as `host:port`, where `host` is a hostname or an IP address literal.
The default endpoint is `[::1]:7891`.

With `--fingerprint`, the CLI MUST authenticate the server against that [fingerprint](transport.md#fingerprint) alone.

Without `--fingerprint`, the CLI MUST keep the fingerprints it trusts in the file `seedling/known_hosts` under the user's [state directory](../platform/linux.md#per-user-state).
Each line of the file is blank, a comment beginning with `#`, or an entry: an endpoint as given to `--endpoint`, a single space, and the fingerprint trusted for that endpoint.
Before each connection, the CLI MUST learn the server's fingerprint with a [fingerprint probe](transport.md#fingerprint-probe) and compare it with the file's entry for the endpoint.
If the file has no entry for the endpoint, the CLI MUST show the fingerprint, MUST connect only if the operator confirms it, and MUST then add an entry for it.
If the CLI cannot write the file, it SHOULD warn the operator and connect.
If the entry differs from the probed fingerprint, the CLI MUST refuse to connect, MUST report both fingerprints and the file's path, and MUST leave the entry unchanged.
If the file exists and cannot be read, the CLI MUST refuse to connect and MUST report the file's path.
The CLI MUST NOT write to a file it could not read.

## Shutdown

The CLI MUST put the local terminal into raw mode for a shell session, and MUST restore it when the session ends.
In raw mode, the CLI MUST pass every control character typed at the terminal, including the interrupt character, to the remote shell as input.
On SIGTERM during a shell session, the CLI MUST send ETX (`0x03`) on the session stream's stdin direction, then wait up to five seconds for the session to end.
If the session has not ended by then, the CLI MUST close the connection and exit with status 130.
When a shell session ends, the CLI MUST exit with the session's `exit_code`, or with status 255 where `exit_code` is `-1`.

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

## Action and shell parameters

`apps action <app> <name> [key[=value]]...` invokes an action, and `apps shell <app> <name> [key[=value]]...` opens a shell, with the parameters given after the name.
In both, a `key=value` argument MUST become the parameter `key` with the string value `value`, split at the first `=`.
A bare `key` MUST become the parameter `key` with the JSON boolean `true`.

`apps cancel-action <app>` MUST [cancel](actions.md#cancellation) the app's lifecycle operation.

`apps gate run <app> <gate>` MUST [run](app-control.md#gate-runs) the named gate.

## Definition sources

`apps create <app> <source>` and `apps update <app> <source>` MUST each take a definition.
`templates create <name> <source>` MUST take a definition, and `templates update <name> [<source>]` MAY take one, leaving the template's definition unchanged when none is given.
In each, `--ref <reference>` MAY stand in place of the source.
The CLI MUST refuse a command that gives both a source and `--ref`.

The CLI MUST send a definition as [definition sources](definitions.md#definition-sources) define, by the form of the source.

| source | sent as |
| --- | --- |
| a path to a file | a bundle holding the file under its own name and a [metadata file](../language/bundle.md#metadata-file) whose only field is `script`, naming that file, as `bundle` |
| a path to a folder | the folder as `bundle`, read as [Definition folders](#definition-folders) specifies |
| `https://github.com/<owner>/<repo>/tree/<ref>/<folder>` | the named folder at the named branch, tag or commit as `bundle` |
| `--ref <reference>` | the reference as `reference`, for the server to fetch |

For a GitHub URL, the CLI MUST resolve the branch, tag or commit to a commit, download the folder at that commit itself, and send `origin` with `url` set to the URL as given and `revision` set to the commit's id.
Where the URL's path could split into a ref and a folder in several ways, the CLI MUST try the shortest ref first.
The CLI MAY authenticate to GitHub with the token in the `GITHUB_TOKEN` environment variable.
The CLI MUST report an error and send no request when the folder holds no files, or when an entry the CLI reads from the downloaded archive is not a regular file or directory, or lies outside the repository.

The CLI MUST report an error and send no request when a path to a file names a file called `seedling.toml`, or one whose name is not valid UTF-8.

> [!NOTE]
> Downloading the folder on the client means the host needs no access to GitHub.

> [!NOTE]
> Sending a file under its own name means its script errors name the file its author wrote, and an [export](#definition-export) gives that file back.

## Definition folders

When reading a folder, the CLI MUST leave out version-control metadata directories, such as `.git` and `.jj`, and anything a `.seedignore` file in the folder or one of its subfolders excludes, read as gitignore patterns.
If a symlink or other non-regular file is not left out, the CLI MUST report an error naming it and send no request.

### Include assembly

When the folder's [metadata file](../language/bundle.md#metadata-file) has an `include` table, the CLI MUST assemble the bundle from each key's source:

- where the source names a regular file, the CLI MUST place its contents at the key's bundle path;
- where the source names a directory, the CLI MUST read it as it reads a definition folder, and place each file it yields at the key's bundle path joined with the file's path relative to the directory.

The include boundary is the directory within which every source the CLI reads lies:

- for a folder on disk, the nearest directory at or above the folder that holds a `.git` or `.jj` entry, or the folder itself where none does;
- for a GitHub URL, the root of the repository at the resolved commit, from which the CLI MUST read each source at that same commit.

A key's source path is the folder's path relative to the include boundary joined with the key's value, read with `/` as its separator, and normalised lexically as a [bundle path](../language/bundle.md#bundle-paths) is.
A key's source is out of bounds when its value is absolute or contains a null byte, when its source path rises above the include boundary at any `..` segment during normalisation, or when a segment of its source path names a version-control metadata directory, such as `.git` or `.jj`.
The CLI MUST NOT read a source that is out of bounds.

The CLI MUST report an error naming the key and send no request when a source it reads:

- is reached from the include boundary through a symlink, or is itself a symlink or anything other than a regular file or a directory;
- is a directory that yields no file, or yields a file named `seedling.toml` at any depth.

Where the folder itself holds files at or beneath a key's bundle path, once `.seedignore` exclusions are applied, the CLI MUST use the folder's files when the key's source is out of bounds or names nothing.
Otherwise, the CLI MUST report an error naming the key and send no request unless the folder's files there have the same paths and contents as the files the source yields.

Where the folder holds no file at or beneath a key's bundle path, the CLI MUST report an error naming the key and send no request when the key's source is out of bounds or names nothing.
An [included file](../language/bundle.md#included-file) MUST be placed in the bundle whether or not a `.seedignore` pattern in the folder matches its bundle path.

> [!NOTE]
> Confining sources to the repository means a definition read from a GitHub URL and the same definition pushed from a clone of that repository assemble from the same files, and a badly written `include` cannot reach past the repository on either path.

> [!NOTE]
> Only the folder's own metadata file is read, so a `seedling.toml` in a shared directory is refused rather than copied in where it would look as though it applied.

> [!NOTE]
> Using the folder's own copy where the source is out of bounds or absent lets an [exported](#definition-export) bundle be pushed again as it stands, wherever it was exported to, while a stale copy is caught whenever its source can be read.

## Parameter changes

`apps params set <app> [name=value]... [--unset <name>]...` MUST send every change it is given in one [parameter change](params.md#parameter-changes) request.
Each `name=value` argument MUST set the parameter `name` to the string `value`, split at the first `=`, and each `--unset name` MUST unset the parameter `name`.

`apps update` MUST accept parameter changes alongside the definition, as `--set name=value` and `--unset name`, each of which MAY be repeated, and MUST send them as `params` in the one [update](apps.md#update) request.

The CLI MUST report an error and send no request when an argument that sets a parameter has no `=`, when one command changes the same parameter more than once, or when `apps params set` is given no change.

## Seed rotation

`apps seed rotate <app>` MUST send a [seed rotation](app-control.md#seed-rotation) request only when given `--yes`.
Without it, the CLI MUST send no request, and MUST report that every value derived from the app's seed will change and every workload holding one will restart.

## Definition export

`apps export <app> <folder> [--generation <n>]` MUST write the app's [definition bundle](../overview.md#definition-bundle) at generation `n`, or at the current generation, into `folder`, reproducing the bundle's files exactly.
If `folder` exists and is not empty, the CLI MUST report an error and write nothing.
If a path in the bundle is absolute, or has an empty or `..` segment, the CLI MUST report an error and write nothing.

## Definition packing

`definitions pack <source> <file>` MUST write the bundle that `source` yields, read and assembled as a definition source sent as `bundle` is, to `file` as the layer of a [definition artefact](definitions.md#fetching): a gzip-compressed tar archive whose root is the bundle root, holding only regular files and the directories containing them.
`source` MUST be a path to a file, a path to a folder, or a GitHub URL, as [definition sources](#definition-sources) give them.
If `file` exists, the CLI MUST report an error and write nothing.
The CLI MUST apply every [bundle rule](definitions.md#bundle-rules) to the bundle, and MUST report an error naming the offending path or field and write nothing when one is broken.
Once it has written `file`, the CLI MUST report the bundle's declared [Seedling version requirement](../language/bundle.md#seedling-version-requirement), where it declares one, as the text to give the `vnd.bes.seedling.versions` annotation.
Packing the same bundle twice with the same CLI MUST write the same bytes.

> [!NOTE]
> A packed bundle is pushed to a registry with a general-purpose OCI tool, so the CLI holds no registry credentials of its own.
> Writing the same bytes for the same bundle gives an unchanged definition the same digest when it is published again.
> A host refuses a fetched bundle whose annotation differs from its declared requirement, so reporting the requirement lets it be carried to the push unchanged.

## Definition checks

`definitions check <source>` MUST check a definition without connecting to a server, using the evaluator of the Seedling version the CLI belongs to.
It MUST take its source in any form `apps create` takes, as [definition sources](#definition-sources) specifies, including `--ref <reference>` in place of the source.

For `--ref`, the CLI MUST fetch the bundle itself, as [fetching](definitions.md#fetching) specifies for the server, with its own Seedling version as the running version in [artefact selection](definitions.md#artefact-selection).
It MUST refuse a reference that the server would refuse before making any request to a registry.
It MUST authenticate to a registry with the credentials the container engine of the user running it would use to pull images.

The CLI MUST check, in this order, and MUST stop at the first check that fails:

1. that the CLI's own Seedling version satisfies the bundle's [Seedling version requirement](../language/bundle.md#seedling-version-requirement), before any other part of the metadata is read;
2. that the bundle keeps the [bundle rules](definitions.md#bundle-rules);
3. that the script [evaluates](../language/overview.md#evaluation) without a script error;
4. that the [sample parameter values](#sample-parameter-values) pass [validation](params.md#validation).

The CLI MUST evaluate the script as the server evaluates an app's definition on [update](apps.md#update), with the sample parameter values in place of the app's stored values and the [check's host constants](#check-host-constants) in place of the host's.
It MUST NOT run any gate, gate check or action closure.

`definitions check` MUST accept each daemon option of the [engine limits](../runtime/engine-limits.md#limits), with the same name, default and meaning, and MUST evaluate within the limits so set.
It MUST refuse a value the daemon would refuse to start with, report an error, and check nothing.

> [!NOTE]
> A definition written for hosts whose daemons set their own limits is checked with those limits given, so that the check is neither stricter nor looser than those hosts.

### Check results

When the source cannot be read or fetched, or a check fails, the CLI MUST report the errors below and exit with a non-zero status.
When every check passes, the CLI MUST exit with status zero.

| kind | reported when | the error names |
| --- | --- | --- |
| `source` | the source cannot be read or fetched | the cause |
| `version` | the version requirement is not satisfied | the requirement and the CLI's version |
| `bundle` | the bundle breaks a bundle rule | the offending path or field |
| `script` | evaluation fails | the script file and the line within it, as [script formation](../language/bundle.md#script-formation) specifies |
| `validation` | a sample parameter value is rejected | the rejecting parameter and its reason, with one error for each such parameter |

The location of an error is the file it lies in and, where it has one, the line within that file.
For a file or folder source, the file MUST be given as the path the source names joined with the file's bundle path, and otherwise as the bundle path alone.

Without `--json`, the CLI MUST print each error to standard error, beginning on a line of its own.
An error with a location MUST begin with the file, then `:` and the line where it has one, then `: ` and the error's message.

With `--json`, the CLI MUST print to standard output a single JSON object, `{ "errors": [...] }`, holding one object per error, and an empty array when every check passes.

| member | type | meaning |
| --- | --- | --- |
| `kind` | string | the error's kind, from the table above |
| `message` | string | the error's message |
| `file` | string or null | the file of the error's location, or `null` where it has none |
| `line` | integer or null | the line of the error's location, or `null` where it has none |
| `parameter` | string or null | for a `validation` error, the rejecting parameter, and otherwise `null` |

> [!NOTE]
> The `file:line: message` form is the one editors and CI annotation matchers read from compilers.

### Sample parameter values

`definitions check` MUST accept `--set name=value`, which MAY be repeated.
Each MUST set the parameter `name` to the string `value`, split at the first `=`.
The CLI MUST evaluate the script with the parameters so set as its only stored parameter values, and MUST then validate them as [proposed values](params.md#proposed-values).
A sample value for a parameter the definition does not declare MUST fail validation, with a reason saying the parameter is not declared.

The CLI MUST report an error and check nothing when an argument to `--set` has no `=`, or when the same parameter is set more than once.

### Check host constants

`definitions check` MUST evaluate the script with each [host constant](../language/constants.md#host-constants) at its default below, unless an option sets it.

| constant | default |
| --- | --- |
| `AVAILABLE_THREADS` | the number of compute threads available to the CLI |
| `AVAILABLE_MEMORY` | the amount of memory, in bytes, of the machine the CLI runs on |
| `CPU_ARCHITECTURE` | the CPU architecture of the machine the CLI runs on |
| `HOST_HAS_IPV4`, `HOST_HAS_IPV6`, `NAT64_ACTIVE`, `HAS_SNAPSHOTS` | `false` |
| `NODE_NAME` | the empty string |
| `TIMEZONE` | `UTC` |

`definitions check` MUST accept `--const NAME=value`, which MAY be repeated, setting the host constant `NAME` to `value`.
The CLI MUST read `value` as the constant's type: a decimal integer for an integer constant, `true` or `false` for a boolean constant, and the text as given for a string constant.

The CLI MUST report an error and check nothing when `NAME` is not a host constant, when the same constant is set more than once, or when `value` is not of the constant's type or breaks the rules [host constants](../language/constants.md#host-constants) give for it.
If the CLI cannot determine a default that no option sets, it MUST report an error naming the constant and check nothing.

> [!NOTE]
> A definition that branches on the host is checked for each kind of host it will run on by running the check once per kind, such as with `--const CPU_ARCHITECTURE=aarch64`.

## Registry tags

`registries tags <repository>` MUST list a repository's tags as [repository tags](definitions.md#repository-tags) does.

## Canopy access

`canopy status` MUST show the [Canopy access status](canopy-relay.md#status).
`canopy enable` and `canopy disable` MUST turn Canopy access on and off through the [Canopy settings](canopy-relay.md#settings).

The CLI MUST NOT offer commands that register or withdraw a Canopy offer, or that relay a request to Canopy.

> [!NOTE]
> An offer is made by the client that carries the requests, over its own connection, and an operator has no such connection to present.
> A command relaying an arbitrary request would give every authorised operator the full authority of the offering client's Canopy identity.
