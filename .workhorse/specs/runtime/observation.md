---
id: OBS
---

# Observation

Applies to how the [runtime](../overview.md#runtime) inspects the [host](../overview.md#host) for the state of each resource instance, and records the result for lifecycle derivation and barriers.

## Observations

On each iteration of the [reconciliation loop](rule:r/reconciliation.loop), the runtime MUST observe each resource instance by inspecting the host, and MUST timestamp each fact it observes.

## Failed observation

The runtime MUST treat an attempt to observe an instance as succeeding in full or failing in full.
The runtime MUST NOT report the partial results of a failed attempt as fact.
A failed attempt yields no facts.

The runtime MUST NOT treat a failed observation as evidence that an instance is absent.
In an iteration where an instance's observation failed, the runtime MUST NOT stop the instance, detect it as terminated, or tear it down on the strength of that observation, and MUST derive the instance's lifecycle state from its last successful observation.

A state that is observed but not recognised is evidence that the thing exists, and the runtime MUST NOT treat it as absence.

When an instance's observation has failed on 3 consecutive iterations, the runtime MUST file a fault of kind `observe_failed` against the instance.
The runtime MUST NOT file it for a single failed observation.
The runtime MUST clear the fault when an observation of the instance next succeeds.

## Containers

For a Deployment or Job instance, the runtime MUST observe whether the instance's network exists, the state of its container, the state of the supervisor unit that runs it, the supervisor's count of restarts of that unit with how its last run ended, and the result of its healthcheck where one is declared.

| container state | meaning |
| --- | --- |
| missing | a successful query found no container |
| created | the container exists and has not started |
| running | the container is running |
| exited | the container has exited and has not been removed |
| present but indeterminate | the container exists in a state that corresponds to no lifecycle transition, such as one still shutting down |

The runtime MUST distinguish a unit the supervisor reports as starting, including one paused before an automatic restart, from one it reports as started, and both from one that has failed or that the supervisor has stopped restarting because it reached its start limit.

## Volumes

For a Volume instance, the runtime MUST observe whether the named volume exists.

If a named volume that is not held in memory exists but its storage does not match the storage the host is configured to use, the runtime MUST report a backend mismatch and MUST NOT report the volume as present.

> [!NOTE]
> A plain directory found where the host is configured for filesystem subvolumes is a backend mismatch, as is the reverse.

## Ingresses

For an Ingress instance, the runtime MUST observe whether the proxy is reachable.

For each hostname of a TLS-terminating ingress, the runtime MUST observe the status of the hostname's certificate.

| status | meaning |
| --- | --- |
| `none` | no certificate has been acquired and no acquisition is in progress |
| `pending` | acquisition is in progress |
| `valid` | an unexpired certificate is held and usable, and its expiry is observable |
| `failed` | acquisition has failed, and the most recent error is observable |

Certificate status determines whether an ingress is [`Ready`](rule:r/lifecycle.ingress) and whether an [`rt.warm_certs`](rule:l/rt.warm-certs) barrier is satisfied.

## Recording

After each observation pass, the runtime MUST record the facts it observed in the [world observation history](rule:r/history.world), from which lifecycle states are derived and barriers decided.
The runtime MUST NOT treat a fact as recorded unless recording it succeeded.
The runtime SHOULD NOT record a fact again for an instance while it is unchanged since it was last recorded.

The runtime MUST record a Service's network as created, and a backend as healthy, from the backends running when the Service's routes are built.
The runtime MUST record an ingress as configured and ready only after a proxy configuration that includes it has been [applied](rule:r/reconciliation.absolute-state) successfully.
For a resource whose desired state is `Unscheduled`, the runtime MUST record that it was sent the stop, and the removal and cleanup of its network or ingress.
