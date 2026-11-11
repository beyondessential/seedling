---
id: CTL
---

# App control

## Scaling

<!-- from interface: Scaling -->

> i[scale.set]
> `/apps/scale { app, deployment, scale }` sets the running scale of a single Deployment within an installed app.
> `scale` is a non-negative integer. The value is clamped to the deployment's declared bounds; requests that would move outside the bounds succeed but stay at the boundary.
> The app must be registered and the named deployment must exist in the current AppDef; otherwise `not_found` is returned.
> On success, the response contains `scale` (the new scale value) and `bounds` with `low` and `high`.

> i[scale.decision-persistence]
> The effective scale chosen by `/apps/scale` is stored durably and survives process restarts.
> On startup, the stored decision is loaded and used as the effective scale for the deployment.

> i[scale.reset-on-uninstall]
> When an app is uninstalled, all stored scaling decisions for that app are discarded.
> After reinstallation, each Deployment's effective scale reverts to its declared lower bound.

> i[scale.describe]
> `/apps/show` includes, for each Deployment resource, a `scale` object with fields `low` (lower bound), `high` (upper bound), and `current` (the effective scale).
> `current` is the stored scaling decision clamped to the declared bounds, or the lower bound if no decision has been stored.
> Where the decision cannot be read, `current` is null, per [scaling.decision.unreadable](rule:r/scaling.decision.unreadable); `low` and `high` come from the app definition and are still reported.
> A null must not be read as a scale: a client adjusting a deployment by one instance has nothing to adjust from, and must say the figure is unavailable rather than counting from zero or from a bound.

## App Priority

<!-- from interface: App Priority -->

> i[app.priority.set]
> `/apps/priority { app, priority }` sets the [app priority](rule:r/priority.app) of an installed app.
> `priority` is one of `high`, `normal`, or `low`; any other value returns `requirements_invalid` and leaves the stored priority untouched.
> The app must be registered; otherwise `not_found` is returned.
> The change takes effect on the app's running workloads without a restart or redeploy, and is stored durably, per [priority.settings](rule:r/priority.settings).
> On success, the response contains `priority` (the new value).

> i[app.priority.reset-on-uninstall]
> When an app is uninstalled, its stored app priority is discarded.
> After reinstallation the app priority reverts to `normal`.

> i[app.priority.describe]
> `/apps/show` includes a top-level `priority` field carrying the app's current app priority, and `/apps/list` includes a `priority` field on each app summary.
> `/apps/show` includes, for each Deployment resource, a `priority` field carrying the [priority](rule:l/deployment.priority) declared for it in the current AppDef, defaulting to `normal` where the definition declares none.

> i[app.canopy.describe]
> `/apps/show` includes a top-level `canopy_type` field carrying the [Canopy application type](rule:l/app.canopy-type) the app's definition declares, or `null` when it declares none.

## Deployment Restart

<!-- from interface: Deployment Restart -->

> i[deployment.restart]
> `/apps/restart { app, deployment }` triggers a restart of all running instances of the named Deployment within the installed app, following its configured update strategy (`on_update`: rolling or replace).
> The app must be registered and the named deployment must exist in the current AppDef; otherwise `not_found` is returned.
> A restart does not change the deployment's definition or generation. Running instances are replaced with fresh containers carrying the same configuration.
> The restart is durable: if the process restarts before the reconciler finishes, the replacement will resume on the next startup.
> On success, the response is `{ operation_id }` where `operation_id` is a unique string identifying this restart operation.

## Resource Stop / Unstop

<!-- from interface: Resource Stop / Unstop -->

> i[resource.stop]
> `/apps/resource/stop { app, kind, name }` turns off a named resource within an installed app without changing its declared configuration.
> `kind` must be one of `deployment`, `job`, or `ingress`; any other kind returns `requirements_invalid`.
> The app must be registered and the named resource must exist in the current AppDef; otherwise `not_found` is returned.
> Stopping a deployment scales its running instances to zero without modifying the declared scale bounds; unstopping later restores the declared effective scale.
> Stopping a job or ingress marks it as unscheduled without removing its definition.
> A stopped state is durable and survives process restarts.
> On success, the response is `{}`.

> i[resource.unstop]
> `/apps/resource/unstop { app, kind, name }` reverses a previous stop for the named resource.
> If the resource was not stopped, this is a no-op and returns `{}`.
> On success, the response is `{}`.

> i[resource.unstop-all]
> `/apps/unstop { app }` unstops all resources for the named app in one call.
> If no resources are stopped, this is a no-op and returns `{}`.
> On success, the response is `{}`.

> i[resource.stop.no-active-op]
> All three stop/unstop endpoints (`/apps/resource/stop`, `/apps/resource/unstop`, `/apps/unstop`) reject with `operation_in_progress` when a lifecycle operation is active or queued for the target app, or a change to it is being applied.
> The operator cancels an active operation first via [action.cancel](rule:i/action.cancel), or waits for a queued one or a change to finish; desired-state mutations must not race with a running action closure.

> i[resource.stop.status]
> `/apps/show` includes a `stopped` boolean field on each resource that has been stopped via `/apps/resource/stop`.
> The top-level response includes a `stopped_resources` array of `{ kind, name }` objects listing every resource currently stopped for the app.
> `/apps/list` includes a `has_stopped_resources` boolean field on each app summary indicating whether any resources are currently stopped.
