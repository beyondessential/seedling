---
id: VER
---

# Versions

Applies to the `version` and `version-semver` [param kinds](action.md#param-kinds), their [parsers](action.md#kind-parsers), and the version values those parsers return.

## Borrowed terms

| term | meaning |
| --- | --- |
| libversion order | the ordering of the [Repology version comparison algorithm](https://github.com/repology/libversion/blob/master/doc/ALGORITHM.md), with none of its flags set |
| SemVer version, precedence | as defined by [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) |

## Generic versions

`std::parse_version(text)` MUST return a `Version` for a value it accepts.
It MUST accept a value made only of printable ASCII characters other than space, holding at least one ASCII digit, and MUST reject every other value.

> [!NOTE]
> The `version` kind is for values whose scheme Seedling does not know, such as a Debian package version or a partial image tag like `16.4`.
> Requiring a digit rejects floating tags such as `latest` or `stable`, which libversion order would otherwise place below every numbered release.

Two `Version` values MUST compare by libversion order through `<`, `<=`, `>`, `>=`, `==` and `!=`.
A leading `v` or `V` directly followed by an ASCII digit MUST be disregarded when ordering, so that `v2.0` and `2.0` are equal.

> [!NOTE]
> libversion order is the ordering the [VERS specification](https://github.com/package-url/vers-spec) names as the likely basis of its `generic` type.
> Under it, `1.0` and `1.0.0` are equal, and `1.0alpha1` and `1.0a1` are equal.
> Plain libversion order reads a leading `v` as a pre-release component, placing `v2.0` below `0.1`.

`version.segments()` MUST return the components libversion order splits the value into, in order, without a leading `v` or `V` that ordering disregards.
Each run of digits MUST be an integer, and each run of letters MUST be a string as written.
`std::parse_version` MUST reject a value holding a run of digits too large for a BSL integer.

> [!NOTE]
> `v1.0a1` has the segments `1`, `0`, `"a"` and `1`.

## SemVer versions

`std::parse_version_semver(text)` MUST return a `SemVer` for a value it accepts.
It MUST accept a value that is a SemVer version, or a `v` followed by a SemVer version, and MUST reject every other value.
It MUST reject a value whose major, minor or patch number is too large for a BSL integer.

> [!NOTE]
> A SemVer version always has a major, minor and patch number, so `2.71` is not a `version-semver` value.
> A script taking a partial version declares the `version` kind.

| method | returns |
| --- | --- |
| `major()` | the major number, as an integer |
| `minor()` | the minor number, as an integer |
| `patch()` | the patch number, as an integer |
| `pre_release()` | the pre-release identifiers, joined by `.`, or the empty string when there are none |
| `build()` | the build metadata, or the empty string when there is none |

Two `SemVer` values MUST compare by SemVer precedence through `<`, `<=`, `>`, `>=`, `==` and `!=`.

> [!NOTE]
> Precedence disregards build metadata, so `1.0.0+a == 1.0.0+b` holds, and a script telling them apart compares their `build()`.

`semver.matches(requirement: string)` MUST return `true` when the version satisfies `requirement` as a [Seedling version requirement](bundle.md#seedling-version-requirement), and `false` otherwise.
`matches` MUST raise a script error when `requirement` is not a valid Seedling version requirement.

## Mixing version types

Comparing a `Version` or a `SemVer` with a value of any other type, including the other version type, MUST raise a script error.

> [!NOTE]
> The two orderings disagree on some values: libversion order places `1.0.0+build` before `1.0.0`, where SemVer precedence holds them equal.
