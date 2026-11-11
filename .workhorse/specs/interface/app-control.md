---
id: CTL
---

# App control

Applies to the methods that adjust how a registered app runs without changing its definition: deployment scale, app priority, deployment restarts, and stopping resources.

## Scaling

`/apps/scale` sets the scale of one deployment of an app.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `deployment` | string | yes | the deployment's name |
| `scale` | integer | yes | the number of instances wanted, from 0 to 65535 |

The server MUST refuse the request with `not_found` if the app is not registered or its current definition declares no deployment of that name.
The server MUST clamp `scale` to the deployment's declared scale bounds, and a request outside them succeeds at the nearer bound.

The result is `{ "scale": <integer>, "bounds": { "low": <integer>, "high": <integer> } }`, giving the scale stored and the deployment's declared bounds.

The runtime MUST store the scaling decision durably, and MUST use it as the deployment's effective scale after the daemon restarts.

Uninstalling an app MUST discard every scaling decision stored for it.
A deployment with no stored decision has its declared lower bound as its effective scale.

[`/apps/show`](apps.md#resources) reports, for each deployment, a `scale` object with these members.

| member | type | meaning |
| --- | --- | --- |
| `low` | integer | the declared lower bound |
| `high` | integer | the declared upper bound |
| `current` | integer or null | the effective scale: the stored decision clamped to the declared bounds, or the lower bound where no decision is stored |

If the stored decision cannot be read, `current` MUST be `null`, as [unreadable scaling decisions](../runtime/autonomous.md#unreadable-scale) requires, and `low` and `high` are still reported.
A client MUST NOT take a `null` `current` as a scale.
A client adjusting a deployment by a step MUST report the scale as unavailable rather than count from zero or from a bound.

## App priority

`/apps/priority` sets an app's [app priority](../runtime/priority.md#priority-setting).

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `priority` | string | yes | `high`, `normal` or `low` |

The server MUST refuse any other `priority` with `requirements_invalid`, leaving the stored priority as it was.
The server MUST refuse the request with `not_found` if the app is not registered.
The result is `{ "priority": <string> }`, giving the priority stored.

The runtime MUST store the app priority durably, and MUST apply it to the app's running workloads without restarting or redeploying them, as [priority settings](../runtime/priority.md#priority-setting) specifies.

Uninstalling an app MUST discard its stored app priority, and an app with none stored has the priority `normal`.

`/apps/show` and each entry of `/apps/list` report the app's priority in a `priority` member.
`/apps/show` reports, for each deployment, a `priority` member giving the [deployment priority](../language/deployment.md#priority) its definition declares, or `normal` where it declares none.
If the stored app priority cannot be read, the server MUST refuse `/apps/show` and `/apps/list` with `internal`, and MUST NOT report `normal` in its place.

## Deployment restart

`/apps/restart` replaces every running instance of one deployment with a fresh instance of the same configuration.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `deployment` | string | yes | the deployment's name |

The server MUST refuse the request with `not_found` if the app is not registered or its current definition declares no deployment of that name.

The runtime MUST replace the instances following the deployment's [update strategy](../runtime/update-strategies.md).
A restart MUST NOT change the app's definition or generation.

The runtime MUST record the restart durably before answering, so that a restart the daemon had not finished when it stopped is resumed when it starts again.

The result is `{ "operation_id": <string> }`, where `operation_id` identifies this restart and is never reused.

## Resource stop

`/apps/resource/stop` turns off one resource of an app without changing its declared configuration.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `kind` | string | yes | `deployment`, `job` or `ingress` |
| `name` | string | yes | the resource's name |

The server MUST refuse any other `kind` with `requirements_invalid`.
The server MUST refuse the request with `not_found` if the app is not registered or its current definition declares no resource of that kind and name.

Stopping a deployment MUST bring its running instances to zero without changing its declared bounds or its stored scaling decision.
Stopping a job or an ingress MUST unschedule it without removing its declaration.
A stopped resource's desired [lifecycle state](../overview.md#lifecycle-state) is `Unscheduled`.

The runtime MUST store the stop durably.
The result is `{}`.

`/apps/resource/unstop` takes the same parameters and reverses a stop of the resource they name.
The server MUST refuse a `kind` that names no resource kind with `requirements_invalid`, and MUST refuse the request with `not_found` if the app is not registered.
Unstopping a resource that is not stopped MUST change nothing.
Unstopping a deployment restores its effective scale.
The result is `{}`.

`/apps/unstop` takes an `app` parameter and reverses every stop of that app's resources.
The server MUST refuse it with `not_found` if the app is not registered.
The result is `{}`.

The server MUST refuse all three methods with `operation_in_progress` while a lifecycle operation is running or queued for the app, or a change to the app is being applied.

> [!NOTE]
> An operator cancels a running operation with [`/apps/action/cancel`](actions.md#cancellation), or waits for a queued one or a change to finish.
> A change to the desired state made while an action closure drives the same resources would race it.

Uninstalling an app MUST discard every stop recorded for it.

`/apps/show` reports a `stopped` member on each resource, true when the resource is stopped, and a top-level `stopped_resources` array of `{ "kind": <string>, "name": <string> }` naming every stopped resource.
Each entry of `/apps/list` reports a `has_stopped_resources` member, true when any of the app's resources is stopped.
