---
id: REX
---

# Regular expressions

Applies to the [helper](std.md#helper) through which a [script](overview.md#script) compiles a regular expression, the `regex` [param kind](action.md#param-kinds) and its [parser](action.md#kind-parsers), and the `Regex` values they return.

## Borrowed terms

| term | meaning |
| --- | --- |
| regex syntax | the syntax of a regular expression as documented for the [Rust `regex` crate](https://docs.rs/regex/1/regex/#syntax), major version 1 |
| leftmost-first | the match semantics that documentation describes, under which a search reports the match that starts earliest in the text, preferring among those starting there the one the pattern's alternations and repetitions prefer |

> [!NOTE]
> The regex syntax is referenced by its current documentation, and the implementation follows the release it tracks, so a Seedling release can change what a Unicode class such as `\p{Greek}` matches as the Unicode data behind it changes.

## Compiling a pattern

The std module MUST offer `std::regex(pattern: string)`, which returns a `Regex` for `pattern` read in the regex syntax.
`std::regex` MUST throw when `pattern` is not a regular expression in the regex syntax.
`std::regex` MUST throw when `pattern`, once compiled, exceeds what remains of the [compiled size](#compiled-size) limit.

Beyond the [single-line rule](action.md#kind-parsers) every parser applies, `std::parse_regex(text)` MUST reject a value for which `std::regex` throws, and MUST return what `std::regex(text)` returns for any other value.

A script that calls `std::regex` or `std::parse_regex` with a string literal, written without interpolation, for which that helper would throw with nothing yet counted toward the compiled size limit MUST fail [evaluation](overview.md#evaluation), whether or not the call would run.

> [!NOTE]
> Most patterns are written into the script, and a malformed one in a branch that rarely runs, such as an action, is then reported when the definition is first evaluated rather than when an operator first runs that action.

A `Regex` MUST convert to a string as the pattern it was compiled from, unchanged.

## Searching

A search MUST report the leftmost-first match of the pattern in the text, wherever in the text it starts.

> [!NOTE]
> A script that needs the whole text to match anchors the pattern with `^` and `$`.
> Unless the `m` flag is set, `$` matches only at the very end of the text, so a trailing newline does not slip past it.

A method that finds every match MUST run successive searches, each starting where the previous match ended, and report the matches in the order found.
When a search finds an empty match where the previous match ended, the method MUST NOT report it, and MUST resume searching one character further on.

> [!NOTE]
> Searching `b*` in `abba` finds the empty match before `a`, then `bb`, then the empty match at the end; the empty match between `bb` and the final `a` touches the end of `bb` and is skipped.

### Positions

A position in a text MUST count the characters before it, from 0, as BSL string positions are counted.

> [!NOTE]
> Counting in characters rather than bytes lets a script pass a position straight to a string's own methods, such as `sub_string`, whatever the text holds.

### Matches

A match MUST be an object map with these entries.

| key | value |
| --- | --- |
| `text` | the text matched, possibly empty |
| `start` | the position where the match starts |
| `end` | the position where the match ends |

### Captures

The captures of a match MUST be an object map with these entries.

| key | value |
| --- | --- |
| `groups` | an array holding, for each capture group in the pattern in order of its opening parenthesis, the match that group captured, or `()` where the group took no part in the match; its first element is the match of the whole pattern |
| `named` | an object map from the name of each named capture group in the pattern to the same value its element of `groups` holds |

## Methods

A `Regex` MUST offer these methods, each taking the text to search as a string.

| method | returns |
| --- | --- |
| `regex.is_match(text: string)` | whether a search finds a match |
| `regex.find(text: string)` | the match a search finds, or `()` |
| `regex.find_all(text: string)` | an array of every match |
| `regex.captures(text: string)` | the captures of the match a search finds, or `()` |
| `regex.captures_all(text: string)` | an array of the captures of every match |
| `regex.replace(text: string, replacement)` | `text` with the match a search finds replaced, as [replacements](#replacements) specify |
| `regex.replace_all(text: string, replacement)` | `text` with every match replaced, as [replacements](#replacements) specify |
| `regex.split(text: string)` | an array of the pieces of `text` between every match |
| `regex.split(text: string, limit: int)` | an array of the pieces of `text` between matches, holding at most `limit` pieces |

`split` MUST return every piece, including an empty piece before a match at the start of the text, between two adjacent matches, or after a match at the end of the text.
Given a `limit`, `split` MUST stop at the match that would make a piece beyond the limit, and its last piece MUST be the rest of the text from the end of the previous match.
`split` MUST throw when `limit` is less than 1.

### Replacements

A replacement MUST be a string or a closure, and any other value MUST raise a script error.

A replacement string replaces each match with the string, after expanding its group references:

- `$$` MUST expand to a single `$`.
- `${ref}` MUST expand to the text the capture group `ref` captured, where `ref` is the group's name, or its number when `ref` is made only of decimal digits, number 0 being the whole match.
- `$` followed by ASCII letters, ASCII digits and underscores MUST expand as `${ref}` does, with `ref` the longest such run.
- A group that took no part in the match MUST expand to an empty string.

`replace` and `replace_all` MUST throw when a replacement string holds a `$` that does not begin one of these forms, or refers to a group number or name the pattern does not have, whether or not the text holds a match.

> [!NOTE]
> `$1a` names a group `1a`, which no pattern can have since a group's name cannot begin with a digit, so it throws; `${1}a` is the first group followed by `a`.

A replacement closure MUST be called once for each match, in the order found, with the captures of that match as its one argument, and the match MUST be replaced with the string it returns, without expanding group references in it.
A replacement closure that returns anything other than a string MUST raise a script error.

## Compiled size

Each call to `std::regex` or `std::parse_regex` MUST count the size of the pattern it compiles toward the regex compiled size [limit](../runtime/engine-limits.md#limits), whether or not the same pattern was compiled before.
When a pattern's compiled size exceeds what remains of that limit, the helper MUST throw, and MUST count the whole remainder as used.

> [!NOTE]
> Repetition is what makes a compiled pattern large: `\w{1000}{1000}` is short to write and enormous once compiled.
> The text of a pattern is already bounded by the string length limit.
>
> Using up the remainder means a script that catches the error and tries again cannot keep compiling large patterns, since every later pattern is refused as soon as compiling starts.
> The size of a compiled pattern is measured by the engine, so a Seedling release can change how large a given pattern counts as.

## Text examined

Each search a method of a `Regex` runs MUST count every byte of the UTF-8 encoding of the text that the search reads toward the regex text examined [limit](../runtime/engine-limits.md#limits).
A byte that a search reads more than once, such as to find where its match starts or what the match's groups captured, MUST count once for each reading.
A byte that several searches read MUST count once for each of them.

> [!NOTE]
> A single search reads each byte of its text a bounded number of times, but a method that finds every match runs one search after another, and a search can read to the end of the text before reporting a match near its start.
> Counting what each search reads bounds that rescanning, which would otherwise take time growing with the square of the text's length, while an ordinary pass over a long text costs about its length once.
>
> How far a search reads to settle its result depends on how the engine searches, so a Seedling release can change the count at which a method reaches the limit.
