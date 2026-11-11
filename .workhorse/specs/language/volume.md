---
id: VOL
---

# Volumes

Applies to the BSL `Volume` and `ExternalVolume` types, the directories of data an app mounts into its containers.

## Terminology

### Volume

A [resource](../overview.md#resource) holding a directory of data that containers mount.

### External volume

A volume the host supplies to an app under a name the app declares.

### Declared file write

A file a script declares to be written into a volume, as a path within the volume and the bytes to write there.

## Declaration

A script declares a volume with `app.volume(name: string)`, or an anonymous volume with `app.volume()`, each of which returns a `Volume` [builder](overview.md#builder).
A container mounts a volume as [volume mounts](container.md#volume-mounts) specifies.

## Storage options

`volume.readonly()` declares the volume read-only.
The runtime MUST mount a read-only volume read-only into every container that mounts it.

`volume.tmpfs()` declares the volume memory-backed.
The runtime MUST hold a tmpfs volume's contents in memory only.
A tmpfs volume's contents do not survive a restart of the host.

## Declared file writes

`volume.write(path: string, contents: string)` and `volume.write(path: string, contents: File)` declare a file write of `contents` at `path`.
A [`File`](bundle.md#bundle-files) MUST be written byte for byte as the bundle holds it, and a string as its UTF-8 encoding.

`volume.write_dir(path: string, dir: Directory)` declares one file write for each file in the [`Directory`](bundle.md#bundle-files), at that file's path relative to the directory, beneath `path`.
Each file MUST be written byte for byte as the bundle holds it.

When the runtime applies declared file writes, it MUST apply them in the order the script declared them, and each MUST replace whatever its path held.
When the runtime applies declared file writes is specified by [actuation](../runtime/actuation.md#volume-creation-and-seeding), and the permissions of the files they create by [storage confinement](../runtime/actuation.md#storage-confinement).

### Write paths

A `path` MUST begin with `/`, and MUST NOT contain a NUL character.
A `path` whose canonical form lies outside the volume's root MUST throw.
The canonical form resolves `.` and `..` segments, without reference to any filesystem.
A `write` path whose canonical form is the volume's root MUST throw.
A `write_dir` path MAY resolve to the volume's root.

## Export

`volume.exported()` and `volume.exported(options: #{ description?: string })` mark the volume as exported.
The runtime MUST list exported volumes to operators, each with the `description` given.
An `options` map with any other key MUST throw.
`exported` called on an anonymous volume MUST throw.

## External volumes

A script declares an [external volume](#external-volume) with `app.external_volume(name: string)`, which returns an `ExternalVolume`.
An `ExternalVolume` has no builder methods other than [`description`](overview.md#descriptions).

In the [action context](app.md#action-context), `app.external_volume(name)` MUST return the static external volume `name` if the static context declared one.
Otherwise it MUST return a [dynamic](app.md#dynamic-resource) external volume named `name`.
A container mounts an external volume as [volume mounts](container.md#volume-mounts) specifies.

The runtime MUST resolve an external volume's name against the bindings of the [lifecycle operation](../overview.md#lifecycle-operation) in progress, if any, and then against the host's external volume mappings.
The runtime makes an operation's bindings for that operation.
A name resolved against an operation's binding MUST refer to the bound storage until the operation ends, and to nothing after it.

> [!NOTE]
> An operation's binding names are generated for each invocation and reach the action closure under [reserved param keys](action.md#param-delivery), as [operation volume params](../runtime/operations.md#operation-scoped-volumes) specifies.

An external volume that resolves to nothing is handled as [an unmapped external volume](../runtime/faults.md#workload-faults) specifies.
