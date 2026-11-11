---
id: GEN
---

# Generations

Applies to the numbering and recorded history of each [app](../overview.md#app)'s definition and parameter values.

## Terminology

### Generation history

The durable record of an app's [generations](../overview.md#generation), one entry per generation.

### Previous generation

The generation numbered one less than a given generation of the same app.

## Generation numbers

An app's generation identifies one pair of [definition bundle](../overview.md#definition-bundle) and parameter values.
In [steady state](reconciliation.md#steady-state), the [runtime](../overview.md#runtime) maintains the [resource graph](../overview.md#resource-graph) of the app's current generation, as [desired state](reconciliation.md#desired-state) specifies.

The runtime MUST increment an app's generation by exactly one on each of these changes:

- the app's registration, which creates generation 1;
- a successful definition update, together with the parameter change made alongside it, if any;
- a successful parameter set or unset.

The runtime MUST commit the new generation atomically with the change that caused it.
If the change triggers a [lifecycle operation](../overview.md#lifecycle-operation), such as an `on_change` handler, the runtime MUST dispatch the operation only after committing the generation.

An app's generations MUST be strictly increasing from its registration until its deregistration.
A failed lifecycle operation MUST NOT roll back the generation that triggered it.
The runtime MUST instead record the failure as the operation's outcome in that generation's history entry.

## History entries

The runtime MUST keep a generation history for each app, holding one entry for every generation from the app's registration until its deregistration.

| member | present for | meaning |
| --- | --- | --- |
| generation | every entry | the generation number |
| created | every entry | when the generation was created |
| kind | every entry | `register`, `script_update`, `param_set` or `param_unset` |
| bundle | every entry | the [content hash](../interface/definitions.md#content-hash-computation) of the definition bundle current at this generation |
| provenance | `register` and `script_update` | the [definition provenance](../interface/definitions.md#provenance-reporting) of the definition it installed |
| parameter | `param_set`, `param_unset`, and a `script_update` that changed a parameter alongside its definition | the name of the parameter changed |
| previous value | every entry with a parameter | the parameter's value before the change, or none if it was unset |
| new value | every entry with a parameter | the parameter's value after the change, or none if the change unset it |
| operation | an entry whose change triggered a lifecycle operation | the operation's identity |
| outcome | an entry with an operation | `pending` while the operation runs or waits, then `succeeded`, or `failed` with details |

Values of a secret parameter are protected in history entries as [secret history](secrets.md#generation-history) specifies.

## Bundle storage

The runtime MUST store definition bundles by their content hash, and a history entry MUST reference its bundle by that hash.
Generations whose bundles are identical MUST share one stored bundle.
The runtime MUST NOT use a stored bundle whose contents do not hash to the hash it is stored under.

## Reconstruction

The runtime MUST be able to reconstruct an app's resource graph as it was at any generation N in the app's generation history, by these steps:

1. Load the bundle referenced by the most recent `register` or `script_update` entry at or before N.
2. Build the parameter values at N, taking each parameter's value from the most recent entry at or before N that changed it, whether a `param_set`, a `param_unset`, or a `script_update` that changed it alongside its definition.
   A parameter that was unset by that entry, or that no entry at or before N changed, has no value.
3. Evaluate the bundle with those parameter values.

If any recorded value needed for step 2 cannot be read, reconstruction MUST fail.
The runtime MUST NOT treat an unreadable value as unset.

The runtime MUST reconstruct the previous generation to supply the `old` argument of an [`on_change` handler](../language/parameter.md#change-handlers).

## Lineage

When an app is deregistered, the runtime MUST delete its generation history and every stored bundle that no remaining generation history entry or [template](../interface/templates.md) references.
A later registration of an app under the same name MUST begin a new lineage at generation 1, independent of the earlier app for every runtime purpose.

> [!NOTE]
> The record of an app across a deregistration and a later registration under its name is kept by the [audit log](audit-log.md), not by the generation history.
