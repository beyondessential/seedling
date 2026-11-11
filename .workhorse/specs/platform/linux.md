---
id: LINUX
---

# Linux platform

Applies to a [host](../overview.md#host) running Linux, settling for that platform each requirement the other specs leave to the platform.

The other specs hold on every platform.
Where they name a platform facility in general terms, or leave a choice to the platform, this spec says what it is on Linux.

## Borrowed terms

| term | meaning |
| --- | --- |
| systemd | The Linux service manager, [systemd](https://systemd.io/). |
| unit | An object systemd manages, such as a service or a slice, [systemd.unit(5)](https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html). |
| transient unit | A unit created at runtime through systemd's API rather than from a unit file, [systemd.unit(5)](https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html). |
| slice | A unit that groups processes into a node of the control-group tree, [systemd.slice(5)](https://www.freedesktop.org/software/systemd/man/latest/systemd.slice.html). |
| control group | A kernel grouping of processes for resource control, [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html). |
| journal | systemd's structured log store, [systemd-journald.service(8)](https://www.freedesktop.org/software/systemd/man/latest/systemd-journald.service.html). |
| OOM score adjustment | A per-process bias, from -1000 to 1000, on the kernel's choice of victim when memory runs out, [proc(5)](https://man7.org/linux/man-pages/man5/proc.5.html). |
| systemd-resolved | systemd's local resolver, [systemd-resolved.service(8)](https://www.freedesktop.org/software/systemd/man/latest/systemd-resolved.service.html). |
| XDG base directories | The per-user directory layout of the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/). |

## Host facilities

| facility named in the specs | on Linux |
| --- | --- |
| process supervisor | systemd, reached over its D-Bus API |
| container runtime | Podman, reached over its API socket at `/run/podman/podman.sock` |
| system journal, host log store | the journal |
| resource-control group | a slice, over cgroup v2 |
| system resolver | systemd-resolved |

The host MUST run systemd as its service manager, with the unified cgroup v2 hierarchy.
Unless the resolver is given its upstreams, the host MUST run systemd-resolved with its extra stub listener on `127.0.0.54`, which is the resolver's [default upstream](../runtime/resolver.md).

## Supervised units

The runtime MUST run each container instance, workload or infrastructure, as a transient service unit.
A workload container instance's unit MUST be named `seedling-<display name>.service`, where the display name is the instance's [display name](../runtime/identity.md).
Every unit the runtime creates MUST have a name beginning `seedling-`.

The runtime MUST create each transient unit so that systemd discards it once it is inactive or failed with no restart pending.

### Exit status

A run's exit status, as [termination](../language/rt.md#termination) and the [restart record](../runtime/autonomous.md#restart-record) use it, is the exit status systemd records for the unit's main process.
A run the supervisor reports as succeeded is one whose unit result is `success`.

### Stopping

The runtime MUST set the unit's kill signal to the container's [stop signal](../language/container.md#stopping), where it declares one.
The runtime MUST set the unit's stop timeout to the container's stop timeout, where it declares one.
A container that declares no stop timeout MUST take systemd's default stop timeout, which is 90 seconds unless the host configures another.

### Restart pacing

The runtime MUST configure each unit's automatic restarts, as [restart pacing](../runtime/autonomous.md#restart-pacing) requires, with these settings:

| setting | value |
| --- | --- |
| delay between restarts | 5 seconds |
| start-limit interval | 600 seconds |
| start-limit burst | 10 starts |

The supervisor has given up restarting an instance when systemd reports the unit's result as `start-limit-hit`.

## Resource-control groups

The runtime MUST realise [resource-control groups](../runtime/priority.md#resource-control-groups) as slices nested under one root slice, `seedling.slice`, which systemd places under its root slice.

| slice | holds | CPU and I/O weight |
| --- | --- | --- |
| `seedling-infra.slice` | the infrastructure containers | 500 |
| `seedling-<app>.slice` | an app's workloads | 400 for a `high` app, 100 for `normal`, 25 for `low` |
| `seedling-<app>-<priority>.slice` | an app's workloads of one [Deployment priority](../language/constants.md#priority), within the app's slice | 400 for `critical`, 200 for `elevated`, 100 for `normal`, 25 for `low` |

In a slice name, `<app>` is the app's name with each `-` replaced by `_`, and `<priority>` is the Deployment priority in lower case.

> [!NOTE]
> systemd reads a `-` in a slice name as a step down the hierarchy, so an app named `a-b` would otherwise land inside the slice of an app named `a`.

The runtime MUST write each slice it creates as a unit file in `/etc/systemd/system`, so that its weights survive a restart of systemd.
The runtime MUST remove only slice unit files whose names have the forms in the table.

## Kill order

The runtime MUST realise the [kill order](../runtime/priority.md#kill-order) through the OOM score adjustment of each unit's processes.

An app workload's adjustment MUST be the sum of an app term and a Deployment term:

| app priority | app term |
| --- | --- |
| `high` | -500 |
| `normal` | 0 |
| `low` | 500 |

| Deployment priority | Deployment term |
| --- | --- |
| `critical` | -200 |
| `elevated` | -100 |
| `normal` | 0 |
| `low` | 200 |

An infrastructure container's adjustment MUST be -900.

The adjustment is fixed when a unit's processes start, so a change of priority reaches a running workload when it next restarts, as [priority setting](../runtime/priority.md#priority-setting) allows.

> [!NOTE]
> No workload takes -1000, which would exempt it from the kernel's choice altogether, so that a host out of memory always has a victim.

## Logs

The runtime MUST send the standard output and standard error of every container to the journal, through its supervised unit.
The runtime MUST attach the [journal fields](../runtime/actuation.md#container-logs) of a container to every entry its unit writes, as fields of that unit's log.
The runtime MUST read [log streams](../interface/logs.md) from the journal, selecting entries by those fields.

## Forwarding

The runtime MUST ensure that the host's IPv6 and IPv4 forwarding, as [NAT64](../runtime/nat64.md#translator) requires, are enabled through the `net.ipv6.conf.all.forwarding` and `net.ipv4.conf.all.forwarding` kernel settings.

## Egress

The default routes the runtime reads, for [IPv6 egress](../runtime/nat64.md#ipv6-egress) and for [`HOST_HAS_IPV4`](../language/constants.md#host-constants), MUST be those of the main routing table.
A globally routable IPv6 address MUST be one within `2000::/3`.

## Idle command

On Linux, [`IDLE_CMD`](../language/constants.md#idle-command) MUST be `["sleep", "infinity"]`.
A container using it needs an image whose `sleep` accepts `infinity`.

## Per-user state

A client's state directory, in which the [client key](../interface/keys.md) and the web interface's client key are kept by default, MUST be `$XDG_STATE_HOME`, or `~/.local/state` where it is unset, as the XDG base directories define it.
