---
id: SEC
---

# Secret storage

Applies to how the [runtime](../overview.md#runtime) protects secret values at rest, holds the seeds that [derived random values](../language/random.md) come from, and withholds secret parameter values and derived values from operators.

## Terminology

### Secret parameter

A parameter whose effective [`secret` flag](../language/parameter.md#secret-parameters) is `true` in the app's current [resource graph](../overview.md#resource-graph).

### App seed

The secret from which [derivation helpers](../language/random.md#derivation-helper) called in the static context of an [app](../overview.md#app)'s definition derive their values, held for as long as the app is registered.

### Operation seed

The secret from which derivation helpers called in the action closures of one [lifecycle operation](operations.md#operation) or one [shell session](../interface/shells.md#shell-session) derive their values.

### Derived value

A value returned by a derivation helper, from either seed.

## Secret key

The runtime MUST keep a secret key for protecting secret values at rest, in the file `seedling.db.key` in its [data directory](actuation.md#data-directory), beside the database.
The key file MUST carry the same [file permissions](infrastructure.md#file-permissions) as the database.
If no key file exists when the runtime starts and the database holds no value protected under the secret key, the runtime MUST create one holding a freshly generated key.
If no key file exists while the database holds a value protected under the secret key, the runtime MUST NOT start, and MUST report an error naming the missing key file.

> [!NOTE]
> A fresh key cannot decrypt the values already stored, so creating one would leave every protected value unreadable without saying so.

## Storage

The runtime MUST store the values of secret parameters apart from the values of other parameters.
It MUST protect them at rest with authenticated encryption under the secret key.

A script reading a parameter's [value](../language/parameter.md#values) MUST receive the value in plain form whether or not the parameter is secret.

## Seeds

A seed MUST be 32 bytes drawn from a cryptographically secure random number generator.
The runtime MUST protect a persisted seed at rest as it protects [stored secret parameters](#storage).
The runtime MUST NOT give a seed to a script, return it to a caller of the [Operator Interface](../overview.md#operator-interface), write it to its logs, or emit it in an [event](../interface/events.md).

### App seeds

The runtime MUST create an app's seed when it registers the app, and MUST persist it durably in the same commit as the registration.
Every registered app MUST have exactly one app seed.
The runtime MUST discard the app seed when it [deregisters](app-teardown.md#deregistration) the app.

If the runtime cannot read an app's seed, every evaluation of the app's definition that calls a derivation helper in the static context MUST fail.
The runtime MUST NOT create a replacement seed for an app that has one it cannot read.

> [!NOTE]
> An app registered again under the same name is a different app, with a different seed, and an app moved to another host is registered there afresh.
> Values derived from the old seed and written into volumes or external stores no longer match what the definition derives.
> Restoring a host from its database and secret key keeps every app's seed.

### App seed rotation

An operator MAY replace an app's seed with a fresh one, as [seed rotation](../interface/app-control.md#seed-rotation) specifies.
The runtime MUST persist the new seed durably, discarding the old one, before it evaluates the app's definition again.
After a rotation, the runtime MUST evaluate the app's definition against the new seed and apply the resulting resource graph, without changing the app's [generation](../overview.md#generation).

> [!NOTE]
> Every value derived from the app seed changes, so every workload whose configuration holds one restarts as a [stale instance](update-strategies.md#stale-instances).
> Data stores keep whatever values were written into them.

### Operation seeds

The runtime MUST create a lifecycle operation's seed when it first persists the operation, and MUST persist it with the operation's parameters, as [persistence](operations.md#persistence) specifies.
Every pass and every [replay](suspension.md#replay) of the operation, including those after a restart, MUST derive from the same seed.
An [invoked action](../language/action.md#action-invocation) runs within its caller's operation, and MUST derive from that operation's seed.
The runtime MUST discard the seed when it clears the persisted operation.

The runtime MUST create a shell session's seed when the session opens, and MUST discard it when the session ends.
The runtime MUST NOT persist a shell session's seed.

## Generation history

An entry in the [generation history](generation.md#history-entries) recording a previous or new value of a parameter MUST protect each value under the secret key according to how that value was held:

- the value being replaced MUST be protected if it was read from secret storage;
- the value replacing it MUST be protected if it is written to secret storage.

The event announcing the change MUST withhold a value under the same rule.

> [!NOTE]
> A definition that drops a parameter's `secret` flag in the same update that changes its value still protects the value it replaced.

The runtime MUST decrypt protected history values when it reconstructs a past generation.

## Redaction

The runtime MUST NOT return the value of a secret parameter to a caller of the [Operator Interface](../overview.md#operator-interface).
The runtime MUST NOT write a secret parameter's value to its logs, or emit it in an [event](../interface/events.md).
It MUST NOT return a value recorded in the generation history if the parameter is secret now, or if the value was protected when it was recorded.

The runtime MUST withhold every derived value under the same rules as a secret parameter's value, wherever it would withhold one.
A Blob derived value MUST be withheld in each text encoding of it that BSL offers a script.

A response withholding a value MUST say so with a machine-readable marker, so that a client can tell an unset value from a withheld one, as [describing an app](../interface/apps.md#parameters) and the [generation history](../interface/apps.md#generation-history) define.

## Migration

When a parameter becomes secret, the runtime MUST move its stored value from non-secret storage to secret storage without operator action.
When a parameter stops being secret, the runtime MUST move its stored value from secret storage to non-secret storage without operator action.
The runtime MUST make either move after every evaluation of the app's definition it applies, whatever caused the evaluation.

The runtime MUST write the value to its new storage before removing it from the old.
A generation history entry recorded while the parameter was secret MUST stay protected, and MUST stay [withheld](#redaction) from callers, after the parameter stops being secret.
