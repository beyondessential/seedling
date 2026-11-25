# Structured format helpers

## Crates

Every format is implemented on an existing crate; Seedling writes no YAML, TOML or JSON parser of its own.
Add each with `cargo add` at implementation time so the pinned version is the current one.

- JSON: `serde_json`, already a dependency, writes. `hifijson` reads: it gives each number's own text and whether it is an integer, where serde_json reads an integer too large for 64 bits, and `-0`, as a float. It bounds depth, keeps duplicate keys for the decoder to refuse, and has no dependencies.
- TOML: `toml`, already a dependency, writing and reading. It writes TOML 1.0 for every value BSL lowers to, and reads TOML 1.1, refusing duplicate keys, out-of-range integers and overflowing floats itself.
  Seedling's tests pin its layout, so a change to it in a later version of the crate is caught there.
- YAML: `saphyr` and `saphyr-parser` 0.3.4 (YAML 1.2). 0.4 was under a week old, inside the local cargo minimum publish age.
  The emitter writes; it already quotes the YAML 1.1 boolean and null words and numeric-looking strings, and leaves `y`/`n` bare. Strings holding C1 controls or U+FFFE/U+FFFF are escaped and double-quoted before reaching it, since it would write them bare.
  The reader is a small loader over saphyr-parser's events, resolving the core schema itself so that tags, out-of-range numbers, duplicate and non-string keys, the alias budget and nesting depth are all checked where they occur, with the line.
  Seedling's own tests pin the quoting of the important cases, so a change of dependency cannot change the output unnoticed.
- Env files: no crate fits. `dotenvy` expands `$VAR` from the process environment, which breaks std helper replicability and leaks the daemon's environment; the other candidates are unmaintained.
  Seedling writes its own: a small winnow parser for the strict grammar (bare or single-quoted values, blank lines, `#` comment lines), and an encoder that single-quotes every value.

## Steps

- [x] `cargo add saphyr` (and winnow, if not already a dependency of the crate holding the std module).
- [x] Shared value walk: position tracking for refusal errors, the 64-deep nesting limit, `Version`/`SemVer` as text, Blob and other opaque types refused.
- [x] JSON encode on `serde_json` and decode on `hifijson`, with integers outside the signed 64-bit range refused rather than read as floats (unlike `defs/json.rs::to_dynamic`).
- [x] YAML encode and decode on `saphyr`: core schema only, one document, alias budget of 100 000, non-string keys refused; tests pinning the quoting cases in the spec.
- [x] TOML encode and decode on `toml`, checking the encoder's output parses as TOML 1.0.0 (no 1.1-only syntax).
- [x] Env encoder and winnow decoder.
- [x] Override Rhai's map `to_json` in `defs.rs` with a function that throws naming `std::encode_json`, as `sleep` is overridden. Removal is not possible: `disable_symbol` acts only on keyword and operator tokens, and `to_json` is part of Rhai's basic map package. Test that both `m.to_json()` and `Fn("to_json").call(m)` throw.
- [x] Redaction: withhold encoder results like ENC helpers; for decoder results, withhold each string within, keys included, counting each toward the derived values limit.
- [x] Update `docs/bsl-scripting.md`.

## Explicitly rejected

- INI encoder and decoder. INI has no specification, and dialects disagree on quoting, escapes and inline comments, so the same file means different things to different readers and no refusal rule makes it safe.
