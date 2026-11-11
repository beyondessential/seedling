---
id: UPD
---

# Update strategies

Applies to how the [runtime](../overview.md#runtime) replaces running Deployment and Job instances whose configuration no longer matches the app's [resource graph](../overview.md#resource-graph).

## Terminology

### Spec hash

A digest of a container's full configuration, recorded on the container when it starts.

### Stale instance

A running instance whose recorded spec hash differs from the spec hash its current definition yields, or which carries no spec hash.

## Stale instances

On each iteration of the [reconciliation loop](reconciliation.md#reconciliation-loop), the runtime MUST compare the observed spec hash of each running Deployment and Job instance with the spec hash derived from its current definition.

The runtime MUST derive the progress of an update from the observed spec hashes on each iteration.
After a runtime restart, the runtime MUST observe the running containers, find the remaining stale instances, and continue the update from where it stopped.

After the host restarts with no containers surviving, every instance is missing rather than stale, and the runtime MUST start each with its current definition.

## Rolling updates

When a Deployment's [update strategy](../language/deployment.md#update-and-termination-strategies) is rolling and it has stale instances, the runtime MUST replace them one at a time:

1. Raise the Deployment's effective instance count by one above its current scale.
2. Start an instance with the current definition in the added slot.
3. Wait until that instance is running and healthy.
4. Stop one stale instance.
5. Repeat from step 2 until no stale instance remains.
6. Return the effective instance count to the Deployment's scale.

An instance without a [healthcheck](../language/deployment.md#health-checks) is healthy once it is running.

The added instance MUST be part of the [desired state](../overview.md#desired-state), so that scaling does not treat it as excess.

During a rolling update, the number of healthy instances MUST NOT fall below the Deployment's lower scale bound, except while every instance is stale and no replacement is yet healthy.

## Replace updates

When a Deployment's update strategy is replace and it has stale instances, the runtime MUST stop every stale instance before starting any replacement.
A replacement MUST NOT start until every stale instance has fully stopped.
A replace update MAY take the Deployment below its lower scale bound.

## Jobs

A Job takes no part in rolling or replace updates.
The runtime MUST stop a stale Job instance and start it again with its current definition.
