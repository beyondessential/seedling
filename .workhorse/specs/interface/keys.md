---
id: KEY
---

# Key management

Applies to the set of client keys authorised to connect to the [Operator Interface](../overview.md#operator-interface).

## Fingerprints

A client key is named by its [fingerprint](transport.md#fingerprint).
The server MUST accept fingerprints in the forms [client authentication](transport.md#client-authentication) specifies, and MUST reject a fingerprint it refuses there with `requirements_invalid`.

## Listing keys

`/keys/list` takes no parameters.
The result is an array of key records, oldest first.

| member | type | meaning |
| --- | --- | --- |
| `fingerprint` | string | the key's fingerprint |
| `label` | string | the operator's label for the key |
| `added_at` | integer | when the key was first authorised, in seconds since the Unix epoch |

## Authorising a key

`/keys/authorise` adds a client key to the [authorised key set](transport.md#authorised-key-set), or relabels a key already in it.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `fingerprint` | string | yes | the key's [fingerprint](#fingerprints) |
| `label` | string | no, default `"unnamed"` | the operator's label for the key |

On success the result is `{}`.
The server MUST keep the `added_at` of a key it relabels.
The server MUST accept connections from an authorised key from the time the request succeeds.

## Revoking a key

`/keys/revoke` removes a client key from the authorised key set.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `fingerprint` | string | yes | the key's [fingerprint](#fingerprints) |

On success the result is `{}`.
The server MUST reject a fingerprint that is not in the authorised key set with `not_found`.
The server MUST apply a revocation to connections already open, as [client authentication](transport.md#client-authentication) requires.
