---
id: ACTU
---

# Actuation

## Actuation

<!-- from runtime: Actuation -->

> r[actuate.deployment.start]
> Starting a Deployment or Job instance must ensure the pod network exists for the instance, ensure the container image is present (pulling if absent), and start the container under process supervision.

> r[actuate.deployment.stop]
> Stopping a Deployment or Job instance must stop the supervised container process and, once stopped, remove the pod network and any anonymous volumes that were created for that instance.

> r[actuate.deployment.anon-volume.start]
> When starting a Deployment or Job instance, for each anonymous volume mounted by the container, the runtime must ensure the volume exists and apply any declared file writes to it before the container starts.

> r[actuate.container.hardening]
> Workload containers must be started with all Linux capabilities dropped, privilege escalation disabled, and a read-only root filesystem with a writable tmpfs at `/tmp`. A default PID limit of 256 and a file-descriptor limit of 65536 are applied. BSL configuration may adjust these defaults.

> r[actuate.container.user]
> A container whose definition names a user must be started as that user. Privilege escalation is disabled for every workload container, so a container that needs to run as a non-root user must be started as one rather than starting as root and dropping privileges from inside; tools that drop privileges themselves cannot be relied on to work under that restriction.

> r[actuate.container.journal-metadata]
> Workload containers must have their stdout and stderr directed to the system journal.
> Each supervised container process must be tagged with structured journal fields that
> identify the owning app, resource kind, resource name, and instance. These fields
> must be present on every journal entry produced by the container so that log queries
> can filter at any granularity (app, resource, or individual instance) without
> relying on unit name conventions.

> r[actuate.infra.journal-metadata]
> Infrastructure containers (proxy, resolver) must have their stdout and stderr directed
> to the system journal. Each infrastructure container must be tagged with a structured
> journal field that identifies the infrastructure component so that log queries can
> target infrastructure logs independently of workload logs.

> r[actuate.breadcrumb]
> The runtime must record its own action breadcrumbs into the same log sink that
> carries container output, tagged with the same app, resource kind, resource, and
> instance fields. A breadcrumb names the `rt.*` primitive it records — or a synthetic
> kind for runtime events such as unit creation and replay boundaries — and, where the
> script surfaced one, the call site.
>
> Sharing the sink and the tagging scheme is the requirement, not an implementation
> convenience: a log query at any granularity must return breadcrumbs and container
> output interleaved in time order, so that an operator reading an app's logs sees the
> closure's call sequence against the output it produced.

> r[actuate.breadcrumb.replay]
> A breadcrumb is emitted on a call's first fresh execution and not on replays of that
> call, so a barrier-suspended operation does not flood the log with each pass. Each
> replay pass instead surfaces a single boundary breadcrumb.

> r[actuate.ingress.plaintext]
> Whether an ingress terminates TLS is a property of its own termination, independent of whether it terminates HTTP.
> An ingress may terminate HTTP as plaintext, as a site-ingress attachment does when its parent's TLS provisioning mode is `none`.
>
> An ingress that does not terminate TLS must be served as plaintext on its port: the proxy must listen for HTTP on that port and route matching requests to the ingress's target.
> The runtime must not request or provision a certificate for such an ingress's hostname, must not create a QUIC listener for it, and must not answer its requests with a redirect unless the ingress itself declares one.
> Forwarding rules for an ingress must cover only the transport protocols it serves, so a plaintext HTTP ingress must not cause UDP traffic on its port to be forwarded.
>
> A hostname may carry both plaintext and TLS-terminating ingresses on different ports.
> Each must be served from the listener matching its own termination, and the presence of a TLS-terminating ingress must not cause a plaintext ingress on the same hostname to be dropped or served only over TLS.

> r[actuate.ingress.port-scope]
> An ingress is served on its own port and no other. A request is routed to the target of an ingress only when it arrives on the port that ingress was declared on, whatever hostname it names.
>
> Two ingresses for the same hostname on different ports are therefore independent: each port serves the routes of the ingress declared on it, and neither displaces the other, whether they terminate TLS or not.
> A redirect an ingress declares answers on the port it names and on no other.

> r[actuate.ingress.warm-certs]
> When an action closure invokes [`rt.warm_certs`](rule:l/rt.warm-certs) with a selection that contains TLS-terminating ingresses, the runtime must initiate certificate acquisition for those ingresses' hostnames without exposing the ingresses to live traffic.
> A typical implementation pushes a partial proxy configuration that requests certificate acquisition while not routing requests to any backend; once the certificate is `valid`, it is served from the proxy's cache when the same ingress is later started for real.
>
> The warm_certs call must be recorded in the [action execution log](rule:r/history.action-log) and must be idempotent on replay: a subsequent invocation with the same selection observes the existing cert state and returns immediately when `valid`, without re-initiating acquisition.

> r[volume.export.host-path]
> A listing of exported volumes must report, for each one, the path where that volume's contents live on the host. An exported volume exists so that something other than its owning app can use it, and a co-located consumer needs a path to address rather than having to reconstruct the runtime's storage layout for itself.

> r[actuate.volume.start]
> Starting a Volume instance must create the named volume if it does not already exist, then apply any declared file writes to the volume.
>
> Creating a named non-tmpfs volume and seeding it with its declared writes is one step. Whichever part of the runtime brings the volume's storage into existence must apply the declared writes to it, and a part that finds the storage already there must not apply them again — an app owns its volume's contents once it has them, so re-seeding would overwrite what the app wrote. A volume that was created without being seeded is never seeded afterwards, so the two must not be separable: a workload that happened to materialise the volume ahead of the component nominally responsible for it would otherwise start against empty storage, permanently and with nothing reported.
>
> Because those parts run concurrently, both may find the storage absent at the same moment, and exactly one of them creates it. Creation must report which one that was. The part whose creation did not take effect is a part that found the storage already there: it owes the volume no writes, and the name already being in use is not an error for it to report. Treating creation as something that can only succeed makes an ordinary overlap fail the start on storage backends whose create refuses an existing name.
>
> When a declared write fails part-way through seeding, storage the same operation had just created must be removed again, so the next attempt seeds from scratch rather than leaving behind a volume that counts as created and will never receive its writes.

> r[actuate.volume.tmpfs]
> Tmpfs-backed Volumes (named or anonymous) must be materialised as a host-managed directory under a tmpfs-backed location and bind-mounted into containers. The runtime must not delegate tmpfs storage to the container runtime's own tmpfs volume driver, because such drivers create a fresh tmpfs at every container mount and so silently drop any declared file writes that were applied to the volume's host backing.
> Because tmpfs contents do not survive a host reboot, any declared file writes must be re-applied unconditionally each time the volume is materialised — not only when the volume is first created.
> That directory is the volume for every step of its lifecycle: it is what starting creates and seeds, what observation reports as present or missing, and what stopping removes. The runtime must not create a container-runtime volume to stand for a tmpfs Volume, because a lifecycle step that acts on the stand-in leaves the directory holding the data untouched — a stop that reports a cleanup it did not perform, and a later volume of the same name that silently inherits the previous one's files.

> r[actuate.volume.storage]
> Named non-tmpfs volumes are stored as host-managed directories under the data directory and bind-mounted into containers. Anonymous non-tmpfs volumes remain managed by the container runtime.

> r[actuate.volume.btrfs]
> When the data directory resides on a BTRFS filesystem, named non-tmpfs volumes must be created as BTRFS subvolumes.

> r[storage.confinement]
> Every host filesystem path the runtime creates, writes, moves, or deletes on behalf of a definition or an operator command is confined to the root that owns it: a volume's root for content written into that volume, and for a volume's own storage the directory that holds volumes of its kind — under the data directory for a non-tmpfs volume, under the tmpfs-backed location for a tmpfs one. Where a volume's storage lives does not change whether the runtime owns it.
>
> Confinement covers every component of a path, not only its last. A parent directory created on the way to a write must be resolved under the same confinement as the write itself, so that a symlink placed inside the root cannot lead the runtime to create or write anything outside it. A path that escapes its root must be refused, and nothing along the way to it may be created.
>
> Storage whose own path is a symlink is not storage the runtime owns. Creating, holding, or deleting such a path must be refused with an error naming it, rather than acting on what it points at or reporting a deletion that removed only the link.

> r[actuate.volume.hold]
> When a named non-tmpfs volume or a managed site volume would be removed — either because the volume name has been removed from the app definition, because the volume's storage backend cannot be updated in-place, or because an operator has requested deletion of a managed site volume — the runtime must preserve the volume's data in a held state instead of deleting it. The held volume remains linked to the originating context: app volumes to their app, and site volumes to the site (reported as app name `_site`). If the app still requires a volume under the same name, a fresh volume is created and the old one is held alongside it. Snapshot site volumes are excluded from the hold mechanism (see [volume.site.snapshot.delete](rule:r/volume.site.snapshot.delete)).
>
> A volume with no storage behind it is held by doing nothing: there is no data to preserve, so the removal it was standing in the way of goes ahead and no held volume is recorded. This must be distinguishable from a hold that could not be carried out — storage the runtime does not own, or a move that failed — which still stops the removal, because there the data is real and would be orphaned. A runtime that cannot tell the two apart leaves a managed site volume whose storage an operator removed by hand permanently undeletable, its record stranded in every listing.

> r[actuate.volume.hold.confirm]
> An operator must explicitly confirm deletion of a held volume before its data is removed.

> r[actuate.volume.hold.restore]
> An operator may restore a held volume's data into a fresh managed [site volume](rule:r/volume.site) instead of confirming its deletion. Restoration moves the held data into place under an operator-chosen site volume name, defaulting to the held volume's recorded name; the resulting volume is recorded as a managed site volume regardless of whether the hold originated from an app volume or a site volume. Restoration must be rejected when a site volume with the target name already exists, so operators surface and resolve the collision instead of silently overwriting either record. The held volume's tracking record is removed once its data has been moved.

> r[actuate.volume.hold.events]
> Creation, confirmed deletion, and restoration of held volumes must be observable on the event feed so that UIs can refresh held-volume counts without polling. A restore event must identify the held volume that was consumed and the name of the new site volume it became.

> r[actuate.volume.stop]
> Stopping a Volume instance must remove the named volume.

## Startup

<!-- from runtime: Startup -->

> r[startup.btrfs]
> At startup, the runtime must verify that the data directory resides on a BTRFS filesystem. If BTRFS is not available and the `--without-btrfs` flag has not been passed, the runtime must exit with an error that mentions the `--without-btrfs` flag.
