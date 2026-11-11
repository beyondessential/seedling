---
id: CONST
---

# Constants

## Constants

<!-- from language: Constants -->

These are not guaranteed to be constant forever, only for the duration of one script execution.

> l[const.available-threads]
> `AVAILABLE_THREADS` is a positive non-zero number.
> It is the amount of compute threads available to the application.
> It may be thought of as the number of cores available on the node, but the exact value is a concern for the control plane.

> l[const.available-memory]
> `AVAILABLE_MEMORY` is a positive non-zero number.
> It is the amount of memory, in bytes, available to the application.
> It may be thought of as the total RAM on the node, but the exact value is a concern for the control plane.

> l[const.cpu-architecture]
> `CPU_ARCHITECTURE` is a non-empty string identifying the CPU architecture of the node.
>
> Common values include `x86_64`, `aarch64`, `arm`, and `riscv64`, but other values may appear as platforms evolve.
> Scripts should treat unknown values as "unsupported" rather than error out.

> l[const.host-has-ipv4]
> `HOST_HAS_IPV4` is a boolean.
> It is `true` when the node has working IPv4 egress, and `false` otherwise.
>
> "Working egress" means a default IPv4 unicast route is configured. Source address selection is not inspected: an RFC1918 host behind NAT is considered to have egress.

> l[const.host-has-ipv6]
> `HOST_HAS_IPV6` is a boolean.
> It is `true` when the node has working IPv6 egress to the internet, and `false` otherwise.
>
> "Working egress" means both a default IPv6 unicast route and at least one globally-routable IPv6 source address. ULAs and link-local addresses do not qualify.

> l[const.nat64-active]
> `NAT64_ACTIVE` is a boolean.
> It is `true` when the node itself is providing NAT64 translation for its workloads, and `false` otherwise.
>
> A `false` value does not imply that IPv4-only workloads cannot reach IPv4 destinations: an external NAT64+DNS64 infrastructure may still be in play. Scripts that need a definitive answer should combine `HOST_HAS_IPV4` and `NAT64_ACTIVE`.

> l[const.has-snapshots]
> `HAS_SNAPSHOTS` is a boolean.
> It is `true` when the node's volume storage supports copy-on-write snapshots, and `false` otherwise.

> l[const.node-name]
> `NODE_NAME` is a string identifying the node the application runs on.
>
> The format is not specified. It may be empty in contexts where the node identity is not meaningful, such as when validating scripts outside of a running node.

> l[const.timezone]
> `TIMEZONE` is a non-empty string identifying the host's IANA timezone, such as `Pacific/Auckland` or `Etc/UTC`.
>
> When the host's local timezone cannot be determined, the value is `UTC`.

> l[const.idle-cmd]
> `IDLE_CMD` is an array of strings that, when used as a container's `command()`, makes the container start, do nothing, and stay alive until told to stop. It is intended for containers whose only purpose is to host one or more `rt.exec` invocations during an action, where there is no natural long-running process to wait on.
>
> The exact array contents are an implementation detail and may change in future releases.

## OnUpdate

<!-- from language: Constants / OnUpdate -->

`OnUpdate` defines strategies for when [Deployments](rule:l/deployment.type) update.

> l[const.on-update.rolling]
> The `OnUpdate.Rolling` strategy first starts at least one _new_ container, waits until it becomes ready, then stops the same amount of _old_ containers, and repeats until all containers in the Deployment have been rotated to new versions.

> l[const.on-update.replace]
> The `OnUpdate.Replace` strategy stops all _old_ containers, even if that violates the Deployment's [scale lower bound](rule:l/deployment.scale), and only then starts the _new_ versions.

## OnTerminate

<!-- from language: Constants / OnTerminate -->

`OnTerminate` defines strategies for when [Containers](rule:l/container.interface) within [Deployments](rule:l/deployment.type) terminate.

> l[const.on-terminate.recreate]
> The `OnTerminate.Recreate` strategy always recreates the container when it terminates.

This is currently the only value.

## OnExit

<!-- from language: Constants / OnExit -->

`OnExit` defines strategies for when commands within [Containers](rule:l/container.interface) exit.

> l[const.on-exit.restart]
> The `OnExit.Restart` strategy always restarts the container when its command exits.

> l[const.on-exit.terminate]
> The `OnExit.Terminate` strategy always terminates the container when its command exits.

> l[const.on-exit.restart-on-failure]
> The `OnExit.RestartOnFailure` strategy restarts the container when its command exits with a non-zero exit status, and terminates it otherwise.

## ResourceType

<!-- from language: Constants / ResourceType -->

> l[const.resource-type.enum]
> `ResourceType` is an opaque enum type, and in the script scope, is a constant object map of names to opaque values of type `ResourceType`.
>
> - `Parameter`
> - `Service`
> - `Ingress`
> - `Deployment`
> - `Job`
> - `Volume`
> - `ExternalVolume`
> - `ExternalService`
> - `Action`
>
> Actions are invocable handles, not resources: `ResourceType.Action` exists for action-log identity, but resource collections such as `col(app)` and `app.select(...)` do not include actions, so selecting by `ResourceType.Action` yields an empty collection. Action invocation goes through [`Action.invoke`](rule:l/action.call).

## Priority

<!-- from language: Constants / Priority -->

`Priority` describes how important a [Deployment](rule:l/deployment.type)'s workload is relative to other workloads when the host is under resource pressure. It is an expression of intent, not a platform tuning value: the runtime maps each level onto the host's own priority mechanism, and the same definition carries across platforms whose mechanisms differ (see [priority.actuation](rule:r/priority.actuation)).

> l[const.priority.enum]
> `Priority` is an opaque enum type, and in the script scope, is a constant object map of names to opaque values of type `Priority`.
>
> - `Critical` — among the last workloads to be shed when the host runs out of memory, and given the largest share of contended CPU and I/O. For a workload whose loss takes the app down, such as its request-serving API.
> - `Elevated` — favoured over ordinary workloads but yields to `Critical`. For supporting workloads that matter but can survive brief starvation, such as background or sync workers.
> - `Normal` — the default. No preferential treatment.
> - `Low` — the first workloads to be shed under memory pressure, and yields contended CPU and I/O to every higher level. For batch or best-effort work that should give way to everything else.
>
> The levels are totally ordered: `Critical` > `Elevated` > `Normal` > `Low`.
