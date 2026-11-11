---
id: SEC
---

# Secret parameter storage

## Secret Parameter Storage

<!-- from runtime: Secret Parameter Storage -->

> r[secret.key]
> The runtime must provision and maintain a secret key for use in protecting secret parameter values.
> The key file must have the same file-permission restriction as the main database (access restricted to the owner).
> If no key file exists on startup the runtime must create one automatically with a freshly generated key.
> The key file must be stored on the same volume as the database.

> r[secret.storage]
> Secret parameter values (those whose effective `secret` flag is `true`) must be stored separately from non-secret parameters and must be protected at rest using the [secret key](rule:r/secret.key).
> Non-secret parameter values continue to be stored unprotected.
> The two storage locations are transparent to BSL scripts: `app.param(name).value()` returns the decrypted string regardless of whether the value is secret.

> r[secret.history]
> Entries in the [generation history](rule:r/generation.history) that record a previous or new value for a secret parameter must protect those values using the [secret key](rule:r/secret.key).
> Each of the two values is protected according to how it was held: the value being replaced is protected when it was read from secret storage, and the value replacing it is protected when it is written there.
> A definition that drops a parameter's `secret` flag in the same update that changes its value must therefore still protect the value it replaced, in history and in the event announcing the change.
> History retrieval must decrypt these values internally before serving them to callers that are authorised to reconstruct past generations.

> r[secret.redaction]
> When a secret parameter value is requested via an operator interface (such as a describe or history RPC), it must never be returned to the caller.
> The redaction decision is based on whether the parameter is currently secret in the live AppDef, regardless of what the schema was when the value was originally stored.
> Absence of the value must be signalled by a machine-readable marker so clients can distinguish "not set" from "set but redacted".

> r[secret.migration]
> When a parameter transitions from non-secret to secret (because the BSL script is updated or the `secret` flag is changed), the runtime must move the existing stored value from non-secret storage to secret storage at the next opportunity, without requiring operator intervention.
>
> The move follows every evaluation of the app's definition that the runtime applies, whatever caused it: startup, a definition update, a parameter change, or any other re-evaluation.
> The value is written to secret storage before it is removed from non-secret storage, so a failure part way leaves it readable.
