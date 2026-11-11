---
id: CTR
---

# Containers

## Container

<!-- from language: Container -->

> l[container.interface]
> Container is an interface (you can't obtain a `Container`-typed value) for the common builder methods, instance methods, and semantics of container workload definitions.

> l[container.image]
> The `container.image(uri: string)` builder method sets the container image reference.
> Image references must be fully qualified: `registry/path:tag` or `registry/path@algorithm:hex`.
> The registry component is the hostname (with optional port) before the first `/` and must contain at least one `.` or a `:` to distinguish it from a path component.
> A tag or digest must be present; bare `registry/path` references without a version specifier are rejected.
>
> A container without an `image` set may be inoperable.

> l[container.image.registry-allowlist]
> After BSL evaluation, every image reference is checked against the operator-configured registry allowlist.
> If an image's registry is not in the allowlist, a fault of kind `disallowed_registry` is filed for the app.
> The fault is cleared when the app is re-evaluated and all image registries pass the check.
> The default allowlist contains `docker.io` and `ghcr.io`.

> l[container.command]
> The `container.command(name: string)` or `container.command(entrypoint: string[])` builder method overrides the container's entrypoint (the executable to run).
> The `command(name: string)` form is equivalent to `command([name])`.
>
> This follows the [Kubernetes `command:` field](https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/) convention, which maps to `--entrypoint` in Docker/Podman — not to the positional arguments after the image name.

> l[container.arg]
> The `container.arg(var: string)` or `container.arg(vars: string[])` builder method appends arguments passed to the container's entrypoint (overriding the image's default `CMD`).
> The `arg(var: string)` form is equivalent to `arg([var])`.
>
> This follows the [Kubernetes `args:` field](https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/) convention, which maps to the positional arguments after the image name in Docker/Podman.

> l[container.env]
> The `container.env(name: string, value: string)` or `container.env(#{ name: string, value: string }[])` builder method inserts variables into the environment of the container.
> The `env(name: string, value: string)` form is equivalent to `env(#{ name: name, value: value })`.
>
> Environment variables set with the same name as previous variables override the earlier ones. That is, `.env("MANUKA", "honey").env("MANUKA", "branch")` is equivalent to `.env("MANUKA", "branch")`.

> l[container.env.validation]
> Environment variable names must be non-empty, consist only of ASCII letters, digits, and underscores, and must not start with a digit.
>
> The following environment variable names are forbidden and must cause the method to throw:
> `PATH`, `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `LD_DEBUG`, `LD_PROFILE`.
>
> Environment variable values must not contain null bytes. If a value contains a null byte, the method must throw.

> l[container.mount-volume]
> The `container.mount(mountpoint: string, volume: Volume)` builder method binds a [volume](rule:l/volume.type) into the filesystem of the container at a given `mountpoint`.
> An [External Volume](rule:l/volume.external) can also be used.
>
> Mounts bound to a mountpoint identical to a previous mount override the earlier one.
>
> The `mountpoint` argument must be a unix-style path.

> l[container.mount-volume.validation]
> The `mountpoint` must be an absolute path (starting with `/`).
>
> The following mountpoints are forbidden and must cause the method to throw: `/`, `/proc`, `/sys`, `/dev`, `/etc`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr`, `/boot`, `/run`.
> A mountpoint whose canonicalised form (after resolving `.` and `..` segments and collapsing repeated `/` separators, without touching the filesystem) equals any forbidden path is also rejected.
>
> The mountpoint must not contain null bytes. If it does, the method must throw.

> l[container.on-exit]
> The `container.on_exit(strategy: OnExit)` builder method defines the strategy used when the command exits.
> The default is [`OnExit.Restart`](rule:l/const.on-exit.restart) for Deployments and [`OnExit.Terminate`](rule:l/const.on-exit.terminate) for Jobs.

> l[container.memory]
> The `container.memory(limit: string)` builder method sets the memory limit for the container.
> The `limit` must be a positive integer followed by a unit suffix: `k` (kibibytes), `m` (mebibytes), or `g` (gibibytes). The suffix is case-insensitive.
> If no memory limit is set, the container has no memory constraint.

> l[container.cpus]
> The `container.cpus(limit: float)` builder method sets the CPU limit for the container.
> The `limit` must be a positive number. Fractional values are permitted (e.g. `0.5` for half a CPU core).
> If no CPU limit is set, the container has no CPU constraint.

> l[container.cap-add]
> The `container.cap_add(capability: string)` builder method adds a Linux capability to the container.
> Containers run with all capabilities dropped by default; this method selectively re-grants individual capabilities.
> The `capability` must be a valid Linux capability name (e.g. `"NET_RAW"`, `"NET_BIND_SERVICE"`). The name is case-insensitive and is normalised to uppercase.

> l[container.writable-rootfs]
> The `container.writable_rootfs()` builder method opts the container out of the default read-only root filesystem.
> By default, the container's root filesystem is mounted read-only; a writable tmpfs is provided at `/tmp`.

> l[container.pids-limit]
> The `container.pids_limit(limit: int)` builder method sets the maximum number of simultaneous PIDs permitted in the container.
> The `limit` must be a positive integer.
> The default PID limit is 256.

> l[container.workdir]
> The `container.workdir(path: string)` builder method sets the working directory of the container process.
> The `path` must be an absolute path.
> If not set, the working directory is determined by the container image.

> l[container.user]
> The `container.user(spec: string)` builder method sets the user, and optionally the group, that the container process runs as.
> The `spec` is passed to the container runtime, which accepts a numeric id, a `uid:gid` pair, or a name defined in the image.
> It must be non-empty and contain no whitespace.
> If not set, the user is determined by the container image.
>
> Setting this is how a script runs a workload as a non-root user without depending on the image's entrypoint to drop privileges itself, which [actuate.container.hardening](rule:r/actuate.container.hardening) prevents some tools from doing.

> l[container.stop-signal]
> The `container.stop_signal(name: string)` builder method declares the POSIX signal sent to the container's main process when the runtime stops the container.
> Accepted forms are the canonical `"SIGFOO"` (e.g. `"SIGINT"`, `"SIGQUIT"`, `"SIGTERM"`) and the bare `"FOO"` shorthand; unknown signal names must be rejected at script-evaluation time.
> When unset, the default signal applied by the runtime is `SIGTERM`.
> Use cases: stateful workloads whose semantics require a specific signal — for example, PostgreSQL maps `SIGINT` to "fast shutdown" and `SIGTERM` to "smart shutdown" (which waits for clients to disconnect and may exceed the stop deadline).

> l[container.stop-timeout]
> The `container.stop_timeout(seconds: int)` builder method sets how long the runtime waits between sending the stop signal and sending `SIGKILL`.
> `seconds` must be a positive integer.
> When unset, the runtime applies its default (currently 90 seconds, matching systemd's `TimeoutStopSec` default).
