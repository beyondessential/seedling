---
id: KEY
---

# Key management

## Key Management

<!-- from interface: Key Management -->

> i[key.list]
> `/keys/list` returns the list of authorized client keys.
> Response `result` is an array of objects, each with `fingerprint` (string), `label` (string),
> and `added_at` (Unix timestamp integer).

> i[key.authorize]
> `/keys/authorise` adds a client key to the authorized set, or updates its label if the key is
> already authorized.
> Params: `fingerprint` (string, required), `label` (string, required).
> Returns `{}` on success. The original `added_at` timestamp is preserved on label update.

> i[key.revoke]
> `/keys/revoke` removes a client key from the authorized set.
> Params: `fingerprint` (string, required).
> Returns `{}` on success. Returns `not_found` if the fingerprint is not known.
> Revocation takes effect on connections already open, as
> [transport.client-auth](rule:i/transport.client-auth) requires.

> i[key.client.file-permissions]
> The client identity key file must be created with owner-read/write-only permissions.
> Seedling-ctl must refuse to load a client identity key file whose group or world
> permission bits are set, and must report an error.

> i[key.client.path]
> The client identity key file location must be configurable.
> When unset it defaults to a per-user state directory.
> Overriding it lets a caller maintain a distinct identity — for example, a co-located
> service that needs its own authorized key, or pre-generating a key at a chosen path
> to authorise it before first use.
