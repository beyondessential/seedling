---
id: IMGM
---

# Image management

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an operator manages the registry allowlist and the container images held on a [host](../overview.md#host).

## Borrowed terms

| term | meaning |
| --- | --- |
| image reference, tag, digest | as defined by the [OCI Distribution Specification](https://github.com/opencontainers/distribution-spec/blob/main/spec.md) |
| manifest, image index | as defined by the [OCI Image Format Specification](https://github.com/opencontainers/image-spec/blob/main/spec.md); an image index is also called a manifest list |

## Registry allowlist

The registry allowlist is the set of registry hosts from which the runtime accepts images and [definition bundles](definitions.md#registry-access).
An image's registry is the part of its reference before the first `/`.
The runtime MUST compare a registry against the allowlist by exact string equality.

`/registries/list` takes no parameters, and its result is `{ "registries": [ ... ] }`, the allowlisted registries as strings in ascending order.

`/registries/add` adds a registry to the allowlist, and `/registries/remove` removes one.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `registry` | string | yes | the registry host |

On success the result of either is `{ "ok": true }`.
Adding a registry already on the allowlist MUST succeed and change nothing.
The server MUST reject removing a registry that is not on the allowlist with `not_found`.

After either change, the runtime MUST re-evaluate every registered app, so that an app whose images reference a registry off the allowlist holds a `disallowed_registry` fault, and an app whose images all reference allowlisted registries holds none.

## Listing images

`/images/list` takes no parameters, and its result is `{ "images": [ ... ] }`, one record for each container image held on the host.

| member | type | meaning |
| --- | --- | --- |
| `image_id` | string | the image's identifier, as the host's container storage reports it |
| `tags` | array of strings | the tag references that resolve to the image |
| `digests` | array of objects | the digest references that resolve to the image |
| `manifest_digest` | string or null | the digest of the image's own manifest, where known |
| `size_bytes` | integer | the image's size in bytes |
| `created_at` | string | when the image was created, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `last_used_at` | string | the image's [last-used time](../runtime/images.md#image-usage-tracking), or its first-recorded time where it has never been in use, as an RFC 3339 timestamp |
| `in_use` | boolean | whether a running container uses the image |
| `pinned_by` | array of strings | the apps holding a [pin](../runtime/images.md#pins) on a reference that resolves to the image |

Each member of `digests` is:

| member | type | meaning |
| --- | --- | --- |
| `reference` | string | the digest reference |
| `kind` | string | `"manifest"` where the digest is `manifest_digest`, `"manifest_list"` where it is not, and `"unknown"` where `manifest_digest` is null |

> [!NOTE]
> An image pulled by a multi-architecture tag carries both its own manifest digest and the digest of the image index it was selected from.

`tags` and `digests` MAY be empty, for an image no reference resolves to.
In `pinned_by` each app MUST appear at most once.

## Pulling an image

`/images/pull` pulls an image reference into the host's container storage.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `reference` | string | yes | the image reference to pull |
| `app` | string | no | an app to pin the reference to |

The server MUST reject an empty `reference` with `requirements_invalid`.
The server MUST reject an `app` that is not a valid app name with `requirements_invalid`, and one naming an app that is not registered with `not_found`.
Where `app` is given, the runtime MUST pin `reference` to that app, whether or not the pull succeeds.

The server MUST answer once the pull has finished.
On success the result is `{ "ok": true }`.
If the pull fails, the server MUST answer with an error response whose `message` carries the cause.

## Removing an image

`/images/remove` removes an image from the host's container storage.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `reference` | string | yes | a reference resolving to the image |
| `force` | boolean | no, default `false` | removes the image even while a running container uses it |

The server MUST reject an empty `reference` with `requirements_invalid`.
Unless `force` is `true`, the server MUST reject the request with `requirements_invalid` while a running container uses the image.
Unless the request is rejected, the runtime MUST clear every pin on `reference`, whether or not an image is removed.
The server MUST reject a `reference` that resolves to no image on the host with `not_found`.
On success the result is `{ "ok": true }`.

## Image pins

`/images/pins/list` lists image pins.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's pins |

The server MUST reject an `app` that is not a valid app name with `requirements_invalid`.
The result is `{ "pins": [ ... ] }`.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app holding the pin |
| `reference` | string | the pinned image reference |
| `pinned_at` | string | when the pin was made, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `expires_at` | string or null | when the pin [expires](../runtime/images.md#pin-expiry), as an RFC 3339 timestamp, or `null` where it does not |

`/images/pins/clear` clears pins.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app whose pins to clear |
| `reference` | string | no | clears only the pin on this reference |

The server MUST reject an `app` that is not a valid app name with `requirements_invalid`.
Without `reference`, the runtime MUST clear every pin of the app.
Clearing a pin that does not exist MUST succeed and change nothing.
On success the result is `{ "ok": true }`.

## Image discovery

`/apps/images/discover` runs the [probe](../runtime/images.md#image-discovery) over every handler an app declares, and reports the images each might pull.
The handlers are the install action, the start action, every other action, every shell and every `on_change` handler the app declares.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `proposed_script` | string | no | script source to probe in place of the app's stored script |
| `action_params` | object | no | parameter values per handler: handler name to an object of parameter name to string value |
| `lenient` | boolean | no, default `false` | reports a handler with unresolved required parameters as skipped rather than as an error |

The server MUST reject a request naming an app that is not registered with `not_found`.
Where `proposed_script` is given, the runtime MUST evaluate it with the app's other bundle files and stored parameter values, and probe the result.
The server MUST reject a script that fails to compile or evaluate with `script_error`.

A value in `action_params` MUST take precedence over the handler's stored and default values, as [probe parameters](../runtime/images.md#probe-parameters) specifies.
A handler with a required parameter that resolves to no value MUST NOT be probed.
When `lenient` is `true`, such a handler MUST be reported with a `skipped_reason`; when `lenient` is `false`, it MUST be reported with an `error`.
The other handlers MUST be probed either way.

The result is:

| member | type | meaning |
| --- | --- | --- |
| `per_handler` | array of objects | one entry for each handler |
| `all_images` | array of strings | the distinct image references reported by handlers with neither `error` nor `skipped_reason`, in ascending order |

Each `per_handler` entry is:

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the handler's name |
| `kind` | string | `"install"`, `"start"`, `"action"`, `"shell"` or `"param_change"` |
| `images` | array of strings | the image references the handler might pull |
| `error` | string or null | why the handler could not be probed |
| `skipped_reason` | string or null | why a lenient probe skipped the handler |
