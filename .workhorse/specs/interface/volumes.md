---
id: VOLM
---

# Volume methods

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an operator manages the held volumes, [site volumes](../overview.md#site-volume) and external volume mappings of a [host](../overview.md#host), and lists the volumes its apps declare.

## Terminology

### Volume reference

A JSON object naming one volume on the host by its `kind`.

| `kind` | other members | names |
| --- | --- | --- |
| `"site"` | `name` (string) | the site volume of that name |
| `"app"` | `app` (string), `volume` (string) | the volume `volume` declared by the app `app` |
| `"held"` | `id` (string) | the [held volume](../runtime/actuation.md#held-volume) with that identifier |

Each method that takes a volume reference names the kinds it accepts.
The server MUST reject a volume reference of any other kind with `requirements_invalid`.

### Volume string

The text form of a site or app volume reference: `_site/<name>` for the site volume `name`, and `<app>/<volume>` for the volume `volume` declared by the app `app`.

## Requests

The server MUST reject with `requirements_invalid` every app, volume, site volume and external volume name a method below takes that breaks the [name rules](../language/overview.md#names).
A held volume's identifier is a UUID in its hyphenated text form, and the server MUST reject an identifier that is not with `requirements_invalid`.

A request the server answers with an error MUST leave the host's held volumes, site volumes and external volume mappings, their records and their storage, as they were before the request.

Every event a method below emits carries the request's [actor](wire.md#actor-attribution), and the runtime records it in the [audit log](../runtime/audit-log.md#entries).

## Held volumes

### Listing held volumes

`/volumes/held/list` takes no parameters, and its result is an array with one record for each held volume on the host.

| member | type | meaning |
| --- | --- | --- |
| `id` | string | the held volume's identifier |
| `app` | string | the app the volume was held from, or `_site` for a site volume |
| `volume_name` | string | the name the volume was held under |
| `display_name` | string | a name for the held volume, for a person to read |
| `reason` | string | why the volume was held, for a person to read |
| `held_at` | string | when the volume was held, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |

If the server cannot read a held volume's record, it MUST answer with `internal`, and MUST NOT leave that volume out of the result.

### Deleting a held volume

`/volumes/held/delete` confirms the deletion of a held volume, as [held volumes](../runtime/actuation.md#held-volumes) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `id` | string | yes | the held volume's identifier |

The server MUST reject an `id` that names no held volume with `not_found`.
If the held volume's data cannot be deleted, the server MUST answer with `internal`.
On success the result is `{ "deleted": true }`, and the server MUST emit a `HeldVolumeDeleted` [event](events.md#site-resource-events).

### Restoring a held volume

`/volumes/held/restore` restores a held volume into a new managed site volume, as [held volumes](../runtime/actuation.md#held-volumes) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `id` | string | yes | the held volume's identifier |
| `target_name` | string | no, default the held volume's `volume_name` | the new site volume's name |

The server MUST reject an `id` that names no held volume with `not_found`.
Where `target_name` is absent and the held volume's `volume_name` does not follow the name rules, the server MUST reject the request with `requirements_invalid`.
The server MUST reject a target name that is [reserved](../runtime/identity.md#reserved-names) or [taken](../runtime/site-resources.md#site-volume-names) with `requirements_invalid`.
If the held volume's data cannot be moved, the server MUST answer with `internal`.

On success the result is `{ "restored": true, "name": <string> }`, giving the new site volume's name.
The server MUST emit a `HeldVolumeRestored` event.

## Site volumes

### Creating a site volume

`/volumes/site/create` creates a managed or bind [site volume](../runtime/site-resources.md#site-volumes).

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site volume's name |
| `kind` | string | yes | `"managed"` or `"bind"` |
| `host_path` | string | for a bind site volume | the host path the bind site volume mounts |

The server MUST reject any other `kind` with `requirements_invalid`.
The server MUST reject a bind site volume without `host_path`, or whose `host_path` is not an absolute path, with `requirements_invalid`.
The server MUST reject a request giving `host_path` for a managed site volume with `requirements_invalid`.
The server MUST reject a `name` that is reserved or taken with `requirements_invalid`.
If the managed site volume's storage cannot be created, the server MUST answer with `internal`.

On success the result is `{ "created": true }`, and the server MUST emit a `SiteVolumeCreated` event.

### Listing site volumes

`/volumes/site/list` takes no parameters, and its result is an array with one record for each site volume on the host, in ascending order of name.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the site volume's name |
| `kind` | string | `"managed"`, `"bind"` or `"snapshot"` |
| `created_at` | string | when the site volume was created, as an RFC 3339 timestamp |
| `host_path` | string | the host path a bind site volume mounts; present only for a bind site volume |
| `source` | string | the [volume string](#volume-string) of the volume a snapshot site volume was taken from; present only for a snapshot site volume |

### Deleting a site volume

`/volumes/site/delete` deletes a site volume, holding or deleting its storage by its kind, as [site volumes](../runtime/site-resources.md#site-volumes) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site volume's name |

The server MUST reject a `name` that names no site volume with `not_found`.
If the site volume's data cannot be held, or a snapshot site volume's storage cannot be deleted, the server MUST answer with `internal`.

On success the result is `{ "deleted": true }`.
Where the deletion held the volume's data, the server MUST emit a `HeldVolumeCreated` event.
The server MUST then emit a `SiteVolumeDeleted` event.

### Snapshotting a volume

`/volumes/site/snapshot` creates a snapshot site volume from an app volume or a managed site volume, as [site volume snapshots](../runtime/site-resources.md#site-volume-snapshots) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the new snapshot site volume's name |
| `source` | string | yes | the volume string of the volume to snapshot |

The server MUST reject a `source` that is not a volume string with `requirements_invalid`.
The server MUST reject a `source` naming no site volume, or naming an app volume that has no storage, with `not_found`.
The server MUST reject a `source` naming a bind or snapshot site volume with `requirements_invalid`.
The server MUST reject a `source` whose volume is not stored as a BTRFS subvolume with `requirements_invalid`.
The server MUST reject a `name` that is reserved or taken with `requirements_invalid`.

On success the result is `{ "created": true, "name": <string> }`, giving the snapshot site volume's name.
The server MUST emit a `SiteVolumeSnapshotted` event, whose `source` is the [volume reference](#volume-reference) of the volume snapshotted.

### Promoting a snapshot

`/volumes/site/promote` promotes a snapshot site volume into a new managed site volume, as [snapshot promotion](../runtime/site-resources.md#snapshot-promotion) specifies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `source` | string | yes | the snapshot site volume's name |
| `name` | string | yes | the new managed site volume's name |

The server MUST reject a `source` that names no site volume with `not_found`.
The server MUST reject a `source` that names a site volume other than a snapshot with `requirements_invalid`.
On a host whose data directory is not on BTRFS, the server MUST reject the request with `requirements_invalid`.
The server MUST reject a `name` that is reserved or taken with `requirements_invalid`.

On success the result is `{ "promoted": true, "name": <string> }`, giving the new site volume's name.
The server MUST emit a `SiteVolumePromoted` event, whose `source` is the snapshot site volume's name.

## App volumes

### Listing exported volumes

`/volumes/exported/list` takes no parameters, and its result is an array with one record for each [exported](../language/volume.md#export) volume declared by the current definition of a registered app, in ascending order of app name and then of volume name.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app's name |
| `volume_name` | string | the volume's name |
| `host_path` | string | the host path at which the volume's contents are stored, as [exported volumes](../runtime/actuation.md#exported-volumes) requires |
| `description` | string | the description the volume was exported with; absent where it has none |

### Listing app volumes

`/volumes/app/list` takes no parameters, and its result is an array with one record for each named non-tmpfs volume declared by the current definition of a registered app, exported or not, in ascending order of app name and then of volume name.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app's name |
| `volume_name` | string | the volume's name |
| `exported` | boolean | whether the volume is exported |
| `description` | string | the description the volume was exported with; absent where it is not exported or has none |

## External volume mappings

An external volume mapping binds the [external volume](../overview.md#external-volume) an app knows by `external_name` to a target, as [external mappings](../runtime/site-resources.md#external-mappings) specifies.
A mapping's target is a volume reference of kind `"site"` or `"app"`.

The server MUST reject a `"site"` target that names no site volume with `not_found`.
The server MUST reject an `"app"` target with `not_found` if its app is not registered or its app's current definition declares no volume of that name.
The server MUST reject an `"app"` target whose volume is not exported with `requirements_invalid`.

After each mapping it creates, changes or removes, the runtime SHOULD begin a [reconciliation loop](../runtime/reconciliation.md#reconciliation-loop) iteration without waiting for the next one to fall due.

### Mapping an external volume

`/volumes/external/map` creates a mapping.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `external_name` | string | yes | the external volume's name |
| `target` | volume reference | yes | the volume to map it to |
| `read_only` | boolean | no, default `false` | maps the volume read-only |

The server MUST reject the request with `not_found` if the app is not registered.
The server MUST reject an `external_name` that is reserved with `requirements_invalid`.
The server MUST reject the request with `requirements_invalid` if the app already has a mapping for `external_name`.
The server MUST accept an `external_name` that the app's current definition does not declare.

> [!NOTE]
> An action closure can resolve a dynamic external volume against the host's mappings, as [external volumes](../language/volume.md#external-volumes) specifies, so a mapping can serve a name the static definition never declares.

On success the result is `{ "mapped": true }`, and the server MUST emit an `ExternalVolumeMapped` event.

### Remapping an external volume

`/volumes/external/remap` changes the target and access mode of an existing mapping, and takes the same parameters as `/volumes/external/map`.

The server MUST reject the request with `not_found` if the app has no mapping for `external_name`.
On success the result is `{ "remapped": true }`, and the server MUST emit an `ExternalVolumeRemapped` event, carrying the mapping's target and access mode before the request.

### Unmapping an external volume

`/volumes/external/unmap` removes a mapping.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `external_name` | string | yes | the external volume's name |

If the app is registered and its current definition declares an external volume named `external_name`, the server MUST reject the request with `requirements_invalid`.
The server MUST reject the request with `not_found` if the app has no mapping for `external_name`.
On success the result is `{ "unmapped": true }`, and the server MUST emit an `ExternalVolumeUnmapped` event.

### Listing mappings

`/volumes/external/list` lists external volume mappings.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's mappings |

The result is an array with one record for each mapping, in ascending order of app name and then of external name.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app the mapping belongs to |
| `external_name` | string | the external volume's name |
| `target` | volume reference | the mapping's target |
| `read_only` | boolean | whether the volume is mapped read-only |

### Listing declared external volumes

`/volumes/external/declared` takes no parameters, and its result is an array with one record for each external volume declared by the current definition of a registered app, in ascending order of app name and then of external volume name.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app's name |
| `name` | string | the external volume's name |
| `description` | string | the external volume's [description](../language/overview.md#descriptions); absent where it has none |
