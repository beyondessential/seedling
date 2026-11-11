---
id: IDN
---

# Resource identity

## Fault Handling

<!-- from runtime: Fault Handling -->

> r[namespace.reserved]
> The runtime grants itself names inside namespaces operators also use: site-volume names beginning `backup-snap-`, and the site-ingress name `tailscale`.
> Creating a site volume or site ingress under a reserved name is rejected; the restriction applies to creation only, so an object that predates the reservation can still be renamed or removed.
> Reservation alone is not sufficient: a component that deletes or disables objects it believes are its own must identify them by what the runtime recorded — a registered site volume is never deleted by the [leftover snapshot sweep](rule:r/volume.site.leftover-snapshots), and a manually created site ingress is never disabled or replaced by a discovery provider — regardless of what the object is named.

## Resource Identity

<!-- from runtime: Resource Identity -->

> r[identity.stable]
> The runtime must assign a stable identity to each resource instance.
> The identity must be consistent across reconciliation ticks and runtime restarts.

> r[identity.components]
> A resource identity consists of:
>
> - An opaque instance ID, assigned once at creation and never changed. For most resource types the ID is randomly generated; see [Job instance identity](rule:r/identity.job) for the exception.
> - The application name.
> - The resource type.
> - The resource name (if not anonymous).
> - Whether the instance is a singleton or one of a scaled group.

> r[identity.scaled]
> Scaled resources (e.g. a Deployment with `scale(N)`) produce N instances.
> Each instance must have a distinct, stable opaque identifier so the runtime can track
> instances independently across restarts and scaling events.
> Instance identifiers must not imply ordering or position; a random value must be used
> so that removing one instance does not make the remaining identifiers appear discontinuous.
> A human-readable display name for each instance is derived at creation time from the
> instance ID and stored stably alongside the identity.

> r[identity.anonymous]
> Anonymous resources (those without a name) must receive a runtime-assigned identity that is stable for the lifetime of the resource but does not conflict with named resources.

> r[identity.job]
> Job instances have a deterministic identity scheme rather than a randomly-generated one, because concurrent executions of the same Job definition must be distinguishable without persisting state across restarts.
>
> - **Static Jobs** (defined in the top-level BSL scope) have a fixed all-zero instance ID. Their display name includes the all-zero suffix (e.g. `myapp-worker-00000000`). There is at most one static instance per Job name; the identity is fully deterministic.
> - **Dynamic Jobs** (defined inside an action closure) have an instance ID derived from the lifecycle operation's `OperationId` and the Job's name via a deterministic UUID v5 derivation: `UUID_v5(operation_id, "job:{name}")`. This gives a stable identity within one operation execution (including across barrier replay passes) while ensuring that distinct operation invocations produce distinct container names, allowing concurrent action runs involving the same Job name to coexist without collision.

> r[identity.job.shell]
> A Job used as the target of a shell `attach()` call receives a fresh randomly-generated instance ID chosen at the moment `attach()` is called, independent of any lifecycle operation ID. This allows multiple concurrent shell sessions to run against the same Job definition without collision.
