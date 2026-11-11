---
id: VOL
---

# Volumes

## Volume

<!-- from language: Volume -->

> l[volume.type]
> A Volume is a directory containing data.
>
> Volumes are defined using the `app.volume(name?: string)` method, which returns a [builder](rule:l/bsl.builder). The `name` argument is optional: if it's not provided, the volume is anonymous.
>
> Volumes can be [mounted](rule:l/container.mount-volume) to a container's filesystem.

> l[volume.readonly]
> `volume.readonly()` is a builder method which declares this volume to be read-only.
> A read-only volume cannot be written to.

> l[volume.tmpfs]
> `volume.tmpfs()` is a builder method which declares this volume to be backed by tmpfs (a RAM-based filesystem).
> The contents of a tmpfs volume do not survive a host reboot.

> l[volume.write]
> `volume.write(path: string, contents: string | File)` is an instance method which writes some data to the volume at `path`.
> A [`File`](rule:l/file.type) is written byte for byte as it is held in the bundle.
> Any existing content at `path` is discarded or shadowed.

> l[volume.write-dir]
> `volume.write_dir(path: string, dir: Directory)` is an instance method which writes every file in a [`Directory`](rule:l/app.dir) into the volume beneath `path`.
> Each file keeps its path relative to the directory, and is written byte for byte as it is held in the bundle.
> `path` is validated as for [volume.write](rule:l/volume.write.validation), except that it may resolve to the volume root.
> The writes are declared file writes in the same way as those of `volume.write`, and are applied and reapplied alike.

> l[volume.write.validation]
> The `path` argument must be an absolute path (starting with `/`), must not contain null bytes, and must not escape the volume root after canonicalisation (resolving `.` and `..` segments without touching the filesystem). A path that resolves to `/` itself is also forbidden.
>
> If validation fails, the method must throw.
>
> At the runtime level, files written into volumes must be created with restrictive permissions (no more than 0640).

> l[volume.exported]
> `volume.exported(options?: #{ description?: string })` is a builder method which marks the volume as exported. Exported volumes are advertised to the control plane and operators.
>
> Only named static volumes can be exported. Calling `exported()` on an anonymous volume must throw.

## External Volume

<!-- from language: Volume / External Volume -->

> l[volume.external]
> An External Volume is a Volume provided by the Seedling control plane to a BSL script, at a particular name.
>
> External Volumes are defined using the `app.external_volume(name: string)` method, which returns an `ExternalVolume`.
>
> External Volumes can't be modified or configured further, only [mounted](rule:l/container.mount-volume).

> l[volume.external.dynamic]
> When `app.external_volume(name)` is called within an action closure, the runtime checks operation-scoped volume bindings first, then falls back to the static external volume mapping table. Operation-scoped bindings are injected by the runtime for specific internal operations and are not operator-configurable.
>
> If the name resolves to an operation-scoped binding, the returned `ExternalVolume` references the bound path for the duration of the operation. The binding is removed when the operation ends.
>
> If the name does not match any operation-scoped binding, the lookup falls back to the static external volume mapping table as for any other external volume reference.
>
> The names used for operation-scoped bindings are not fixed strings: they are generated per invocation by the runtime and delivered to the action closure in reserved params. See [operation.volume-param](rule:r/operation.volume-param).
