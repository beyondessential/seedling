---
id: DEP
---

# Deployments

Applies to the BSL `Deployment` resource, a long-lived container workload.

## Terminology

### Deployment

A [resource](../overview.md#resource) that keeps a number of instances of one container workload running, and replaces them when its definition changes.

### Grace window

The time after an instance of a Deployment with a health check starts, `start_period + retries × interval` seconds, before the runtime acts on the instance being unhealthy.

## Declaration

A script declares a Deployment with `app.deployment(name: string)`, which returns a `Deployment` [builder](overview.md#builders).
A `Deployment` implements the [Pod](pod.md) interface, and so the [Container](container.md) interface.

## Scale

A script sets a Deployment's scale with `deployment.scale(fixed: number)` or `deployment.scale(bounds: range)`.
A Deployment on which `scale` is never called has a fixed scale of 1.

`scale(fixed)` is equivalent to `scale(fixed..fixed)`.
A `fixed` below 1 or above 10 MUST throw.

`scale(bounds)` takes a range written `lower..upper`, and reads both `lower` and `upper` as inclusive bounds.
A `lower` below 0 or above 10 MUST throw.
An `upper` below 1 or above 65535 MUST throw.
A `lower` greater than `upper` MUST throw.

> [!NOTE]
> `scale(1..4)` allows one to four instances, although a Rhai range written `1..4` excludes 4.

While a Deployment runs, the runtime MUST keep at least `lower` and at most `upper` of its instances running.
An operator or the runtime MAY choose an instance count within the bounds.
Until one does, the instance count MUST be `lower`.

## Update and termination strategies

`deployment.on_update(strategy: OnUpdate)` sets the strategy the runtime follows when a change to the Deployment's definition is applied.
The default is [`OnUpdate.Rolling`](constants.md#onupdate).

`deployment.on_terminate(strategy: OnTerminate)` sets the strategy the runtime follows when a Deployment's container terminates.
The default is [`OnTerminate.Recreate`](constants.md#onterminate).

## Health checks

`deployment.healthcheck(config: map)` declares a periodic health check of each of the Deployment's instances.
The method is defined on `Deployment` alone.

| field | type | required | default | meaning |
| --- | --- | --- | --- | --- |
| `kind` | string | yes | | the variety of check; `"command"` |
| `cmd` | string or array of strings | with `kind: "command"` | | the probe command |
| `interval` | integer seconds | no | 30 | the time between successive checks |
| `timeout` | integer seconds | no | 30 | the time a single check may run before it counts as failed |
| `retries` | integer | no | 3 | the consecutive failures that make a healthy instance unhealthy |
| `start_period` | integer seconds | no | 0 | the time after the container starts during which failures do not count against `retries` |
| `on_failure` | string | no | `"replace"` | the response to an instance that stays unhealthy; `"replace"` or `"monitor"` |

A `config` with any key not in the table MUST throw.
`interval`, `timeout` and `start_period` MUST be non-negative, and a negative value MUST throw.
A `retries` below 1 MUST throw.
An `on_failure` other than `"replace"` or `"monitor"` MUST throw.

A `kind` of `"http"`, `"tcp"` or `"grpc"` MUST throw with an error naming the kind as unsupported.
Any other `kind` except `"command"` MUST throw.

A Deployment on which `healthcheck` is never called has no health check.
The runtime MUST treat a running instance with no health check as healthy.

### Command checks

With `kind: "command"`, the runtime MUST run `cmd` inside the container as the probe.
A string `cmd` MUST be run by the container's shell.
An array `cmd` MUST be run directly, its first element naming the executable.
A probe passes when it exits with status zero, and fails otherwise.

An empty string, an empty array, and an array whose every element is empty MUST each throw.

### Check timing

The runtime MUST run a check every `interval` seconds, and MUST count a check still running after `timeout` seconds as failed.
After `retries` consecutive failures, the runtime MUST treat the instance as unhealthy.
Failures within `start_period` seconds of the container starting MUST NOT count towards `retries`.

### Failure response

`on_failure` sets the runtime's response to an instance that is still unhealthy once its [grace window](#grace-window) has elapsed.

With `"replace"`, the runtime MUST replace the instance, as [health check replacement](../runtime/autonomous.md#health-check-replacement) specifies.

With `"monitor"`, the runtime MUST NOT replace the instance.
The instance's health still governs its place in the [routing pool](../runtime/lifecycle.md#routing-pool), and its recovery is left to an operator.

`on_failure` does not affect whether an instance is [Ready](../runtime/lifecycle.md#containers).

## Priority

`deployment.priority(level: Priority)` declares the Deployment's [priority](constants.md#priority), its claim on host resources relative to other workloads when the host is under pressure.
The default is `Priority.Normal`.
The method is defined on `Deployment` alone, and every workload that is not a Deployment MUST run at `Priority.Normal`.

How the runtime realises priority is specified by [PRIO](../runtime/priority.md).
