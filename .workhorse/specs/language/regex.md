---
id: REX
---

# Regular expressions

Applies to the [helper](std.md#helper) through which a [script](overview.md#script) compiles a regular expression, and to the `Regex` values it returns.

## Borrowed terms

| term | meaning |
| --- | --- |
| regex syntax | the syntax of a regular expression as documented for the [Rust `regex` crate](https://docs.rs/regex/1/regex/#syntax), major version 1 |

> [!NOTE]
> The regex syntax is referenced by its current documentation, and the implementation follows the release it tracks, so a Seedling release can change what a Unicode class such as `\p{Greek}` matches as the Unicode data behind it changes.

## Compiling a pattern

The std module MUST offer `std::regex(pattern: string)`, which returns a `Regex` for `pattern` read in the regex syntax.
`std::regex` MUST throw when `pattern` is not a regular expression in the regex syntax.
`std::regex` MUST throw when `pattern`, once compiled, exceeds the size the runtime bounds a compiled pattern to.

> [!NOTE]
> Repetition is what makes a compiled pattern large: `\w{1000}{1000}` is short to write and enormous once compiled.
> The text of a pattern is already bounded by the string length [limit](../runtime/engine-limits.md#limits).
