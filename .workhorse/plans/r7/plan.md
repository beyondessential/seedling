# Regular expressions

## Technical notes

- Engine: the `regex` crate (already in the dependency tree via oci-client and zxcvbn). Ruled out after a survey: `regex-lite` (no Unicode classes), `fancy-regex` (backtracking under a step budget, wanted only for lookaround and backreferences), `regress` (unbounded backtracking), `resharp` (no captures, non-standard syntax), `pcre2` (wrapper exposes no match limit), `onig` (C dependency).
- The crate bounds a single search to O(pattern x text) but iterating over every match to O(pattern x text^2); see its "Iterating over matches" docs. The text-examined limit (REX#text-examined) closes that gap.
- Counting the bytes a search reads means driving the lower-level automata in `regex-automata` rather than calling `regex::Regex`'s search methods, which report no scan length. Captures then run over the matched span only.
- A helper cannot add to Rhai's operation count (`GlobalRuntimeState` is read-only to native functions, and only under `internals`), so the count is kept per evaluation frame as `runtime::seeds` keeps the derived-values and password-hash counts, with its limit carried in `HelperLimits`.
- The crate expands an unknown group reference in a replacement string to nothing; REX requires a throw, so replacement strings are parsed and checked against the pattern's groups before replacing.
- Compiled patterns are cached by pattern text across evaluations, in a bounded cache shared by the daemon's engines, so a pattern written into a script or recurring in a parameter value compiles once. A miss compiles and throws exactly as an uncached call would, so the cache is invisible to scripts; the text-examined count is per search and unaffected.
- The literal-pattern check (REX#compiling-a-pattern) walks the compiled script for `std::regex` calls whose one argument is a string constant, compiles each (warming the cache) and fails with the pattern's error. `AST::walk` needs Rhai's `internals` feature, which is not enabled today.
- Rhai's default optimisation propagates `const` values into call arguments, so a walk over the optimised AST would also refuse `const P = "("; std::regex(P)`, which REX does not. Walk an AST parsed with optimisation off, then optimise, or otherwise tell a written literal from a propagated one.
