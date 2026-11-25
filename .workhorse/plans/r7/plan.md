# Regular expressions

Adds `std::regex`, the `Regex` type, the `regex` param kind and its parser, the regex compiled size limit, and the definition-time refusal of malformed literal patterns, per the Regular expressions spec (REX).

## Technical notes

- Engine: `regex_automata::meta::Regex`, the engine behind the `regex` crate, used directly because it reports its memory usage and takes an NFA size limit. Ruled out after a survey: `regex-lite` (no Unicode classes), `fancy-regex` (backtracking), `regress` (unbounded backtracking), `resharp` (no captures, non-standard syntax), `pcre2` (wrapper exposes no match limit), `onig` (C dependency).
- No search budget. A byte count does not bound search time, since a search's work grows with the pattern's active states as well as with the text: `(?:a|b|ab){4000}c` takes 7.6 s over 100 KB in any linear engine. Counting work exactly needs an engine whose per-byte work is deterministic, which rules out the lazy DFA. Operators are trusted, so a runaway evaluation is left to the circuit breaker on U7 rather than bounded here.
- Compiled size (REX#compiled-size): build with the NFA size limit set to what remains, then refuse when `memory_usage()` exceeds it too, since the meta engine builds more than the NFA. A refusal uses up the remainder. Script calls count in a per-evaluation counter in `runtime::seeds`, reset by `AppSeedFrame` and `OperationSeedFrame` as the derived-values count is, with the limit in `HelperLimits`.
- `ParamKind::parse` takes a compile budget. Script calls of `std::parse_regex` pass the evaluation's; the runtime's own checks (`runtime::apps`, `runtime::action_params`, `param.parsed()`, schema defaults) pass a fresh budget at the configured limit, since REX counts only script calls. Restoring a persisted `Regex` param recompiles without a limit, since the value was accepted when it was stored.
- Compiled patterns are cached by pattern text across evaluations, with their compiled size, so a hit counts what a miss would and the cache stays invisible to scripts.
- The literal-pattern check needs Rhai's `internals` feature for `AST::walk`. Rhai's optimiser propagates `const` values into call arguments, so the engine compiles with optimisation off, the walk runs, and `defs::compile` then optimises the AST at the usual level.
- The crate expands an unknown group reference in a replacement string to nothing; REX requires a throw, so replacement strings are parsed (winnow) and checked against the pattern's groups before replacing.
- Byte offsets convert to character positions incrementally across one method call, so `find_all` stays linear in conversion.
- A persisted `Regex` param is stored as its pattern text with a type tag beside the version tags (OPS#persistence).

## Checklist

### Limits

- [ ] `cargo add regex-automata winnow` to `crates/core`; enable Rhai's `internals` feature.
- [ ] `ScriptLimits` and `HelperLimits`: `max_regex_compiled_size` (33 554 432).
- [ ] Daemon option `--script-max-regex-compiled-size`, with the 0-means-unlimited test extended.
- [ ] `runtime::seeds`: the per-frame compiled-size counter, reset by both seed frames.

### Engine and BSL surface

- [ ] `crates/core/src/defs/regex.rs`: the `Regex` type (pattern text, compiled meta regex, compiled size), the budget, the cache, compiling under the budget.
- [ ] `std::regex`, registered in the std module.
- [ ] Methods `is_match`, `find`, `find_all`, `captures`, `captures_all`, `split` (both forms), `replace`, `replace_all`, with match and capture maps in character positions.
- [ ] Replacement string parser and validation; closure replacements.
- [ ] Conversion to a string as the pattern.

### Param kind

- [ ] `ParamKind::Regex`, `ParsedValue::Regex`; `ParamKind::parse` takes a budget at every call site.
- [ ] `runtime/param_map.rs`: store a `Regex` as its text with a type tag, and restore it.
- [ ] Web UI `ParamFields`: single-line text input for `regex`.

### Literal-pattern check

- [ ] Engine compiles with optimisation off; `defs::compile` parses, walks for `std::regex`/`std::parse_regex` with a string-constant argument, checks each against a fresh budget, then optimises.
- [ ] Every `engine.compile(...)` call site goes through `defs::compile`.

### Tests and docs

- [ ] `crates/core/src/tests/regex.rs`: compiling, searching, positions, captures, methods, split, replacements, compiled size, literal check.
- [ ] Param kind tests: refusal when set and as a default; persistence round trip; `tests/stdlib.rs` helper list.
- [ ] `docs/bsl-scripting.md`: regular expressions, the `regex` kind, the limit.
