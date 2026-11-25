---
id: BND
---

# Definition bundles

Applies to the files of a [definition bundle](../overview.md#definition-bundle): their paths, the metadata file and its metadata table, how they form an app's [script](overview.md#script), and how a script reads them.

## Terminology

### Bundle path

The path of a file within a definition bundle, relative to the bundle root, with `/` as its separator.

### Script file

A bundle file whose contents form part of the app's script.

### Sidecar file

A bundle file that is not a script file.

### Included file

A bundle file that a client copies into the bundle, when it assembles it, from a source path the metadata file names.

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
| `metadata` | table | no | the [metadata table](#metadata-table), free-form values for the script to read |
| `include` | table of strings | no | the [included files](#included-files), each key the file's bundle path and each value the path it is assembled from |

A definition whose metadata has no `seedling` field supports every Seedling version.
If the metadata has no `script` field, the script files MUST be taken to be `["app.seed.rhai"]`.
A bundle without a metadata file MUST be treated as one whose metadata has none of these fields.

The `script` array MUST name at least one file.
It MUST NOT name the same file twice once its entries are normalised, and MUST NOT name `seedling.toml`.
Each file it names MUST be present in the bundle and MUST be valid UTF-8.

When an app is registered with a definition or updated to one, the server MUST refuse a definition whose metadata file breaks these rules with [`bundle_invalid`](../interface/wire.md#error-codes), as [bundle rules](../interface/definitions.md#bundle-rules) specifies.

## Included files

Each key of the `include` table MUST be a valid bundle path, MUST NOT name `seedling.toml`, and MUST NOT name the same file as another key once the keys are normalised.
Each value MUST be a non-empty string.
Each file the table names MUST be present in the bundle.

The runtime MUST NOT interpret the table's values.
An included file is otherwise a bundle file like any other, and is a script file when the `script` array names it.

> [!NOTE]
> Requiring every included file to be present means a bundle published without being assembled is refused when it is pushed or fetched, rather than failing when its script is evaluated.

## Metadata table

The metadata table holds values the publisher of a definition chooses, under keys the publisher chooses.
The runtime MUST NOT interpret the metadata table, other than to check its values and convert them as below.

> [!NOTE]
> A publisher records the release a definition was published for as, say, `version = "2.71.4"`, and the script parses it with whichever [version parser](version.md) fits the release scheme.

The metadata table MUST NOT hold a floating-point number that is not finite, at any depth, and the server refuses a bundle whose table holds one, as [bundle rules](../interface/definitions.md#bundle-rules) specifies.

`app.metadata()` MUST return a map holding the metadata table, with each value converted as follows.

| TOML type | BSL value |
| --- | --- |
| string | string |
| integer | integer |
| float | floating-point number |
| boolean | boolean |
| offset date-time, local date-time, local date, local time | string holding the value in the form TOML gives for its type, with `T` between a date and a time |
| array | array of the converted elements |
| table, inline table | map of the converted values |

A bundle whose metadata has no `metadata` field MUST yield an empty map.
`app.metadata()` MUST return a new map on each call, so that a script changing the map changes nothing else.

A script MAY call `app.metadata` in any context.
It MUST read the metadata of the bundle the app was evaluated from.
Within a [gate's body](gate.md#body), `last.metadata` MUST read the metadata of the bundle of the gate's last success.

> [!NOTE]
> Reading a key the table does not hold gives the unit value, so a script tells an absent `version` apart from a present one by comparing it with `()`.

## Seedling version requirement

A Seedling version requirement is a string of one or more comparator sets separated by `||`.
Each comparator set is a comma-separated list of comparators in Cargo's [version requirement syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax), such as `>=0.13, <0.15`.
Whitespace around `||` is ignored, and a requirement with an empty comparator set is invalid.

A version satisfies a requirement when it satisfies every comparator of at least one comparator set.
A version with a pre-release satisfies a comparator set only when a comparator of that set names a pre-release with the same major, minor and patch numbers, so `2.71.4-rc.1` satisfies `>=2.71.4-rc.0` but not `~2.71`.
Seedling versions are [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) versions, and a script matches a [SemVer](version.md#semver-versions) value against a requirement of this form.

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
Within a [gate's body](gate.md#body), `last.file` and `last.dir` MUST read the bundle of the gate's last success.

A `File` is an immutable sequence of bytes.
`file.text()` MUST return the file's contents as a string, and MUST throw if they are not valid UTF-8.

A `Directory` is consumed by [`volume.write_dir`](volume.md#declared-file-writes).
