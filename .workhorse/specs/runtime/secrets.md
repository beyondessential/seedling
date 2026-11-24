---
id: SEC
---

# Secret storage

Applies to how the [runtime](../overview.md#runtime) protects secret values at rest, holds the seeds that [derived random values](../language/random.md) come from, and withholds sensitive values from operators.

## Terminology

### Secret parameter

A parameter whose effective [`secret` flag](../language/parameter.md#secret-parameters) is `true` in the app's current [resource graph](../overview.md#resource-graph).

### App seed

The secret from which [derivation helpers](../language/random.md#derivation-helper) called in the static or validator context of an [app](../overview.md#app)'s definition derive their values, held for as long as the app is registered.

### Operation seed

The secret from which derivation helpers called in the action closures of one [lifecycle operation](operations.md#operation) or one [shell session](../interface/shells.md#shell-session) derive their values.

### Derived value

A value returned by a derivation helper, from either seed.

### Withheld value

A secret parameter's value, a derived value, or a helper result the runtime withholds as a derived value, as [redaction](#redaction) specifies.

### Sensitive value

A non-empty value that is the stored value of a secret parameter, the value of an action or shell param whose [`secret`](../language/action.md#param-schemas) is `true`, whether supplied or defaulted, or a derived value.

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

If the runtime cannot read an app's seed, every evaluation of the app's definition that calls a derivation helper in the static or validator context MUST fail.
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

The runtime MUST NOT return a sensitive value to a caller of the [Operator Interface](../overview.md#operator-interface), write one to its logs, or emit one in an [event](../interface/events.md).
It MUST withhold a value that contains a sensitive value wherever it would withhold the sensitive value itself.
It MUST NOT return a value recorded in the generation history if the parameter is secret now, or if the value was protected when it was recorded.

A Blob derived value MUST be withheld in each text encoding of it that BSL offers a script.

The runtime MUST withhold the result of a helper that [digests and encodings](../language/encoding.md) specify as a derived value when any of its arguments contains a withheld value.
An argument contains a withheld value when the string, or the bytes of the Blob, includes that value in any form in which the runtime withholds it.
No argument contains a withheld value that is empty.
A [`File`](../language/bundle.md#bundle-files) or [`Directory`](../language/bundle.md#bundle-files) argument contains no withheld value.
Each result withheld this way MUST count toward the derived values [limit](engine-limits.md#limits).

> [!NOTE]
> An encoding can be reversed outright, and a digest or HMAC of a short secret can be recovered by guessing, so neither result is safer to show than the value it was computed from.

> [!NOTE]
> No minimum length applies, so a short secret value withholds more results than it needs to, which an operator can see, rather than fewer, which an operator could not.
> A bundle's files come from the definition, not from secret storage or a seed.

Redaction applies to what the runtime records and reports itself, such as definition summaries, parameter values, history, events, breadcrumbs, fault descriptions, error messages and its own log lines.
The runtime MUST relay the output of a workload container or a shell session unchanged.

> [!NOTE]
> A workload's output is the app's own, and the runtime cannot tell a value the app chose to print from one it leaked.
> Masking it would also corrupt output that happens to contain a short sensitive value, such as a binary stream or a terminal session.

### Matching

The runtime MUST match a value against the sensitive values of the app it concerns: the stored values of its secret parameters, and the derived values the evaluation of its current definition returned.
It MUST also match a value a lifecycle operation or a shell session records against that operation's or session's sensitive values: the values of its own secret params and of the secret params of every action it has invoked, and the derived values its action closures returned.

A response withholding a value MUST say so with a machine-readable marker, so that a client can tell an unset value from a withheld one, as describing an app's [parameters](../interface/apps.md#parameters) and [redacted values](../interface/apps.md#redacted-values), and the [generation history](../interface/apps.md#generation-history) define.

## Migration

When a parameter becomes secret, the runtime MUST move its stored value from non-secret storage to secret storage without operator action.
When a parameter stops being secret, the runtime MUST move its stored value from secret storage to non-secret storage without operator action.
The runtime MUST make either move after every evaluation of the app's definition it applies, whatever caused the evaluation.

The runtime MUST write the value to its new storage before removing it from the old.
A generation history entry recorded while the parameter was secret MUST stay protected, and MUST stay [withheld](#redaction) from callers, after the parameter stops being secret.
