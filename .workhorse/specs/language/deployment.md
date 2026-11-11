---
id: DEP
---

# Deployments

## Deployment

<!-- from language: Deployment -->

> l[deployment.type]
> A Deployment is a long-lived instance of a container workload. It describes how to run a single container image and associated configuration, and will manage updates to the underlying resource (the running container) from its declarative configuration.
>
> Deployments are defined using the `app.deployment(name: string)` method, which returns a [builder](rule:l/bsl.builder).

> l[deployment.pod]
> Deployment implements the [Pod](rule:l/pod.interface) interface.

> l[deployment.scale]
> The `deployment.scale(fixed: number)` or `deployment.scale(scalable: range)` builder method defines the scale ("replicas" in Kubernetes terms) of a Deployment.
>
> A fixed-scale Deployment will try to always keep that amount of container copies alive as long as the Deployment is running, not more or less. It is equivalent to `scale(fixed..fixed)`.
> The `fixed` number must be a positive non-zero integer.
>
> A scalable Deployment is defined from a lower and upper bound (represented as a range of positive integers). The Deployment will try to keep at least the lower bound and at most the upper bound of containers running, and operators or the Seedling control plane may modify the scale of the Deployment within the defined range. The lower bound may be zero. The upper bound must be non-zero.
>
> If a Deployment has a lower bound scale of zero it will be scheduled with zero containers initially.

> l[deployment.scale.max-lower-bound]
> The lower bound of a scale definition must not exceed 10. If a fixed scale or the lower bound of a range exceeds 10, the method must throw.

> l[deployment.on-update]
> The `deployment.on_update(strategy: OnUpdate)` builder method defines the strategy used when an update is applied to a Deployment.
> The default is [`OnUpdate.Rolling`](rule:l/const.on-update.rolling).

> l[deployment.on-terminate]
> The `deployment.on_terminate(strategy: OnTerminate)` builder method defines the strategy used when the controlled container terminates within a Deployment.
> The default is [`OnTerminate.Recreate`](rule:l/const.on-terminate.recreate).

> l[deployment.healthcheck]
> The `deployment.healthcheck(config: map)` builder method declares a periodic health check for the Deployment's container.
> The `config` map must contain a `kind` key identifying the check variety, plus zero or more common timing fields and zero or more kind-specific fields.
>
> Healthchecks are only valid on Deployments. The method is not registered on [Jobs](rule:l/job.type), so calling it on a Job is a BSL evaluation error.
>
> Common fields (all optional):
>
> - `interval`: seconds between successive checks. Default 30.
> - `timeout`: seconds a single check may run before being considered a failure. Default 30.
> - `retries`: consecutive failures required to transition from healthy to unhealthy. Default 3.
> - `start_period`: seconds of grace after the container starts during which failures do not count against `retries`. Default 0.
> - `on_failure`: response to sustained unhealth, one of `"replace"` or `"monitor"`. Default `"replace"`.
>
> All timing fields must be non-negative. `retries` must be a positive integer.
>
> If `healthcheck` is not called, the Deployment has no declared health check. A running container with no declared check is treated as healthy.

> l[deployment.healthcheck.kind]
> The `kind` field is required.
>
> In the current spec, the only accepted value is `"command"`. The values `"http"`, `"tcp"`, and `"grpc"` are reserved for future use and must be rejected by the current implementation with an error identifying the kind as unsupported.
>
> Any other value must cause the method to throw.

> l[deployment.healthcheck.command]
> When `kind` is `"command"`, the `config.cmd` field is required and defines the probe command run inside the container. The command is considered passing when it exits with code zero, and failing otherwise.
>
> `cmd` may be:
>
> - A string: run through a shell (equivalent to `["CMD-SHELL", cmd]`).
> - A string array: run directly, the first element being the executable.
>
> An empty `cmd` must cause the method to throw.

> l[deployment.healthcheck.timings]
> The timing fields control how often the probe runs and how quickly a failing probe transitions the container from healthy to unhealthy:
>
> - `interval`: seconds between successive checks. Default 30.
> - `timeout`: seconds a single check may run before being considered a failure. Default 30.
> - `retries`: consecutive failures required to transition from healthy to unhealthy. Default 3.
> - `start_period`: seconds of grace after the container starts during which failures do not count against `retries`. Default 0.
>
> All timing values must be non-negative; `retries` must be a positive integer. The grace window before a starting container is treated as unhealthy is `start_period + retries × interval`.

> l[deployment.healthcheck.on-failure]
> The `on_failure` response determines how the runtime reacts to a Deployment instance that has been unhealthy long enough to exceed the grace window:
>
> - `"replace"` (default): the runtime spawns a replacement instance alongside the unhealthy one and lets the unhealthy instance keep serving traffic until the replacement is healthy. When the replacement is healthy, traffic shifts to it and the unhealthy instance is retired. If the replacement also fails to become healthy, the runtime stops the cycle, leaves the original running (degraded), and files a hard fault per [fault.healthcheck-replace-failed](rule:r/fault.healthcheck-replace-failed). See [autonomous.healthcheck-replace](rule:r/autonomous.healthcheck-replace).
> - `"monitor"`: no automatic replacement. The container is observed and routing decisions account for its health (see [lifecycle.service](rule:r/lifecycle.service)), but the runtime does not spawn replacements. Recovery is operator-driven.
>
> `on_failure` does not affect whether the container is considered Ready — an unhealthy container is not Ready regardless of the policy.

> l[deployment.priority]
> The `deployment.priority(level: Priority)` builder method declares the Deployment's [priority](rule:l/const.priority.enum): its claim on host resources relative to other workloads when the host is under memory, CPU, or I/O pressure.
> The default is [`Priority.Normal`](rule:l/const.priority.enum).
>
> Priority is declared on Deployments only. The method is not registered on [Jobs](rule:l/job.type), so calling it on a Job is a BSL evaluation error, and every workload that is not a Deployment runs at `Normal`.
>
> The level expresses intent; how it is realised is a runtime concern. The order in which workloads are shed under memory pressure is defined by [priority.kill-order](rule:r/priority.kill-order), and how contended CPU and I/O are divided by [priority.scheduling](rule:r/priority.scheduling). A Deployment's priority composes with the operator-set [app priority](rule:r/priority.app) of the app it belongs to: comparisons are app-major, so a Deployment in a higher-priority app outranks one in a lower-priority app whatever either Deployment's own level.
