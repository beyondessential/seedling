---
id: SITE
---

# Site resources

## Actuation

<!-- from runtime: Actuation -->

> r[volume.site]
> A site volume is a named volume managed by operators, independent of any app. Site volumes come in two kinds:
>
> - **Managed**: a host directory (or BTRFS subvolume) created and maintained by the runtime under the data directory.
> - **Bind**: an arbitrary host path provided by the operator, mounted as-is.
>
> Site volumes may be read-write or read-only. Tmpfs site volumes are not supported.

> r[volume.site.lifecycle]
> Site volumes are created and deleted exclusively through operator commands. The runtime must create the backing storage for managed site volumes at creation time. When a managed site volume is deleted, its backing storage must be routed through the [held volume](rule:r/actuate.volume.hold) mechanism so that an operator must explicitly confirm final removal. Deleting a snapshot site volume removes its backing storage directly without routing through the hold mechanism (see [volume.site.snapshot.delete](rule:r/volume.site.snapshot.delete)). Deleting a bind site volume must only drop the runtime's reference and must not affect the operator-provided host path.

> r[volume.site.name-taken]
> Every operation that brings a new site volume into being — creating one, snapshotting to one, promoting a snapshot into one, or [restoring a held volume](rule:r/actuate.volume.hold.restore) as one — must be rejected as an invalid request when the chosen name is already taken, before any storage is touched. A name is taken when a site volume is recorded under it, and also when storage already stands at the name with no record of it.
>
> The check comes first because the storage operations do not refuse a name that is in use on their own terms: a snapshot taken onto an existing directory is placed *inside* it and reported as success, leaving a subvolume nested in an unrelated live volume with no record pointing at it, which only then fails on the record's unique name. Storage with no record is refused rather than taken over, because the runtime cannot tell leftovers from an operator's volume whose record has not been written.
>
> The refusal holds between operators as well as within one request. Operator commands are served concurrently, so finding the name free and then taking it must be one indivisible step; otherwise two requests naming the same target both find it free and both go on to make storage, which is the nesting above with nothing to have stopped it. Only one of the two may proceed, and the other must be refused as though it had arrived second.

> r[volume.site.lifecycle.events]
> Site volume creation and deletion must emit events on the event feed and be recorded in the [audit log](rule:r/audit.log). Deletion events must identify the site volume's kind and, when the deletion routed through the held-volume mechanism, the resulting held volume identifier.

> r[volume.site.snapshot]
> A snapshot site volume is a read-only point-in-time snapshot of a named volume (app volume or managed site volume) that supports snapshotting. Only BTRFS-backed volumes support snapshotting. Snapshot site volumes carry metadata identifying their source. They are inherently read-only: even when mapped without the read-only flag, mounts of snapshot site volumes are always read-only.

> r[volume.site.snapshot.events]
> Creating a snapshot site volume must emit an event identifying the new snapshot and the source volume (app-scoped or site-scoped).

> r[volume.site.snapshot.delete]
> Deletion of a snapshot site volume removes the backing storage directly and does not route through the [held volume](rule:r/actuate.volume.hold) mechanism. Snapshots are read-only by construction (which prevents the rename used by the hold path) and the source volume is unaffected by snapshot deletion, so there is no data the operator could meaningfully recover from a held copy of a snapshot. The corresponding [site volume deletion event](rule:r/volume.site.lifecycle.events) must therefore record no held volume identifier for the snapshot kind.

> r[volume.site.promote]
> An operator may promote a snapshot site volume to a fresh managed site volume with an operator-chosen name. The runtime must materialise the new volume as a writable copy seeded from the snapshot's contents and record it as a managed site volume. The source snapshot is not modified and remains available; operators may independently delete it afterwards if they wish. Promotion is only supported on BTRFS-backed installations because it relies on the same snapshotting primitive as `volume.site.snapshot`.

> r[volume.site.promote.events]
> Promoting a snapshot site volume must emit an event identifying the new managed site volume and the source snapshot. The promoted volume is treated as a new managed site volume for audit purposes, distinct from the source snapshot which remains intact.

> r[volume.site.leftover-snapshots]
> Earlier releases ran backups from a read-only snapshot of the source volume, stored as site-volume storage under a name beginning `backup-snap-` and removed when the run finished. A run interrupted between those two steps left the snapshot on disk with no site volume recorded for it. On startup, the runtime must delete any site-volume storage whose name begins `backup-snap-` and for which no site volume is recorded. Storage recorded as a site volume is the operator's and must be left alone, as must storage whose ownership could not be determined.

> r[volume.external.mapping.events]
> Creating, removing, or retargeting an operator-configured external volume mapping must emit an event identifying the app and external volume name, the new mapping target (or absence of one for removal), and — for retargeting — the previous target. These events feed both the event feed and the [audit log](rule:r/audit.log).

> r[service.site]
> A site service is a named service managed by operators, independent of any app. A site service carries a set of one or more endpoints, each a 4-tuple `(service_port, protocol, remote_host, remote_port)`.
>
> `service_port` is the port on which the site service exposes this backend; `remote_host` and `remote_port` are the address traffic is forwarded to. The two ports may differ (e.g. a site service exposes 80/tcp in front of backends listening on 8080). The protocol is one of `tcp`, `udp`, or `http`.
>
> `remote_host` may be an IPv6 literal, an IPv4 literal, or a DNS name. Names are resolved at runtime by the daemon; address-form values are used directly. See [service.site.address](rule:r/service.site.address) for resolution and routing semantics.
>
> The `(service_port, protocol)` pairs across a site service's endpoints define the ports the service exposes. Traffic destined for `(service, service_port, protocol)` is distributed across the `(remote_host, remote_port)` pairs of all endpoints whose `(service_port, protocol)` matches, as for any other multi-backend service (see [service.routing](rule:l/service.routing)). Two endpoints sharing a `(service_port, protocol)` are peers for that port's traffic; endpoints differing in `(service_port, protocol)` back different exposed ports on the same site service.

> r[service.site.address]
> Site service endpoints whose `remote_host` is an IPv6 literal are routed natively using that address. Endpoints whose `remote_host` is an IPv4 literal, or a DNS name resolving only to A records, must be routed through NAT64 using the IANA well-known prefix `64:ff9b::/96` (RFC 6052). Endpoints whose `remote_host` is a DNS name resolving to AAAA records are routed natively when the host has IPv6 egress; on hosts without IPv6 egress, AAAA results are ignored in favour of A results routed through NAT64. When a name resolves to both A and AAAA records and the host has IPv6 egress, the AAAA records are used.
>
> The runtime must report endpoints as unroutable when an IPv4 path is required but NAT64 is unavailable on the host (e.g. when the operator has explicitly disabled it and no external NAT64 is present), and as unresolvable when DNS resolution for an endpoint hostname has failed past a small consecutive-failure threshold. These conditions must surface as faults on the affected site service and must auto-clear when the underlying condition resolves. An unresolvable or unroutable endpoint must not block routing for other endpoints, services, or apps; only the affected site service's own backend pool is reduced.

> r[service.site.address.partial-answer]
> A DNS name is resolved from the address families that answered. When the lookup for one family fails and the other returns records, the endpoint is resolved from those records, the failed family keeps whatever it last answered, and the name is looked up again promptly rather than after the answer's full lifetime. A failed lookup must not be recorded as the family having no records. The name counts towards the unresolvable threshold only when no lookup returned records and at least one failed. The resolver's status report must name the family whose lookup is failing, for as long as it keeps failing, so that a name resolving on one family alone is distinguishable from one resolving on both.

> r[service.site.lifecycle]
> Site services are created, retargeted, and deleted exclusively through operator commands. Creating a site service registers its name and an optional human-readable description; endpoints are added and removed independently so operators may adjust the backing set without recreating the service.
>
> Deleting a site service must be rejected while any [external service mapping](rule:r/service.external.mapping.events) still targets it; operators must unmap or remap those slots first. Deletion removes the service record and its endpoints; no held-resource mechanism applies because site services carry no persistent state.

> r[service.site.lifecycle.events]
> Site service creation and deletion, and endpoint add/remove operations, must emit events on the event feed and be recorded in the [audit log](rule:r/audit.log). Endpoint events identify the site service name and the `(service_port, protocol, remote_host, remote_port)` tuple.

> r[service.external.mapping.events]
> Creating, removing, or retargeting an operator-configured external service mapping must emit an event identifying the app and external service name, the new mapping target (or absence of one for removal), and — for retargeting — the previous target. These events feed both the event feed and the [audit log](rule:r/audit.log).

> r[ingress.site]
> A site ingress is a named entry point managed by operators, independent of any app. A site ingress carries a hostname, an optional human-readable description, a TLS provisioning mode, and a source.
>
> A site ingress has one of two sources:
>
> - **Manual**: created and deleted by the operator.
> - **Discovered**: created and deleted automatically by the daemon based on host configuration. Each discovered site ingress is owned by a *provider* (initially Tailscale; see [ingress.site.tailscale](rule:r/ingress.site.tailscale)) and carries an opaque, stable provider key that survives hostname changes.
>
> The TLS provisioning mode determines how certificates for the hostname are obtained when an attachment requires TLS termination. Valid modes are public-PKI ACME issuance, internal CA, the discovering provider's own certificate facility (only available on discovered ingresses), and "no TLS" (only plaintext attachments are permitted).

> r[ingress.site.lifecycle]
> Manual site ingresses are created, updated, and deleted exclusively through operator commands. Discovered site ingresses are created, updated, and deleted by the daemon's discovery loop; an operator command to delete a discovered site ingress while its source remains active must be rejected. Deletion of a site ingress, regardless of source, cascades to remove all of its attachments.
>
> When a discovery source temporarily disappears (e.g. the underlying provider becomes unreachable) the corresponding site ingress must be marked stale rather than deleted, so that the operator's attachments persist across transient outages and resume serving when the source returns.

> r[ingress.site.lifecycle.events]
> Site ingress creation, update, and deletion (whether by the operator or by a discovery provider) and attachment add, update, and remove operations must emit events on the event feed and be recorded in the [audit log](rule:r/audit.log). Events identify the site ingress name and, for attachment events, the `(port, protocol)` tuple and the attachment target.

> r[ingress.site.attachment]
> A site ingress carries zero or more *attachments*. Each attachment is keyed by `(port, protocol)` within the parent site ingress and binds that key to a target. The target is one of:
>
> - **Forward**: traffic for the attachment's `(port, protocol)` is routed to a specified app service.
> - **Redirect**: requests for the attachment's `(port, protocol)` are answered with an HTTP redirect response to a configured target URL, using a configured response code, optionally preserving the request path.
>
> The set of supported protocols matches that of app-declared ingresses. The hostname and TLS provisioning mode are inherited from the parent site ingress; the attachment chooses only the listening `(port, protocol)` and the target. Multiple attachments differing in `(port, protocol)` may coexist on the same site ingress.
>
> A parent site ingress whose TLS provisioning mode is `none` yields plaintext attachments, served per [actuate.ingress.plaintext](rule:r/actuate.ingress.plaintext).

> r[ingress.site.tailscale]
> The Tailscale discovery provider creates a single discovered site ingress representing the host's Tailscale identity. The site ingress's hostname is the host's tailnet DNS name; its provider key is the host's stable Tailscale node identity. When the operator renames the node, the site ingress's hostname must be updated in place and its existing attachments preserved. When the underlying node identity changes (e.g. the node is re-created on the tailnet), the existing discovered site ingress is removed and a fresh one created.
>
> The Tailscale-provided site ingress's TLS mode resolves to certificates obtained from the local Tailscale facility. The runtime must not attempt to obtain certificates for tailnet hostnames through public ACME.

> r[ingress.site.conflict]
> When an app ingress and a site-ingress attachment both claim the same `(hostname, port)`, the runtime must drop both entries from the proxy configuration on each tick on which the conflict persists, and must file a fault against each side identifying the conflicting party and the `(hostname, port)` tuple. Faults filed for a `(hostname, port)` conflict must be cleared automatically on the first subsequent tick on which that `(hostname, port)` is no longer in conflict. Independent `(hostname, port)` tuples on the same hostname are not affected by a conflict on a different port.
