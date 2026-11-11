---
id: KEY
---

# Key management

Applies to the set of client keys authorised to connect to the [Operator Interface](../overview.md#operator-interface), and to the client identity key the command-line client keeps.

## Fingerprints

A fingerprint names a client key as [client authentication](rule:i/transport.client-auth) defines it: 64 hexadecimal digits.
The server MUST accept a fingerprint with surrounding whitespace, in either case, and with or without a `sha256:` or `SHA256:` prefix, and MUST store and compare it as 64 lower-case hexadecimal digits.
The server MUST reject any other value with `requirements_invalid`.

## Listing keys

`/keys/list` takes no parameters.
The result is an array of key records, oldest first.

| member | type | meaning |
| --- | --- | --- |
| `fingerprint` | string | the key's fingerprint |
| `label` | string | the operator's label for the key |
| `added_at` | integer | when the key was first authorised, in seconds since the Unix epoch |

## Authorising a key

`/keys/authorise` adds a client key to the authorised set, or relabels a key already in it.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `fingerprint` | string | yes | the key's [fingerprint](#fingerprints) |
| `label` | string | no, default `"unnamed"` | the operator's label for the key |

On success the result is `{}`.
Relabelling a key MUST keep its `added_at`.
An authorised key MUST be able to connect from the time the request succeeds.

## Revoking a key

`/keys/revoke` removes a client key from the authorised set.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `fingerprint` | string | yes | the key's [fingerprint](#fingerprints) |

On success the result is `{}`.
The server MUST reject a fingerprint that is not in the authorised set with `not_found`.
Revocation MUST take effect on connections already open, as [client authentication](rule:i/transport.client-auth) requires.

## Client identity key

The command-line client authenticates with an Ed25519 key ([RFC 8032](https://www.rfc-editor.org/rfc/rfc8032)) held in a key file as a DER-encoded PKCS#8 private key ([RFC 5958](https://www.rfc-editor.org/rfc/rfc5958), [RFC 8410](https://www.rfc-editor.org/rfc/rfc8410)).

The client MUST read the key file from the path given by its `--key-file` option.
Without the option, the client MUST use `seedling/client.key` under the user's state directory (`$XDG_STATE_HOME`, or its platform equivalent).
If the key file does not exist, the client MUST generate a key and write it there, creating missing parent directories.

The client MUST create the key file readable and writable by its owner only.
The client MUST refuse to load a key file that grants any permission to its group or to other users, and MUST report an error naming the file.

> [!NOTE]
> A distinct key file gives a caller a distinct identity, such as a co-located service that needs its own authorised key.
> A key can be generated at a chosen path and authorised before its first use.
