---
id: SEC
---

# Secret parameter storage

Applies to how the [runtime](../overview.md#runtime) protects secret values at rest, and withholds secret parameter values from operators.

## Terminology

### Secret parameter

A parameter whose effective [`secret` flag](../language/parameter.md#secret-parameters) is `true` in the app's current [resource graph](../overview.md#resource-graph).

## Secret key

The runtime MUST keep a secret key for protecting secret values at rest, in the file `seedling.db.key` in its [data directory](actuation.md#data-directory), beside the database.
The key file MUST carry the same [file permissions](infrastructure.md#file-permissions) as the database.
If no key file exists when the runtime starts, the runtime MUST create one holding a freshly generated key.

## Storage

The runtime MUST store the values of secret parameters apart from the values of other parameters.
It MUST protect them at rest with authenticated encryption under the secret key.

A script reading a parameter's [value](../language/parameter.md#values) MUST receive the value in plain form whether or not the parameter is secret.

## Generation history

An entry in the [generation history](generation.md#history-entries) recording a previous or new value of a parameter MUST protect each value under the secret key according to how that value was held:

- the value being replaced MUST be protected if it was read from secret storage;
- the value replacing it MUST be protected if it is written to secret storage.

The event announcing the change MUST withhold a value under the same rule.

> [!NOTE]
> A definition that drops a parameter's `secret` flag in the same update that changes its value still protects the value it replaced.

The runtime MUST decrypt protected history values before giving them to a caller authorised to reconstruct past generations.

## Redaction

The runtime MUST NOT return the value of a secret parameter to a caller of the [Operator Interface](../overview.md#operator-interface).
It MUST NOT return a value recorded in the generation history if the parameter is secret now, or if the value was protected when it was recorded.
A response withholding a value MUST say so with a machine-readable marker, so that a client can tell an unset value from a withheld one, as [describing an app](../interface/apps.md#parameters) and the [generation history](../interface/apps.md#generation-history) define.

## Migration

When a parameter becomes secret, the runtime MUST move its stored value from non-secret storage to secret storage without operator action.
The runtime MUST make that move after every evaluation of the app's definition it applies, whatever caused the evaluation.

The runtime MUST write the value to secret storage before removing it from non-secret storage.
