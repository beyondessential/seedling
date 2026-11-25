# Structured format helpers

## Crates

Every format is implemented on an existing crate; Seedling writes no YAML, TOML, JSON or INI parser of its own.
Add each with `cargo add` at implementation time so the pinned version is the current one.

- JSON: `serde_json`, already a dependency.
- TOML: `toml`, already a dependency. It implements TOML 1.1 while the overview spec cites TOML 1.0.0, so the encoder's output and the decoder's accepted input need checking against 1.0.
- YAML: `saphyr` (YAML 1.2, passes the YAML test suite). Its mapping type is ordered, so sorted BSL keys survive to the output. Its emitter already quotes the YAML 1.1 boolean and null words and numeric-looking strings; leaves `y`/`n` bare.
  Convert BSL values straight to `saphyr::Yaml` rather than going through serde.
- INI: `rust-ini` for parsing, with its quote, escape and multi-line options set to match the INI dialect in the spec.
- Env files: no crate fits. `dotenvy` expands `$VAR` from the process environment, which breaks std helper replicability and leaks the daemon's environment; the other candidates are unmaintained.
  The env grammar is strict enough (bare or single-quoted values, blank lines, `#` comment lines) for a small winnow parser; the encoder is a few lines.
