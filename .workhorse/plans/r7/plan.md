# Regular expressions

Adds `std::regex`, the `Regex` type, the `regex` param kind and its parser, the two regex engine limits, and the definition-time refusal of malformed literal patterns, per the Regular expressions spec (REX).

## Technical notes

- Engine: the `regex` crate (already in the dependency tree via oci-client and zxcvbn). Ruled out after a survey: `regex-lite` (no Unicode classes), `fancy-regex` (backtracking under a step budget, wanted only for lookaround and backreferences), `regress` (unbounded backtracking), `resharp` (no captures, non-standard syntax), `pcre2` (wrapper exposes no match limit), `onig` (C dependency).
- The crate bounds a single search to O(pattern x text) but iterating over every match to O(pattern x text^2); see its "Iterating over matches" docs. The text-examined limit (REX#text-examined) closes that gap.
- Counting the bytes a search reads means driving the lower-level automata in `regex-automata` rather than calling `regex::Regex`'s search methods, which report no scan length. Captures then run over the matched span only.
- A helper cannot add to Rhai's operation count (`GlobalRuntimeState` is read-only to native functions, and only under `internals`), so the count is kept per evaluation frame as `runtime::seeds` keeps the derived-values and password-hash counts, with its limit carried in `HelperLimits`.
- The crate expands an unknown group reference in a replacement string to nothing; REX requires a throw, so replacement strings are parsed and checked against the pattern's groups before replacing.
- Compiled patterns are cached by pattern text across evaluations, in a bounded cache shared by the daemon's engines, so a pattern written into a script or recurring in a parameter value compiles once. A miss compiles and throws exactly as an uncached call would, so the cache is invisible to scripts; the text-examined count is per search and unaffected.
- The literal-pattern check (REX#compiling-a-pattern) walks the compiled script for `std::regex` and `std::parse_regex` calls whose one argument is a string constant, compiles each (warming the cache) and fails with the pattern's error. `AST::walk` needs Rhai's `internals` feature, which is not enabled today.
- Rhai's default optimisation propagates `const` values into call arguments, so a walk over the optimised AST would also refuse `const P = "("; std::regex(P)`, which REX does not. Walk an AST parsed with optimisation off, then optimise, or otherwise tell a written literal from a propagated one.
- Compiled size (REX#compiled-size): build each pattern with the engine's size limit set to what remains of the budget, so an oversized pattern aborts early, and count the compiled object's reported memory usage. The cache stores that size beside the compiled pattern so a hit counts exactly what a miss would. A refusal zeroes the remainder. The budget is kept in the same regex frame as the text-examined count.
- Searching drives the lazy DFA (`regex_automata::hybrid`) byte by byte through `next_state`, so every byte read is counted exactly: a forward search to find a match's end, then a reverse search from that end to find its start. Captures run anchored over `[start, end)` only (one-pass DFA where the pattern allows, else PikeVM), counting `end - start`. Configure the lazy DFA never to give up on cache thrash, so the count does not depend on cache history.
- The lazy DFA quits on a non-ASCII byte when the pattern holds a Unicode word boundary (`\b`, the default form). Those searches need another counted path; see the spike below.
- Per-search caches (`hybrid::regex::Cache`, PikeVM cache) belong to the evaluation, never to the shared pattern cache, so one evaluation's searches cannot change another's.
- Byte offsets convert to character positions incrementally across one method call (carry the last byte offset and its character count forward), or `find_all` becomes quadratic in conversion alone.
- Counts live in a thread-local regex frame in `runtime::seeds`: compiled size remaining and text examined so far. `AppSeedFrame` and `OperationSeedFrame` carry one, and every other evaluation site enters a bare `RegexFrame` with the configured limits. A helper called with no frame throws, as derivation helpers do without a seed, so a site that was missed fails loudly rather than running unbounded.
- `ParamKind::parse` runs both inside evaluation (`std::parse_regex`, schema defaults) and outside it (`runtime::apps` parameter checks, `runtime::action_params` invocation checks). The outside callers enter a `RegexFrame` with the daemon's limits around the parse.
- A param map is persisted as JSON (OPS#persistence). A `Regex` param is stored as its pattern text with a type tag beside the version tags, and recompiled when the map is restored for a closure, counting toward that evaluation's budget.

## Checklist

### Dependencies and limits

- [ ] `cargo add regex-automata` and `winnow` (for the replacement-string parser) to `crates/core`, plus `regex-syntax` if its group-name rules are needed directly, pinned to full versions; enable Rhai's `internals` feature.
- [ ] `ScriptLimits` in `crates/core/src/lib.rs`: add `max_regex_compiled_size` (33 554 432) and `max_regex_text_examined` (67 108 864); carry both in `HelperLimits`.
- [ ] `crates/daemon/src/main.rs`: `--script-max-regex-compiled-size` and `--script-max-regex-bytes`, wired into `From<ScriptLimitArgs>`, with the 0-means-unlimited test extended.
- [ ] `runtime::seeds`: the regex frame (remaining compiled size, text examined), its counting functions, carried by `AppSeedFrame` and `OperationSeedFrame`, and a standalone `RegexFrame` for other sites.
- [ ] Audit every `run_ast_with_scope`, `eval_ast_with_scope` and closure-call site (`defs/app.rs`, `runtime/apps.rs`, `runtime/probe.rs`, `runtime/barrier/replay.rs`, `oi/handler/images.rs`, `oi/handler/actions/lifecycle.rs`, `oi/shells/session.rs`, the validator frame) and make sure each enters a regex frame.

### Engine wrapper

- [ ] Spike: a counted search for patterns the lazy DFA quits on. Candidates: drive the PikeVM over a growing window and accept its result only when the window was not the limiting factor; or restrict counting to the bounded backtracker where the visited set fits. Record the outcome here before building on it.
- [ ] New module `crates/core/src/defs/regex.rs` with an internal compiled-pattern type: forward and reverse lazy DFAs, the capture engine, group names, pattern text, and compiled size from the engines' reported memory usage.
- [ ] Compile under the remaining budget (NFA size limit set to the remainder); on refusal zero the remainder and throw (REX#compiled-size). A syntax error throws with the parser's message.
- [ ] Bounded cache of compiled patterns keyed by pattern text, storing the compiled size, so a hit counts what a miss would.
- [ ] Counted search primitive returning the match span and charging every byte read (forward, reverse, captures) to the frame; over the limit raises `ErrorDataTooLarge`, as `stdlib::within` does.
- [ ] Iteration: successive searches with the empty-match skip of REX#searching, advancing one character on a skipped empty match.

### BSL surface

- [ ] `Regex` as a Rhai `CustomType` registered in `defs::register`, with `PartialEq`/`Eq` by pattern text (needed by `ParsedValue`), and conversion to a string as its pattern (REX#compiling-a-pattern).
- [ ] `std::regex(pattern)` in `defs/stdlib.rs`'s module, taking its argument through `defs::take`.
- [ ] Methods `is_match`, `find`, `find_all`, `captures`, `captures_all`, with match maps `{text, start, end}` in character positions and capture maps `{groups, named}` with `()` for a group that took no part.
- [ ] `split(text)` and `split(text, limit)`, keeping empty pieces and throwing on a limit below 1.
- [ ] Replacement string parser in winnow (`$$`, `${ref}`, `$name`), validated against the pattern's groups before any search, so an invalid string throws even when nothing matches; a digit-only ref is a group number, 0 the whole match.
- [ ] `replace` and `replace_all` with a string or a closure; a closure receives each match's captures and must return a string, inserted unexpanded; any other replacement value is a script error.

### Param kind

- [ ] `ParamKindName::Regex` and `ParamKind::Regex` in `defs/param_def.rs`; `ParsedValue::Regex`, whose `into_dynamic` gives the `Regex`. `std::parse_regex` then comes from the kind-parser loop in `defs/stdlib/kinds.rs`.
- [ ] Enter a regex frame around `parse_supplied` in `runtime/apps.rs` and `runtime/action_params.rs`.
- [ ] `runtime/param_map.rs`: store a `Regex` as pattern text with a type tag, and recompile it in `to_rhai`.
- [ ] Web UI: `ParamFields` offers the single-line text input for `regex`, with its test extended.

### Literal-pattern check

- [ ] A single `defs::compile(engine, scope, source)` that parses with optimisation off, walks for `std::regex`/`std::parse_regex` calls whose one argument is `Expr::StringConstant`, compiles each against a fresh budget (warming the cache), fails with the pattern's error, then optimises the AST at the engine's usual level.
- [ ] Replace every `engine.compile(...)` call site (`runtime/apps.rs`, `runtime/probe.rs`, `runtime/barrier/replay.rs`, `oi/handler/images.rs`, `oi/handler/actions/lifecycle.rs`, `oi/shells/session.rs`, `lib.rs` tests) with it, so no path skips the check.

### Tests

- [ ] `crates/core/src/tests/regex.rs`: compile errors and size refusal, including the remainder being used up after a refused pattern; leftmost-first search; the `b*`-on-`abba` and `|a`-on-`a` empty-match cases; character positions over multibyte text; captures with a non-participating group; every method's shape; `split` limits; replacement expansion and every throwing form; closures returning a non-string.
- [ ] Limits: a `find_all` over a long text tripping the text-examined limit; a loop compiling distinct patterns tripping the compiled-size limit; both removed by 0; a cache hit counting the same as a miss.
- [ ] Literal check: a malformed literal in a never-called function fails evaluation; a `const` or interpolated pattern does not; a literal oversized pattern fails.
- [ ] Param kind: a bad pattern refused when set and as a `default_value`; a `Regex` param surviving persistence and replay; `tests/stdlib.rs`'s helper list gains `regex` and `parse_regex`.

### Docs

- [ ] `docs/bsl-scripting.md`: a regular expressions section, `regex` in the kind list, and the two limits.
