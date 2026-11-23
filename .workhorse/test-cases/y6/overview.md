# Pair every parameter kind with a parse function

## Parsers

- [x] Every kind resolves to `std::parse_<kind>`, with `-` as `_` (`std::parse_weak_password`) (verifies spec: ACN)
- [x] A non-string argument raises the argument-rules error (verifies spec: BSL)
- [x] `parse_text`, `parse_email`, `parse_password`, `parse_weak_password` and `parse_random` refuse LF, CR, tab, NUL, ESC, DEL, U+0085, U+2028 and U+2029, naming the code point (verifies spec: ACN)
- [x] `parse_text` accepts accented letters, emoji and the empty string (verifies spec: ACN)
- [x] `parse_multiline` accepts line breaks and control characters (verifies spec: ACN)
- [x] `parse_email("a@b@example.org")` returns `local` `a` and `domain` `b@example.org`; `@x.org`, `a@b`, `a@.b`, `a@b.` and `ab` are refused (verifies spec: ACN)
- [x] `parse_password` refuses `password` and accepts a strong passphrase; `parse_weak_password` accepts `a` (verifies spec: ACN)
- [x] `parse_volume` accepts `backups` and refuses `ab`, `-abc` and `abc-` (verifies spec: ACN)
- [x] Parsers are callable at the top level, in an action closure and in a validator (verifies spec: STD)

## Invocations

- [x] An action `text` param with a line break is refused, naming the field and the reason; `multiline` takes it (verifies spec: ACN)
- [x] An empty optional `email` action param is absent, not refused (verifies spec: ACN)

## Stored parameters

- [x] `/apps/params/set` refuses an `email` parameter set to `not-an-email` or `""` with `validation_failed`, storing nothing; unsetting with `null` passes (verifies spec: PARAM, PRM)
- [x] A validator does not run on a value its kind refuses (verifies spec: PARAM)
- [x] A validator that lets `std::parse_email` throw rejects with the parser's reason (verifies spec: PARAM)
- [x] `/apps/update` changing a stored parameter's kind from `text` to `email` is refused when the stored value is not an address (verifies spec: PRM)
- [x] `/apps/plan` lists a kind failure under `rejections` and changes nothing (verifies spec: PRM)
- [ ] In the console, a `text` parameter rejected for a pasted line break shows the parser's reason on save
