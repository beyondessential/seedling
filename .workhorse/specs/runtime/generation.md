---
id: GEN
---

# Generations

## Generation

<!-- from runtime: Generation -->

> r[generation.definition]
> A *generation* is a monotonically increasing per-app counter that uniquely identifies the application's defined state at a point in time.
> Each generation corresponds to a specific `(definition bundle, parameter values)` pair.
> The current generation of an app determines the AppDef that the reconciler maintains in steady state.

> r[generation.bumps]
> The generation of an app must be incremented (by exactly one) on each of:
>
> - The initial registration of the app.
> - A successful definition update, together with the parameter change made alongside it, if any.
> - A successful parameter set or unset.
>
> The bump must be committed to durable storage atomically with the change that caused it.
> If the change triggers a lifecycle operation (such as an `on_change` handler), the operation is dispatched after the generation has been committed.

> r[generation.monotonic]
> Generations must be strictly monotonic for the lifetime of an app.
> A failed lifecycle operation (for example, a failing `on_change` handler) must not roll back the generation: the change to the defined state has already been committed, and the failure is recorded as the operation's [outcome](rule:r/generation.history).
> An operator may file a corrective change as a new generation; the failed generation remains visible in history.

> r[generation.history]
> The runtime must maintain a durable per-app history of generations.
> Each generation history entry must contain:
>
> - The generation number.
> - A timestamp of when the generation was created.
> - The kind of change: `Register`, `ScriptUpdate`, `ParamSet`, or `ParamUnset`.
> - For `ParamSet` and `ParamUnset`: the parameter name, the previous value (`Option<Value>`), and the new value (`Option<Value>`). A `ParamSet` from an unset state has a previous value of `None`; a `ParamUnset` has a new value of `None`.
> - For `Register` and `ScriptUpdate`: the content hash of the definition bundle installed at this generation, and that definition's [provenance](rule:i/definition.provenance).
> - For a `ScriptUpdate` that changed a parameter alongside its definition: the parameter name, the previous value, and the new value, as for `ParamSet` and `ParamUnset`.
> - The identity of the lifecycle operation triggered by this change, if any.
> - The outcome of that operation: `Pending` (still running or queued), `Succeeded`, or `Failed` (with details).

> r[generation.script-storage]
> Definition bundles must be stored content-addressed by their [content hash](rule:i/definition.content-hash).
> Multiple generations whose bundles are identical share a single stored bundle.
> A generation history entry references its bundle by hash; the bundle is retrieved by looking up the hash.

> r[generation.reconstruction]
> Given a generation number N for an app, the runtime must be able to reconstruct the AppDef as it was at generation N by:
>
> 1. Looking up the bundle hash of the most recent `Register` or `ScriptUpdate` entry at or before N and loading the corresponding bundle.
> 2. Building the parameter map at generation N: for each parameter that has any change recorded at or before N, the value is taken from the most recent such change, whether a `ParamSet`, a `ParamUnset`, or a `ScriptUpdate` that changed it alongside its definition. An unset (or absence of any change) yields `None`.
> 3. Evaluating the bundle's script with that parameter map.

> r[generation.previous]
> The *previous generation* of generation N is generation N − 1.
> Reconstruction of the previous generation is required to materialise the `old` argument of [`on_change`](rule:l/param.on-change.old) handlers.

> r[generation.deregister]
> When an app is deregistered, its generation history and all stored generation data (parameter values, bundles referenced only by this app) must be deleted as part of teardown.
> A subsequent registration of an app with the same name begins a new generation lineage from generation 1; the two registrations are independent for all runtime purposes.
>
> Forensic reconstruction of a prior app's history (across a deregister/re-register cycle) is the responsibility of the [audit log](rule:r/audit.log), not the per-app generation history.
