---
id: PRIO
---

# Resource priority

## Resource Priority

<!-- from runtime: Resource Priority -->

Two levers set how a workload competes for host resources under pressure. Each Deployment carries a [priority](rule:l/deployment.priority) declared in its definition, and each app carries an app priority set by the operator. Together they order every workload on the host, and the runtime realises that ordering as the workloads compete for memory, CPU, and I/O.

> r[priority.app]
> Each installed app has an app priority, one of `high`, `normal`, or `low`, defaulting to `normal`. It is set by the operator (see [app.priority.set](rule:i/app.priority.set)), not declared in the app's definition, so the relative standing of apps on a shared host is an operational decision rather than a claim any one definition can make for itself.
> App priority expresses relative standing only: it orders apps against each other under contention and places no hard ceiling on what an app may consume.

> r[priority.settings]
> The app priority is operator-visible and operator-settable, and a change takes effect without restarting the runtime and without redeploying the app: the runtime re-derives the resource ordering of the app's running workloads and applies what it can to them in place.
> The division of contended CPU and I/O changes for the running workloads as soon as the ordering is re-derived, because it is a property of the group they sit in rather than of the processes themselves.
> A workload's place in the kill order is fixed when its processes start, so a change reaches every workload started after it, and reaches those already running as they are next restarted.
> The setting is stored durably and survives process restarts. When an app is uninstalled its stored app priority is discarded, so a later reinstall starts again at `normal`.

> r[priority.kill-order]
> When the host runs out of memory, the kernel must shed workloads in an app-major order: a workload belonging to a lower-priority app is shed before any workload of a higher-priority app, and within a single app the lower [Deployment priority](rule:l/const.priority.enum) is shed first. A `Critical` Deployment in a `low`-priority app is therefore shed before a `Normal` Deployment in a `high`-priority app.
> The two levers compose as an ordering, not as independent thresholds: it is always the app comparison that decides first, and the Deployment priority only orders workloads that belong to equally-ranked apps.
> The runtime's own infrastructure (proxy, resolver) ranks above every app workload, so a host under memory pressure does not lose the components that route to whatever survives.
> The concrete values used to encode this ordering are an implementation concern; the requirement is the relative order, that a `Critical` workload is protected from the kernel's out-of-memory killer where the platform allows it, and that a `Low` workload is the preferred victim.

> r[priority.scheduling]
> When CPU or I/O is contended, host capacity must be divided first between apps in proportion to their app priority, and then within each app between its Deployments in proportion to their Deployment priority. A higher-priority workload receives a larger share of contended capacity; a workload that is alone on an otherwise idle host is not throttled to its share.
> As with kill order this is a relative division, not a cap: the shares bound how contention is resolved, not how much a workload may use when capacity is free. Per-container hard limits remain the province of [`container.memory`](rule:l/container.memory) and [`container.cpus`](rule:l/container.cpus).

> r[priority.groups-owned]
> The runtime organises its workloads into its own resource-control groups and does not place them in, or depend on, groups defined by the host beyond the platform's standard defaults. On a host whose init system predefines named groups for its own services, the runtime neither expects those groups to exist nor joins them; it creates and manages the grouping it needs.
> The names the runtime gives these groups are names it grants itself in an operator-shared namespace, and so are reserved: a definition may not create a workload whose realised unit would collide with one.

> r[priority.actuation]
> On Linux, the runtime realises the ordering through systemd slices that it owns: each app's workloads sit in a per-app slice, and within it each Deployment priority in a per-tier child slice, so that the slice tree mirrors the app-major, then-tier ordering. The per-app slice carries the CPU and I/O weight derived from the app priority; the per-tier child slice carries the weight derived from the Deployment priority; and each supervised unit carries the out-of-memory kill preference derived from its combined standing.
> The exact slice layout, weights, and kill-preference values are an implementation concern; what must hold is that the realised grouping produces the kill order of [priority.kill-order](rule:r/priority.kill-order) and the capacity division of [priority.scheduling](rule:r/priority.scheduling), using slices the runtime owns per [priority.groups-owned](rule:r/priority.groups-owned) rather than any the host defines.
> A platform without systemd realises the same two orderings through its own mechanism — on Kubernetes, a PriorityClass for scheduling and preemption together with the QoS that governs out-of-memory eviction — so the definition-level priority is portable even where the Linux slice model is not.
