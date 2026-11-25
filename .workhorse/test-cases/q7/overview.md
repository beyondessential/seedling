# Quoting for shells and URLs

## Shell quoting

- [x] A word of only ASCII letters, digits and `_ - . / : , + @ %` is returned unchanged (verifies spec: QUOT)
- [x] `=`, `~`, whitespace, shell metacharacters and non-ASCII characters cause single-quoting (verifies spec: QUOT)
- [x] `""` gives `''`; each `'` is written as `'\''` (verifies spec: QUOT)
- [x] An array gives one quoted word per element joined by spaces; `[]` gives an empty string (verifies spec: QUOT)
- [x] A non-string element, or an argument neither string nor array, throws naming the type (verifies spec: QUOT)
- [x] A NUL character throws without echoing the word (verifies spec: QUOT, SEC)
- [x] A result over the string limit fails (verifies spec: QUOT)
- [x] `/bin/sh` reads back every awkward word exactly, including empty, quotes, newlines, `$`, backticks, globs, `~` and a leading `-` (verifies spec: QUOT)

## Percent-encoding

- [x] Unreserved characters pass through; every other byte is `%XX` in uppercase, space as `%20` (verifies spec: ENC)
- [x] `decode_percent` takes either case of hex digit, leaves `+ = & : !` and non-ASCII text as their UTF-8 bytes, and returns a Blob (verifies spec: ENC)
- [x] A `%` not followed by two hex digits throws, naming its position but not the text (verifies spec: ENC, SEC)
- [x] Every byte value round-trips through encode then decode (verifies spec: ENC)
- [x] A result over the string limit fails (verifies spec: ENC)

## Redaction

- [x] A quoted array or a percent-encoded string or Blob built from a derived value reads as `null` in the app's description; a quoted plain word does not (verifies spec: SEC)
- [x] A Blob is withheld in its percent-encoded form (verifies spec: SEC)
