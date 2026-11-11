---
id: CTR
---

# Containers

Applies to the BSL `Container` interface: the builder methods and semantics every container workload shares.

## Borrowed terms

| term | meaning |
| --- | --- |
| entrypoint, command arguments | the `Entrypoint` and `Cmd` of an image's configuration, as defined by the [OCI Image Format Specification](https://github.com/opencontainers/image-spec/blob/main/config.md) |
| capability | a Linux capability, as defined by [capabilities(7)](https://man7.org/linux/man-pages/man7/capabilities.7.html) |
| signal | a POSIX signal, as defined by [signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html) |

## Container interface

`Container` is an interface, and no value has the type `Container`.
[Deployments](deployment.md) and [Jobs](job.md) implement it, through the [Pod](pod.md) interface.
Every method in this spec is a [builder method](rule:l/bsl.builder) available on each type that implements `Container`.

## Image

`container.image(uri: string)` sets the container's image reference.
A reference MUST have the form `registry/path`, followed by `:tag`, `@digest`, or both, and any other reference MUST throw.

| part | form |
| --- | --- |
| `registry` | the text before the first `/`, containing at least one `.` or `:` |
| `path` | one or more segments separated by `/`, each non-empty and of lowercase ASCII letters, digits, `.`, `_` and `-` |
| `tag` | the text after the last `:` following `registry/`, non-empty and of ASCII letters, digits, `.`, `_` and `-` |
| `digest` | `algorithm:hex`, where `algorithm` is non-empty and of lowercase ASCII letters and digits, and `hex` is at least 32 lowercase hexadecimal digits |

A reference with neither a tag nor a digest MUST throw.

## Registry allowlist

Each host holds a registry allowlist, which operators edit through the [Operator Interface](../overview.md#operator-interface).
A new host's allowlist MUST hold `docker.io` and `ghcr.io`.

Each time the runtime evaluates an app's definition, it MUST compare the registry of every Deployment and Job image against the allowlist.
While any image's registry is absent from the allowlist, the runtime MUST keep one [fault](../overview.md#fault) of kind `disallowed_registry` filed against the app, naming each disallowed registry.
The runtime MUST clear that fault when an evaluation finds every image's registry in the allowlist.
If the runtime cannot read the allowlist, it MUST NOT file or clear the fault.

## Command and arguments

`container.command(entrypoint: array)` sets the container's [entrypoint](#borrowed-terms) to `entrypoint`, replacing the image's.
`container.command(name: string)` is equivalent to `command([name])`.

`container.arg(vars: array)` appends `vars` to the container's [command arguments](#borrowed-terms).
`container.arg(var: string)` is equivalent to `arg([var])`.
A container on which `arg` is called at least once MUST be run with the appended arguments in place of the image's.

An array element that is not a string MUST throw.

## Environment

`container.env(name: string, value: string)` sets the environment variable `name` to `value`.
`container.env(entries: array)` sets one variable for each entry, a map of the form `#{ name: string, value: string }`.
An entry missing `name` or `value`, or carrying any other key, MUST throw.

Setting a variable already set MUST replace its value.

A name MUST be non-empty, MUST consist of ASCII letters, digits and `_`, and MUST NOT begin with a digit.
The names `PATH`, `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `LD_DEBUG` and `LD_PROFILE` MUST throw.
A value containing a NUL character MUST throw.

## Volume mounts

`container.mount(mountpoint: string, volume: Volume)` mounts a [volume](volume.md) into the container's filesystem at `mountpoint`.
`container.mount(mountpoint: string, volume: ExternalVolume)` mounts an [External Volume](volume.md#external-volumes) in the same way.
A mount at a mountpoint already mounted MUST replace the earlier mount.

A `mountpoint` MUST be an absolute path beginning with `/`, and MUST NOT contain a NUL character.
A mountpoint whose canonical form is `/`, `/proc`, `/sys`, `/dev`, `/etc`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr`, `/boot` or `/run` MUST throw.
The canonical form resolves `.` and `..` segments and collapses repeated `/`, without reference to any filesystem.

## Exit strategy

`container.on_exit(strategy: OnExit)` sets the strategy the runtime follows when the container's command exits.
The default is [`OnExit.Restart`](rule:l/const.on-exit.restart) for a Deployment and [`OnExit.Terminate`](rule:l/const.on-exit.terminate) for a Job.

## Resource limits

`container.memory(limit: string)` sets the container's memory limit.
A `limit` MUST be a decimal integer of at least 1, without leading zeros, followed by one unit suffix.
The suffixes are `k` for kibibytes, `m` for mebibytes and `g` for gibibytes, in either case.
Any other `limit` MUST throw.

`container.cpus(limit: float)` sets the container's CPU limit, in CPU cores.
A `limit` MUST be finite and greater than zero, and MAY be fractional.

A container with no memory limit or no CPU limit is not constrained in that resource.

`container.pids_limit(limit: number)` sets the most processes the container may hold at once.
A `limit` MUST be an integer from 1 to 4294967295.
The default is 256.

## Privileges

The runtime starts every container with the restricted privileges [container hardening](rule:r/actuate.container.hardening) specifies.

`container.cap_add(capability: string)` grants the container one [capability](#borrowed-terms).
A `capability` MUST be one of `AUDIT_CONTROL`, `AUDIT_READ`, `AUDIT_WRITE`, `BLOCK_SUSPEND`, `BPF`, `CHECKPOINT_RESTORE`, `CHOWN`, `DAC_OVERRIDE`, `DAC_READ_SEARCH`, `FOWNER`, `FSETID`, `IPC_LOCK`, `IPC_OWNER`, `KILL`, `LEASE`, `LINUX_IMMUTABLE`, `MAC_ADMIN`, `MAC_OVERRIDE`, `MKNOD`, `NET_ADMIN`, `NET_BIND_SERVICE`, `NET_BROADCAST`, `NET_RAW`, `PERFMON`, `SETFCAP`, `SETGID`, `SETPCAP`, `SETUID`, `SYS_ADMIN`, `SYS_BOOT`, `SYS_CHROOT`, `SYS_MODULE`, `SYS_NICE`, `SYS_PACCT`, `SYS_PTRACE`, `SYS_RAWIO`, `SYS_RESOURCE`, `SYS_TIME`, `SYS_TTY_CONFIG`, `SYSLOG` or `WAKE_ALARM`, compared case-insensitively, and any other name MUST throw.
Granting a capability already granted has no further effect.

`container.writable_rootfs()` makes the container's root filesystem writable.
A container on which it is never called MUST have a read-only root filesystem, with a writable temporary filesystem at `/tmp`.

`container.user(spec: string)` sets the user, and optionally the group, the container's process runs as.
A `spec` is a numeric user id, a `uid:gid` pair, or a name defined in the image.
Leading and trailing whitespace MUST be removed from `spec`, and a `spec` that is then empty or contains whitespace MUST throw.
A container on which `user` is never called runs as the user its image names.

> [!NOTE]
> A container that must run as a non-root user needs `user`, since [container user](rule:r/actuate.container.user) prevents an entrypoint from dropping privileges itself.

## Working directory

`container.workdir(path: string)` sets the working directory of the container's process.
A `path` not beginning with `/` MUST throw.
A container on which `workdir` is never called uses the working directory its image names.

## Stopping

`container.stop_signal(name: string)` sets the [signal](#borrowed-terms) the runtime sends to the container's main process to stop it.
A `name` MUST be one of `HUP`, `INT`, `QUIT`, `ILL`, `TRAP`, `ABRT`, `BUS`, `FPE`, `KILL`, `USR1`, `SEGV`, `USR2`, `PIPE`, `ALRM`, `TERM`, `STKFLT`, `CHLD`, `CONT`, `STOP`, `TSTP`, `TTIN`, `TTOU`, `URG`, `XCPU`, `XFSZ`, `VTALRM`, `PROF`, `WINCH`, `IO`, `POLL`, `PWR` or `SYS`, optionally prefixed with `SIG`.
A `name` MUST be compared case-insensitively, ignoring leading and trailing whitespace, and any other name MUST throw.
If a container sets no stop signal, the runtime MUST send `SIGTERM`.

`container.stop_timeout(seconds: number)` sets how long the runtime waits after sending the stop signal before sending `SIGKILL`.
A `seconds` MUST be an integer from 1 to 4294967295.
If a container sets no stop timeout, the runtime SHOULD wait 90 seconds.
