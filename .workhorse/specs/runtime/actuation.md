---
id: ACTU
---

# Actuation

Applies to how the [runtime](../overview.md#runtime) realises resource instances on the [host](../overview.md#host): workload containers, their logs, volume storage and held volumes.

## Terminology

### Data directory

The host directory, named by the daemon's `--data-dir` option, in which the runtime keeps its persistent state and the storage of named non-tmpfs volumes.

### Tmpfs location

The host directory, on a tmpfs filesystem, in which the runtime keeps the storage of tmpfs volumes.

### Workload container

A container the runtime runs for a Deployment or Job instance.

### Infrastructure container

A container the runtime runs for itself rather than for an [app](../overview.md#app): the proxy or the resolver.

### Breadcrumb

A log entry the runtime writes to record one of its own actions on an app.

### Held volume

The preserved data of a volume the runtime would otherwise have deleted, kept until an operator confirms its deletion or restores it.

## Deployment and Job instances

When starting a Deployment or Job instance, the runtime MUST ensure that the instance's pod network exists, ensure that the container image is present on the host, and start the container under process supervision.
The runtime MUST pull the image if it is not present, as [IMG](images.md#image-pulls) specifies.

Before the container starts, the runtime MUST ensure that each anonymous volume the container mounts exists, and MUST apply that volume's declared file writes to it.

When stopping a Deployment or Job instance, the runtime MUST stop the supervised container process.
Once the process has stopped, the runtime MUST remove the instance's pod network and every anonymous volume created for the instance.

## Container hardening

The runtime MUST start every workload container with the settings below.
A script MAY change a setting only through the definition method the table names.

| setting | default | script adjustment |
| --- | --- | --- |
| Linux capabilities | all dropped | [`cap_add`](../language/container.md#privileges) adds a named capability back |
| privilege escalation | disabled | none |
| root filesystem | the default [`writable_rootfs`](../language/container.md#privileges) gives | `writable_rootfs` makes it writable, and no tmpfs is mounted at `/tmp` |
| process limit | the default [`pids_limit`](../language/container.md#resource-limits) gives | `pids_limit` sets another limit |
| open file limit | 65536, soft and hard | none |

A container whose definition [names a user](../language/container.md#privileges) MUST be started as that user.

> [!NOTE]
> With privilege escalation disabled, a process that starts as root and drops to another user from inside the container cannot be relied on to work.
> A container that needs to run as a non-root user names that user in its definition.

## Container logs

The runtime MUST direct the standard output and standard error of every workload container and every infrastructure container to the host's system journal.

The runtime MUST tag every journal entry a container produces with the fields below, so that a log query can select an app, a resource, an instance, or an infrastructure component by field without relying on unit names.

| field | carried by | value |
| --- | --- | --- |
| `SEEDLING_APP` | workload containers and breadcrumbs | the app's name |
| `SEEDLING_RESOURCE_KIND` | workload containers and breadcrumbs | the resource's kind, in lower case |
| `SEEDLING_RESOURCE` | workload containers and breadcrumbs | the resource's name |
| `SEEDLING_INSTANCE` | workload containers and breadcrumbs | the instance's display name |
| `SEEDLING_INFRA` | infrastructure containers | `proxy` or `resolver` |
| `SEEDLING_RT_CALL` | breadcrumbs | the name of the `rt.*` method recorded without its `rt.` prefix, such as `start` or `warm_images`; `sub_action` for an action invocation; `unit_create`; or `replay` |
| `SEEDLING_SCRIPT_POS` | breadcrumbs | the script position of the call, as `<file>:<line>:<column>` |

A breadcrumb carries `SEEDLING_RESOURCE_KIND`, `SEEDLING_RESOURCE` and `SEEDLING_INSTANCE` only where its action names a single resource or instance.
It carries `SEEDLING_SCRIPT_POS` only where the script exposed the call's position.

## Breadcrumbs

The runtime MUST write a breadcrumb to the system journal, tagged as above, for each `rt.*` call an action closure makes, each action it invokes, each process supervision unit it creates, and each replay pass of a [lifecycle operation](../overview.md#lifecycle-operation).
A log query at any granularity MUST return breadcrumbs and container output together, in time order.

The runtime MUST write a call's breadcrumb when the call first executes, and MUST NOT write it again when the call is replayed.
The runtime MUST write one `replay` breadcrumb at the start of each replay pass.

## Volume storage

The runtime MUST store each named non-tmpfs volume as a directory in the data directory and bind-mount it into the containers that use it.
When the data directory is on a BTRFS filesystem, the runtime MUST create each named non-tmpfs volume as a BTRFS subvolume.
The container runtime MAY manage anonymous non-tmpfs volumes.

The runtime MUST store each tmpfs volume, named or anonymous, as a directory in the [tmpfs location](#tmpfs-location), and bind-mount it into the containers that use it.
That directory MUST be the volume for every lifecycle step: starting creates and seeds it, observation reports it present or missing, and stopping removes it.
The runtime MUST NOT stand a container-runtime volume in for a tmpfs volume.

> [!NOTE]
> A container runtime's own tmpfs driver creates a fresh tmpfs at every mount, so writes applied to the volume's host directory never reach the container.
> A lifecycle step acting on a stand-in leaves the directory holding the data untouched, and a later volume of the same name inherits its files.

## Volume creation and seeding

When starting a Volume instance, the runtime MUST create the volume's storage if it does not exist, then apply the volume's declared file writes to it.

Creating a named non-tmpfs volume's storage and applying its declared writes MUST be one step.
The part of the runtime that creates the storage MUST apply the declared writes.
A part of the runtime that finds the storage already present MUST NOT apply them.

> [!NOTE]
> An app owns its volume's contents once it has them, so seeding again would overwrite data the app wrote.
> A volume created without being seeded is never seeded afterwards, so a workload that happened to create the volume before the component responsible for it would start against empty storage.

Several parts of the runtime MAY find the storage absent at the same moment.
Exactly one of them MUST create it, and creation MUST report whether this caller created it.
A caller whose creation did not take effect MUST treat the storage as already present, and MUST NOT report the name being in use as an error.

If a declared write fails, the runtime MUST remove storage the same step created, so that the next attempt creates and seeds it from scratch.

The runtime MUST apply a tmpfs volume's declared writes each time it creates the volume's directory, including after the host restarts.
A tmpfs volume's directory and contents MUST survive a restart of the runtime while the host stays up.

## Exported volumes

A listing of [exported volumes](../language/volume.md#export) MUST report, for each volume, the host path at which its contents are stored.

## Volume removal

When stopping a Volume instance, the runtime MUST remove the volume.
For a tmpfs volume, the runtime MUST delete its directory.
For a named non-tmpfs volume, the runtime MUST hold its data as a [held volume](#held-volume), and MUST NOT delete it.

## Held volumes

A named non-tmpfs volume of an app is removed, and its data held as [volume removal](#volume-removal) requires, when the volume's name is removed from the app's definition, when the app is uninstalled, and when the volume's storage cannot be changed in place to the [backend of the data directory](#volume-storage).
If the app still declares a volume under the same name, the runtime MUST create a fresh volume and keep the held one beside it.

The runtime MUST hold a managed [site volume](site-resources.md#site-volumes)'s data when an operator deletes it.
Deleting a snapshot site volume deletes its storage, as [SITE](site-resources.md#site-volumes) specifies.

A held volume MUST record the context it came from: an app volume its app, and a site volume the app name `_site`.

If the volume has no storage, holding it MUST succeed without recording a held volume, and the removal that required the hold MUST proceed.
The runtime MUST commit the held volume's record before it moves the volume's data.
If the move then fails, the runtime MUST remove the record.
If the volume has storage the runtime cannot hold, because the runtime does not own it or because moving it failed, the hold MUST fail and the removal MUST NOT proceed.

The runtime MUST NOT delete a held volume's data until an operator confirms its deletion.

An operator MAY restore a held volume into a new managed site volume instead of confirming its deletion.
The operator MAY name the new site volume.
If the operator names none, the runtime MUST use the held volume's recorded name.
The runtime MUST reject a restore whose target name is [taken](site-resources.md#site-volume-names).
The restored volume MUST be recorded as a managed site volume, whichever context the held volume came from.
The runtime MUST remove the held volume's record once its data has been moved.

The runtime MUST emit an [event](../interface/events.md) when it creates a held volume, when an operator confirms a held volume's deletion, and when an operator restores one.
A restore event MUST identify the held volume consumed and the name of the site volume it became.

## Storage confinement

Every host path the runtime creates, writes, moves or deletes on behalf of a definition or an operator command MUST be confined to the root that owns it.
Content written into a volume is confined to that volume's root.
A volume's own storage is confined to the directory holding volumes of its kind: in the data directory for a non-tmpfs volume, and in the tmpfs location for a tmpfs volume.

Confinement MUST cover every component of a path.
The runtime MUST resolve a parent directory it creates on the way to a write under the same confinement as the write.
The runtime MUST refuse a path that escapes its root, and MUST create nothing along the way to it.

The runtime MUST create each file a [declared file write](../language/volume.md#declared-file-writes) writes with permissions no broader than `0640`.

Storage whose own path is a symbolic link is not owned by the runtime.
The runtime MUST refuse to create, hold or delete such a path, with an error naming it.
The runtime MUST NOT act on the target of such a link, and MUST NOT report a deletion that removed only the link.

## BTRFS check

When it starts, the runtime MUST determine whether the data directory is on a BTRFS filesystem.
If it is not and the daemon was started without the `--without-btrfs` flag, the runtime MUST exit with an error that names the `--without-btrfs` flag.
If the filesystem type cannot be determined, the runtime MUST exit with an error.
