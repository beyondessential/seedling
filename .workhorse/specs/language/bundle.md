---
id: BND
---

# Definition bundles

## Preamble

<!-- from language: None -->

> l[bsl.bundle]
> An application's definition is a *bundle*: a tree of files, one or more of which are its *script files*.
> The BSL script that defines the application is the script files' contents concatenated in the order the bundle's [metadata](rule:l/bsl.bundle.metadata) lists them.
> Every other file in the bundle is a *sidecar file*, which the script can [read](rule:l/app.file) and copy into volumes.
> A definition consisting of a script alone is a bundle holding only `app.seed.rhai`.

> l[bsl.bundle.metadata]
> A bundle may hold a metadata file, `seedling.toml`, at its root.
> It has two optional fields:
>
> - `seedling`: a [Seedling version requirement](rule:l/bsl.bundle.seedling-versions) naming the Seedling versions the definition supports. Absent, the definition supports every version.
> - `script`: an array of bundle paths naming the script files, in concatenation order. Absent, it is `["app.seed.rhai"]`.
>
> A bundle without the file is treated as one whose fields are all absent.
> The file is part of the bundle and is not itself a script file.

> l[bsl.bundle.seedling-versions]
> A Seedling version requirement is a string of one or more semantic-version comparator sets separated by `||`, each set a comma-separated list of comparators such as `>=0.13, <0.15`.
> A Seedling version satisfies the requirement when it satisfies every comparator of at least one set.

> l[bsl.bundle.script-errors]
> An evaluation error arising in a script file names that file and the line within it, not a position in the concatenated script.

## Bundle files

<!-- from language: App global / Bundle files -->

> l[app.file]
> `app.file(path: string)` returns a `File` holding the contents of the file at `path` in the app's [bundle](rule:l/bsl.bundle).
> `path` is relative to the bundle root, uses `/` as its separator, and is normalised without touching any filesystem.
> A path that is absolute, contains a null byte, escapes the bundle root after normalisation, or names no file in the bundle throws.

> l[app.dir]
> `app.dir(path: string)` returns a `Directory` holding every file in the bundle beneath `path`, each at its path relative to `path`.
> `path` follows the same rules as for [app.file](rule:l/app.file), and a path beneath which the bundle holds no file throws.

> l[app.bundle.context]
> `app.file` and `app.dir` may be called in any context, and always read the bundle the app was evaluated from.
> Within an [`on_change`](rule:l/param.on-change.old) handler, `old.file` and `old.dir` read the previous generation's bundle.

> l[file.type]
> A `File` is an immutable sequence of bytes read from the bundle.

> l[file.text]
> `file.text()` returns the file's contents as a string, and throws if they are not valid UTF-8.
