---
id: IDN
---

# Resource identity

Applies to how the [runtime](../overview.md#runtime) identifies resource instances, and to the names it reserves for itself in namespaces [operators](../overview.md#operator) also use.

## Borrowed terms

| term | meaning |
| --- | --- |
| UUID | A 128-bit universally unique identifier, [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562). |
| nil UUID | The UUID with all 128 bits set to zero, [RFC 9562 §5.9](https://www.rfc-editor.org/rfc/rfc9562#section-5.9). |

## Terminology

### Instance ID

A 128-bit identifier that the runtime assigns to one resource instance and never changes.

### Display name

The human-readable name of a resource instance, used where the instance appears on the host and to operators.

## Instance identity

The runtime MUST assign each resource instance an identity that stays the same across iterations of the [reconciliation loop](reconciliation.md#reconciliation-loop) and across runtime restarts.
An instance's identity consists of:

| member | meaning |
| --- | --- |
| instance ID | assigned when the instance is created and never changed |
| app | the name of the [app](../overview.md#app) the instance belongs to |
| resource type | the kind of [resource](../overview.md#resource) the instance realises |
| resource name | the resource's name, absent for an anonymous resource |
| variant | singleton, or one of a scaled group |

The runtime MUST derive an instance's display name when it creates the instance, and MUST store it with the identity.
A stored display name MUST NOT change.
Where a display name carries a suffix derived from the instance ID, the suffix MUST be the first eight hexadecimal digits of the instance ID.

The runtime MUST match an instance by its recorded identity, never by the shape of its display name or of any name derived from it.

## Scaled instances

A scaled resource, such as a Deployment with `scale(N)`, has N instances.
Each instance MUST have its own instance ID, chosen at random, so that identifiers imply no order or position.

## Anonymous instances

An anonymous resource MUST receive an identity that is stable for the resource's lifetime and cannot equal the identity of a named resource.

## Job instances

The instance ID of a Job instance depends on where the Job is declared:

| Job | instance ID |
| --- | --- |
| declared at the top level of a definition (static) | the nil UUID, so a static Job has at most one instance and its display name ends in `00000000` |
| declared inside an action closure (dynamic) | the name-based UUID, version 5 per [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562), with the [lifecycle operation](../overview.md#lifecycle-operation)'s ID as namespace and `job:<name>` as name |
| the target of a shell `attach()` | a random UUID chosen when `attach()` is called |

> [!NOTE]
> A dynamic Job instance therefore keeps its identity within one lifecycle operation, including across replay passes, while separate operations, and separate shell sessions, get distinct instances of the same Job.

## Reserved names

The runtime grants itself these names in namespaces operators also use:

| namespace | reserved | held for |
| --- | --- | --- |
| site ingress names | `tailscale` | the site ingress of the [Tailscale discovery provider](site-resources.md#tailscale-discovery) |
| app names | `infra` | the [resource-control group](priority.md#resource-control-groups) of the runtime's infrastructure |
| app names | `seedling` | faults about the runtime's own reporting, as [FLT](faults.md#fault-records) specifies |
| external volume mapping names | every name beginning `seedling-op-` | [operation-scoped volume bindings](operations.md#operation-scoped-volumes) |

The runtime MUST reject every operation that would bring a site volume, site ingress, external volume mapping or app into being under a reserved name, including each operation that [site volume names](site-resources.md#site-volume-names) lists.
The runtime MUST NOT reject an update, rename or deletion of an object because its name is reserved.

A component that deletes, disables or replaces objects it considers its own MUST identify them by the ownership the runtime recorded, whatever they are named.
A discovery provider MUST NOT disable or replace a manual site ingress.
