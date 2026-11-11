---
id: BND
---

# Definition bundles

Applies to the files of a [definition bundle](../overview.md#definition-bundle): their paths, the metadata file, how they form an app's [script](overview.md#script), and how a script reads them.

## Terminology

### Bundle path

The path of a file within a definition bundle, relative to the bundle root, with `/` as its separator.

### Script file

A bundle file whose contents form part of the app's script.

### Sidecar file

A bundle file that is not a script file.

## Bundle paths

A definition bundle is a tree of files, each identified by its bundle path.
A bundle path is normalised lexically, without consulting any filesystem: empty and `.` segments are removed, and each `..` segment removes the segment before it.

A bundle path is invalid if it is absolute, contains a null byte, or rises above the bundle root at any `..` segment during normalisation.

A definition supplied as a script alone MUST be treated as a bundle holding that script as its only file, at `app.seed.rhai`.

## Metadata file

A bundle MAY hold a metadata file at the bundle path `seedling.toml`.
The metadata file MUST be a [TOML](https://toml.io/en/) document holding only the fields below.

| field | type | required | meaning |
| --- | --- | --- | --- |
| `seedling` | string | no | a [Seedling version requirement](#seedling-version-requirement) naming the Seedling versions the definition supports |
| `script` | array of strings | no | the bundle paths of the script files, in concatenation order |

A definition whose metadata has no `seedling` field supports every Seedling version.
If the metadata has no `script` field, the script files MUST be taken to be `["app.seed.rhai"]`.
A bundle without a metadata file MUST be treated as one whose metadata has neither field.

The `script` array MUST name at least one file.
It MUST NOT name the same file twice once its entries are normalised, and MUST NOT name `seedling.toml`.
Each file it names MUST be present in the bundle and MUST be valid UTF-8.

## Seedling version requirement

A Seedling version requirement is a string of one or more comparator sets separated by `||`.
Each comparator set is a comma-separated list of comparators in Cargo's [version requirement syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax), such as `>=0.13, <0.15`.
Whitespace around `||` is ignored, and a requirement with an empty comparator set is invalid.

A Seedling version satisfies a requirement when it satisfies every comparator of at least one comparator set.
Seedling versions are [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) versions.

## Script formation

An app's script MUST be the contents of its script files, concatenated in the order the metadata lists them.
Each script file MUST begin on a line of its own: if a script file's contents are not empty and do not end with a line feed, the runtime MUST insert a line feed before the next script file's contents.

An error arising during evaluation MUST name the script file it arose in and the line within that file, not a position in the concatenated script.

## Bundle files

`app.file(path: string)` MUST return a `File` holding the contents of the file at the bundle path `path`.
It MUST throw if `path` is invalid or names no file in the bundle.

`app.dir(path: string)` MUST return a `Directory` holding every file in the bundle beneath the bundle path `path`, each at its path relative to `path`.
A `path` that normalises to the bundle root names the whole bundle.
It MUST throw if `path` is invalid or the bundle holds no file beneath it.

A script MAY call `app.file` and `app.dir` in any context.
They MUST read the bundle the app was evaluated from.
Within a [parameter change handler](parameter.md#change-handlers), `old.file` and `old.dir` MUST read the bundle of the previous generation.

A `File` is an immutable sequence of bytes.
`file.text()` MUST return the file's contents as a string, and MUST throw if they are not valid UTF-8.

A `Directory` is consumed by [`volume.write_dir`](volume.md#declared-file-writes).
