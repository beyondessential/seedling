# Structured format helpers

## Crates

Every format is implemented on an existing crate; Seedling writes no YAML, TOML or JSON parser of its own.
Add each with `cargo add` at implementation time so the pinned version is the current one.

- JSON: `serde_json`, already a dependency.
- TOML: `toml`, already a dependency. It implements TOML 1.1 while the overview spec cites TOML 1.0.0, so the encoder's output and the decoder's accepted input need checking against 1.0.
- YAML: `saphyr` (YAML 1.2, passes the YAML test suite). Its mapping type is ordered, so sorted BSL keys survive to the output. Its emitter already quotes the YAML 1.1 boolean and null words and numeric-looking strings; leaves `y`/`n` bare.
  Convert BSL values straight to `saphyr::Yaml` rather than going through serde.
  Seedling's own tests pin the quoting of the important cases (`no`, `on`, `null`, numeric-looking strings, leading and trailing spaces), so a change of dependency cannot change the output unnoticed.
- Env files: no crate fits. `dotenvy` expands `$VAR` from the process environment, which breaks std helper replicability and leaks the daemon's environment; the other candidates are unmaintained.
  Seedling writes its own: a small winnow parser for the strict grammar (bare or single-quoted values, blank lines, `#` comment lines), and an encoder that single-quotes every value.

## Explicitly rejected

- INI encoder and decoder. INI has no specification, and dialects disagree on quoting, escapes and inline comments, so the same file means different things to different readers and no refusal rule makes it safe.
