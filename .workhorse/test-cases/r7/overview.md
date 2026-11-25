# Regular expressions: test cases

## Compiling

- [x] A malformed pattern throws a catchable script error (verifies spec: REX#compiling-a-pattern)
- [x] A non-string pattern throws naming `pattern` (verifies spec: REX#compiling-a-pattern)
- [x] A `Regex` converts to its pattern (verifies spec: REX#compiling-a-pattern)
- [x] A malformed literal passed to `std::regex` or `std::parse_regex` fails evaluation in a function never called and in a branch never taken, including a backtick literal (verifies spec: REX#compiling-a-pattern)
- [x] A `const` or interpolated pattern is checked only when it runs (verifies spec: REX#compiling-a-pattern)
- [x] A literal too large for the whole limit fails evaluation (verifies spec: REX#compiling-a-pattern)

## Compiled size

- [x] Distinct patterns compiled in a loop are refused once the budget is spent, and after a refusal even a tiny pattern is refused (verifies spec: REX#compiled-size)
- [x] A cached pattern counts what it counted uncached (verifies spec: REX#compiled-size)
- [x] A limit of 0 removes the limit (verifies spec: LIM#limits)
- [x] The daemon option accepts 0 (verifies spec: LIM#limits)

## Searching and results

- [x] A search finds the leftmost-first match anywhere; `^…$` anchors; `$` does not match before a trailing newline (verifies spec: REX#searching)
- [x] `b*` on `abba` and `|a` on `a` skip the empty match touching the previous match (verifies spec: REX#searching)
- [x] Positions count characters, line up with `sub_string`, and an empty pattern never splits a character (verifies spec: REX#positions)
- [x] A match is `{text, start, end}`; no match is `()` (verifies spec: REX#matches)
- [x] Captures list every group with `()` for one that took no part, and name the named ones (verifies spec: REX#captures)
- [x] `find_all` and `captures_all` return every match in order (verifies spec: REX#methods)
- [x] `split` keeps empty pieces, honours a limit, and throws on a limit below 1 (verifies spec: REX#methods)
- [x] A non-string text throws naming `text` (verifies spec: BSL#arguments)

## Replacements

- [x] `$n`, `$name`, `${ref}`, `$$`, `$0` and a non-participating group expand as specified, for `replace` and `replace_all` (verifies spec: REX#replacements)
- [x] A stray `$`, an unclosed `${`, an unknown number or name, and `$1a` throw even when nothing matches (verifies spec: REX#replacements)
- [x] A closure gets each match's captures, its result is inserted unexpanded, and a non-string result or a replacement of another type throws (verifies spec: REX#replacements)

## Param kind

- [x] `std::parse_regex` returns a `Regex` and applies the single-line rule (verifies spec: ACN#kind-parsers)
- [x] A `regex` kind is accepted in app and action param schemas (verifies spec: ACN#param-kinds)
- [x] A malformed `default_value` makes declaring the schema throw (verifies spec: ACN#param-schemas)
- [x] An invocation refuses a malformed pattern and delivers a valid one (verifies spec: ACN#kind-parsers)
- [x] A `Regex` param survives persistence as a `Regex` (verifies spec: OPS#persistence)
- [x] The web UI offers a single-line input for a `regex` param (verifies spec: ACN#presentation-of-kinds)
- [x] `action.invoke` passes a received `Regex` param on as its pattern (verifies spec: ACN#action-invocation)
