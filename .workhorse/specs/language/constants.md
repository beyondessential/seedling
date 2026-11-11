---
id: CONST
---

# Constants

Applies to the constants and enums that BSL places in the scope of every [script](overview.md#script).

## Borrowed terms

| term | meaning |
| --- | --- |
| NAT64 | stateful translation between IPv6 clients and IPv4 servers, as in [RFC 6146](https://www.rfc-editor.org/rfc/rfc6146) |
| DNS64 | synthesis of IPv6 address records for IPv4-only names, as in [RFC 6147](https://www.rfc-editor.org/rfc/rfc6147) |
| IANA time zone name | a zone name from the [IANA Time Zone Database](https://www.iana.org/time-zones) |

## Constant lifetime

The runtime MUST place every constant and enum in this specification in the scope of each evaluation of a script.
A constant's value MUST NOT change during one evaluation.
A later evaluation MAY see a different value.

## Host constants

| constant | type | value |
| --- | --- | --- |
| `AVAILABLE_THREADS` | integer | the number of compute threads available to the app |
| `AVAILABLE_MEMORY` | integer | the amount of memory, in bytes, available to the app |
| `CPU_ARCHITECTURE` | string | the CPU architecture of the [host](../overview.md#host) |
| `HOST_HAS_IPV4` | boolean | whether the host has IPv4 egress |
| `HOST_HAS_IPV6` | boolean | whether the host has IPv6 egress to the internet |
| `NAT64_ACTIVE` | boolean | whether the host itself provides NAT64 for its workloads |
| `HAS_SNAPSHOTS` | boolean | whether the host's volume storage supports copy-on-write snapshots |
| `NODE_NAME` | string | the name of the host the app runs on |
| `TIMEZONE` | string | the IANA time zone name of the host's local time zone |

`AVAILABLE_THREADS` and `AVAILABLE_MEMORY` MUST each be greater than zero.
The runtime determines both values.

> [!NOTE]
> `AVAILABLE_THREADS` approximates the number of cores on the host, and `AVAILABLE_MEMORY` approximates its total memory.

`CPU_ARCHITECTURE` MUST NOT be empty.
Its values include `x86_64`, `aarch64`, `arm` and `riscv64`, and other values MAY appear.
A script SHOULD treat an architecture it does not recognise as unsupported rather than throw.

`HOST_HAS_IPV4` MUST be `true` when the host has a default IPv4 unicast route configured, and `false` otherwise.
The host's choice of IPv4 source address does not affect the value, so a host with a private address behind NAT has IPv4 egress.
If the host's routes cannot be read, whether the host has IPv4 egress is undetermined, and `HOST_HAS_IPV4` MUST be `false` until it is determined.

`HOST_HAS_IPV6` MUST be `true` when the host has [IPv6 egress](../runtime/nat64.md#ipv6-egress), and `false` otherwise, including while IPv6 egress is undetermined.

`NAT64_ACTIVE` MUST be `true` when the host itself provides NAT64 translation for its workloads, and `false` otherwise.

> [!NOTE]
> A `false` value does not mean IPv4-only workloads cannot reach IPv4 destinations, since NAT64 and DNS64 outside the host may still serve them.
> A script needing to know whether they can combines `HOST_HAS_IPV4` with `NAT64_ACTIVE`.

`NODE_NAME` has no defined format.
It MAY be empty where the host's identity has no meaning, such as when a script is validated outside a running daemon.

`TIMEZONE` MUST NOT be empty.
While the runtime cannot determine the host's local time zone, `TIMEZONE` MUST be `UTC`.

### Determining host constants

The runtime MUST determine the host constants when it starts, and MUST NOT change a determined value while it runs.

While whether the host has IPv4 egress is undetermined, the runtime MUST file an [`ipv4_egress_undetermined`](../runtime/faults.md#host-faults) fault.
While the host's local time zone is undetermined, the runtime MUST file a [`timezone_undetermined`](../runtime/faults.md#host-faults) fault.
The runtime MUST retry an undetermined value on each later reconciliation iteration until it is determined.
When an undetermined value of `HOST_HAS_IPV4`, `HOST_HAS_IPV6` or `TIMEZONE` is determined, the runtime MUST re-evaluate the definition of every installed app.

> [!NOTE]
> A change to the host's network or time zone after a value is determined reaches scripts when the runtime next starts.

## Idle command

`IDLE_CMD` is an array of strings which, used as a container's command, starts a process that does nothing and runs until the container is stopped.
The runtime MAY change the array's contents between releases.

> [!NOTE]
> `IDLE_CMD` serves a container that only hosts `rt.exec` invocations during an action, where no long-running process exists to wait on.

## OnUpdate

`OnUpdate` is an enum naming how a [deployment](deployment.md#declaration) is updated.

| variant | strategy |
| --- | --- |
| `Rolling` | the [rolling update](../runtime/update-strategies.md#rolling-updates) |
| `Replace` | the [replacing update](../runtime/update-strategies.md#replace-updates) |

## OnTerminate

`OnTerminate` is an enum naming the runtime's response when a container of a [deployment](deployment.md#declaration) terminates.

Under `OnTerminate.Recreate`, the runtime MUST recreate the container whenever it terminates.

## OnExit

`OnExit` is an enum naming the runtime's response when the command of a [container](container.md#container-interface) exits.

Under `OnExit.Restart`, the runtime MUST restart the container whenever its command exits.
Under `OnExit.Terminate`, the runtime MUST terminate the container whenever its command exits.
Under `OnExit.RestartOnFailure`, the runtime MUST restart the container when its command exits with a non-zero status, and MUST terminate it when its command exits with status zero.

## ResourceType

`ResourceType` is an enum whose variants are `Parameter`, `Service`, `Ingress`, `Deployment`, `Job`, `Volume`, `ExternalVolume`, `ExternalService` and `Action`.

`ResourceType.Parameter` and `ResourceType.Action` name things that are not resources, so a [selection](collection.md#selection) by either MUST yield an empty collection.

> [!NOTE]
> `ResourceType.Action` identifies actions in the action log.
> An action is invoked through [`Action.invoke`](action.md#action-invocation).

## Priority

`Priority` is an enum naming how strongly a [deployment](deployment.md#declaration)'s workloads claim host resources relative to other workloads when the host is under pressure.
Its variants, from strongest claim to weakest, are `Critical`, `Elevated`, `Normal` and `Low`.

The runtime MUST order the variants `Critical` > `Elevated` > `Normal` > `Low`.
`Normal` MUST confer no preferential treatment.
How the runtime realises the order is specified in [PRIO](../runtime/priority.md).

> [!NOTE]
> `Critical` suits a workload whose loss takes the app down, such as its request-serving API.
> `Elevated` suits supporting workloads that matter but survive brief starvation, such as background or sync workers.
> `Low` suits batch or best-effort work that gives way to everything else.
