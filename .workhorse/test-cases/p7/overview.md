# Structured formats test cases

## Encoding

- [ ] JSON of a map is indented by two spaces, keys sorted, ending with a line feed (verifies spec: FMT#json)
- [ ] JSON takes a scalar, an empty array and an empty map, and escapes only what JSON requires (verifies spec: FMT#json)
- [ ] A Version is written as its text (verifies spec: FMT#values-each-format-holds)
- [ ] A refused value names its position and the reason, for each format (verifies spec: FMT#refusal)
- [ ] TOML and env encoders refuse an argument that is not a map (verifies spec: FMT#helpers)
- [ ] 128 levels of nesting encode; 129 throw (verifies spec: FMT#helpers)
- [ ] YAML is block style, two-space indent, `null` for unit, `-.inf` for negative infinity (verifies spec: FMT#yaml)
- [ ] YAML quotes the 1.1 words, numeric-looking strings, indicator-led strings and strings with edge whitespace, and each round-trips (verifies spec: FMT#yaml)
- [ ] YAML quotes map keys by the same rules (verifies spec: FMT#yaml)
- [ ] YAML escapes C1 controls and U+FFFE/U+FFFF (verifies spec: FMT#yaml)
- [ ] TOML writes key/value lines before tables, arrays of maps as `[[array]]`, an empty array and mixed arrays inline (verifies spec: FMT#toml)
- [ ] TOML strings escape only what TOML 1.0 requires, with no 1.1-only escapes (verifies spec: FMT#toml)
- [ ] TOML output decodes back to the value it was written from (verifies spec: FMT#toml)
- [ ] Env files single-quote every value; an empty map gives an empty string (verifies spec: FMT#env-files)
- [ ] Env files refuse a value holding `'`, LF, CR or NUL, and a key outside the key form (verifies spec: FMT#env-files)
- [ ] Rhai's `to_json` throws naming `std::encode_json`, as a method, a function and through `Fn` (verifies spec: FMT#json)

## Decoding

- [ ] Each decoder maps values by the table, including `-0` as an integer and TOML date-times as strings with `T` (verifies spec: FMT#decoding)
- [ ] Integers outside 64 bits and numbers beyond the float range throw, in JSON, YAML and TOML (verifies spec: FMT#decoding)
- [ ] A key given twice throws, comparing decoded strings (verifies spec: FMT#decoding)
- [ ] A failure names the line and never quotes the text; invalid UTF-8 throws (verifies spec: FMT#decoding)
- [ ] A leading byte order mark is ignored (verifies spec: FMT#decoding)
- [ ] Env files decode their strict form and throw on anything else, naming the line; a key named twice throws (verifies spec: FMT#env-files)
- [ ] Env file output decodes back to the map it was written from (verifies spec: FMT#env-files)
- [ ] YAML holding no document or more than one throws (verifies spec: FMT#yaml-documents)
- [ ] YAML core tags apply; a custom tag or a mismatched tag throws (verifies spec: FMT#yaml-documents)
- [ ] A non-string YAML key throws (verifies spec: FMT#yaml-documents)
- [ ] YAML aliases copy their anchor; an alias bomb, an unknown alias and a self-referencing alias throw (verifies spec: FMT#yaml-documents)
- [ ] Decoders take a Blob, and refuse a non-bytes argument (verifies spec: FMT#helpers)
- [ ] Results beyond the engine's string, array and map limits throw (verifies spec: FMT#helpers)

## Redaction

- [ ] An encoding of a value holding a secret parameter is withheld; one holding a non-secret parameter is not (verifies spec: SEC#redaction)
- [ ] A string decoded from a secret is withheld; a decoded number is not (verifies spec: SEC#redaction)
- [ ] Each distinct non-empty decoded string counts toward the derived values limit (verifies spec: SEC#redaction)
