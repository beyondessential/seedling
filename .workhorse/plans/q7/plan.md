# Quoting for shells and URLs

Adds `std::quote_shell` (spec QUOT) and `std::encode_percent` / `std::decode_percent` (spec ENC) to the `std` module, and withholds their results like the other encoding helpers (spec SEC).

## Notes

- **Shell quoting is written by hand, not taken from a crate.** QUOT fixes the exact output: an allow-list of unquoted characters, otherwise the whole string in single quotes with `'\''` for each `'`. `shlex` 2.0.1 mixes single- and double-quoted chunks and its allow-list differs (`,` and `%` quoted, `]` not); `shell-words` 1.1.1 quotes from a deny-list, so `{`, `!` and control characters pass unquoted. Neither can meet the spec, and the function is a few lines.
- **`%` stays in the safe set.** POSIX lists it among characters that "may need to be quoted", but only builtins such as `fg` and `kill` give it a meaning, and they read it from the word's value, which quoting does not change.
- **Percent-encoding uses `percent-encoding`**, already a dependency of `core` at 2.3.2: an `AsciiSet` of `NON_ALPHANUMERIC` with `-`, `.`, `_` and `~` removed. The crate writes uppercase hex digits, as ENC requires. Its decoder passes a malformed `%` through unchanged, so `decode_percent` checks that every `%` begins two hex digits before decoding.
- **Results go through the existing redaction path.** `text_result` and `settle` in `stdlib/encoding.rs` already withhold a helper's result when an argument holds a withheld value. `quote_shell` passes each string it quotes as an argument, so a withheld value in any array element withholds the result.

## Shared result helpers

- [x] Move `text_result`, `blob_result`, `settle`, `max_string`, `max_blob`, `unary` and `HelperResult` from `crates/core/src/defs/stdlib/encoding.rs` to `crates/core/src/defs/stdlib.rs`, where the child modules reach them, updating every call site in `encoding.rs`.
- [x] Re-cite the moved result helpers to both `ENC#results` and `QUOT#results`.

## Percent-encoding

- [x] Add `encode_percent` to `encoding::register` in `crates/core/src/defs/stdlib/encoding.rs`: take a byte argument, check the worst-case length (three bytes out per byte in) against the string limit before encoding, encode with the unreserved set, and return through `text_result`. Cite `ENC#encodings`.
- [x] Add `decode_percent`: take `text`, refuse a `%` not followed by two hex digits with a `not_encoded("percent-encoding", …)` error naming its position, decode, and return through `blob_result`. Cite `ENC#encodings`.
- [x] Add the percent-encoded form to `blob_forms` in `crates/core/src/defs/stdlib.rs`, since SEC withholds a Blob derived value in every text encoding BSL offers.
- [x] Tests in `crates/core/src/defs/stdlib/encoding/tests.rs`:
  - every unreserved character passes through, and every other ASCII byte and a non-ASCII byte are encoded with uppercase digits; a space is `%20`
  - `decode_percent` reads `%2f` and `%2F` alike, leaves `+`, `=`, `&`, `:` and `!` as they are, and returns a Blob
  - `%`, `%4`, `%zz` and a trailing `%` throw, naming the position but not the text
  - an empty argument gives an empty string or Blob
  - a result beyond the string limit fails
  - round trip: `decode_percent(encode_percent(b))` is `b` for arbitrary bytes

## Shell quoting

- [x] Add `crates/core/src/defs/stdlib/quoting.rs` with `register`, called from `stdlib::module`.
- [x] Register `quote_shell`, dispatching on its argument: a string, or an array taken with `take_string_array("words", …)` so a non-string element throws. Mark the string-or-array dispatch `// take: dispatch`, ending in a throw that names both accepted types. Cite `QUOT#shell-words`.
- [x] Implement the quoting of one string: throw on NUL without echoing the string, return it unchanged when non-empty and all in the safe set, otherwise wrap in `'…'` with `'\''` for each `'`. Join array elements with single spaces; an empty array gives `""`.
- [x] Return through `text_result` with each quoted string as a `ByteArg::text`, checking the result against the string limit. Cite `QUOT#results`.
- [x] Tests in `crates/core/src/defs/stdlib/quoting/tests.rs`:
  - each safe character and a mix of them comes back unchanged
  - `=`, `~`, space, `*`, `$`, `` ` ``, `"`, `\`, `#`, `;`, `!`, `{`, newline and a non-ASCII character each cause quoting
  - `""` gives `''`; `"it's"` gives `'it'\''s'`
  - an array gives space-joined words; `[]` gives `""`; `["a", 1]` throws naming the element's type
  - a NUL character throws without echoing the argument
  - a non-string, non-array argument throws naming both accepted types
  - round trip through a real `sh -c 'printf "%s\0" "$@"' sh <quoted>` for a set of awkward strings, reading back each argument exactly; skipped where `/bin/sh` is absent

## Redaction

- [x] Add to `an_env_value_containing_a_derived_value_is_null` in `crates/core/src/oi/handler/apps/tests.rs`: an env value built from `std::quote_shell(["--pw", pw])`, from `std::encode_percent(pw)` and from `std::encode_percent(raw)` reads as `null`, and one from `std::quote_shell("plain")` does not.

## Docs

- [x] `docs/bsl-scripting.md`, *Digests and encodings*: add `encode_percent` / `decode_percent` to the table, and say under the strict-decoder paragraph that `decode_percent` takes a URL component as a URL carries it.
- [x] `docs/bsl-scripting.md`: add a *Quoting* subsection under *Standard helpers* for `std::quote_shell`, with an `sh -c` example built from an array and a line on `--` before a value that may begin with `-`.

## Finish

- [x] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh` and `etc/ci/check-defs-coercion.sh` clean.
