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

Two `Version` values MUST compare by libversion order through `<`, `<=`, `>`, `>=`, `==` and `!=`.

> [!NOTE]
> The `version` kind is for values whose scheme Seedling does not know, such as a Debian package version or a partial image tag like `16.4`.
> libversion order is the ordering the [VERS specification](https://github.com/package-url/vers-spec) names as the likely basis of its `generic` type.
> Under it, `1.0` and `1.0.0` are equal, and `1.0alpha1` and `1.0a1` are equal.

## SemVer versions

`std::parse_version_semver(text)` MUST return a `SemVer` for a value it accepts.
It MUST accept a value that is a SemVer version, or a `v` followed by a SemVer version, and MUST reject every other value.

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

`semver.matches(requirement: string)` MUST return `true` when the version satisfies `requirement` as a [Seedling version requirement](bundle.md#seedling-version-requirement), and `false` otherwise.
`matches` MUST raise a script error when `requirement` is not a valid Seedling version requirement.

## Mixing version types

Comparing a `Version` with a `SemVer` MUST raise a script error.

> [!NOTE]
> The two orderings disagree on some values: libversion order places `1.0.0+build` before `1.0.0`, where SemVer precedence holds them equal.
