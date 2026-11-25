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

## Text examined

Each search a method of a `Regex` runs MUST count every byte of the UTF-8 encoding of the text that the search reads toward the regex text examined [limit](../runtime/engine-limits.md#limits).
A byte that several searches read MUST count once for each of them.

> [!NOTE]
> A single search reads each byte of its text a bounded number of times, but a method that finds every match runs one search after another, and a search can read to the end of the text before reporting a match near its start.
> Counting what each search reads bounds that rescanning, which would otherwise take time growing with the square of the text's length, while an ordinary pass over a long text costs about its length once.
