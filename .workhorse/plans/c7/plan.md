# Digests, encodings and password hashes

Implements the helpers in the digests and encodings spec (ENC) and the password hashes in RND, the password hash limit and the Blob counting of LIM, and the withheld-result rule in SEC#redaction.
The BSL guide (`docs/bsl-scripting.md`) already documents the helpers.

## Notes

### Blob length limit

Rhai 1.25.1 counts a Blob's bytes against `max_array_size` after every native call (`eval/data_check.rs`) and in its Blob built-ins (`packages/blob_basic.rs`), so a separate Blob limit would need a patched Rhai.
Decided instead: a Blob's length counts against the array length limit, and LIM says so; no new option.

### Withheld results

Whether a parameter is secret is only known once evaluation ends, because `secret(...)` can be set after `value()` has been read.
So a helper can't decide withholding when it is called:

- `param.value()` records each value it returns in the seed frame, keyed by parameter name.
- When a helper is called, it checks its string and Blob arguments against three things: the derived values, the parameter values read so far, and the candidates recorded so far.
  If any argument matches, the helper records its result's forms as a candidate.
  The candidate depends on the union of what matched: "derived" and/or a set of parameter names.
  This dependency carries through to a helper given an earlier candidate's result.
- When the frame ends, a candidate becomes a derived value if it depends on "derived" or on any parameter that is now effectively secret.
  Each one that does counts toward the derived values limit, and going over that limit fails the evaluation.
- Empty forms and `File`/`Directory` arguments never match.
- Only the static and validator contexts record, as `record` does today.

### Helper limits through the engine tag

Today the engine's default tag carries only the derived values limit.
It becomes a small `Clone` struct holding both the derived values limit and the password hash limit.
The password hash count is a second counter in the seed frames, kept beside `DERIVATIONS`.

### Shared field encoding

`Field` and `encode` move out of `random.rs` and gain `Field::Bytes`: a u64 big-endian length, then the bytes.
The directory encoding and the password helpers' call encoding both use them.

## Checklist

### Groundwork

- [x] Run `cargo clippy`, `cargo fmt --check` and both spec checks, and fix anything outstanding.
- [x] Add direct dependencies with `cargo add`, at full x.y.z versions:
  - `blake2` and `digest` (check which digest generation `sha2` 0.11 uses; use a `blake2` release on the same one if there is one);
  - `blake3`, `hex`, `bcrypt`;
  - `argon2` as a workspace dependency of `seedling-core`.
- [x] Move `Field`/`encode` from `defs/stdlib/random.rs` into `defs/stdlib/fields.rs`, and add `Field::Bytes`.
- [x] Add `take_byte_arg(what, Dynamic)` in `defs/stdlib/bytes.rs` (only the std helpers take bytes), returning the bytes of a string, Blob, `File` or `Directory` (the last through the directory encoding).
  Any other type gets the usual "must be …" error. Cite ENC#byte-arguments and ENC#directory-encoding.
- [x] Add a check helper that fails with `ErrorDataTooLarge` when a result would exceed the string or array limit (ENC#results).
  Encoders check before allocating; decoders check what they decoded, which is no larger than their string argument.

### Limits

- [x] Password hash limit: `ScriptLimits.max_password_hashes`, `--script-max-password-hashes` (default 16, 0 removes it), and its `From` conversion and parse test.
- [x] The engine default tag carries both helper limits. Update `seed_for_call` and `setup_language` to match.
- [x] Add a password hash counter to the seed frames (`runtime/seeds.rs`), reset on entry and restored on drop.

### Helpers (new `defs/stdlib/encoding.rs`, registered in `stdlib.rs`)

- [x] `encode_hex`/`decode_hex`: lowercase output; decoding accepts either case and throws on odd length or a non-digit.
- [x] `encode_base64`/`decode_base64`: padded and canonical.
  Throws on a bad character, a length that isn't a multiple of four, or non-zero trailing bits.
- [x] `encode_base64url`/`decode_base64url`: unpadded.
  Throws on any padding, on length mod 4 = 1, or on non-zero trailing bits.
- [x] `encode_utf8`/`decode_utf8`: `decode_utf8` takes `bytes` and throws on invalid UTF-8.
- [x] Empty input returns an empty string or Blob for all eight helpers.
- [x] `digest_sha256`, `digest_sha512`, `digest_blake2b` (64 bytes, unkeyed) and `digest_blake3` (32 bytes), each returning a Blob.
- [x] `hmac_sha256` and `hmac_sha512`, accepting a key of any length.
- [x] `uuid_v5(namespace, name)`.
  `namespace` must be exactly the 36-character hyphenated form (the uuid crate's parser also accepts simple, braced and URN forms, so check the shape first).
  Either case is accepted, and the output is lowercase hyphenated.

### Password hashes (in `defs/stdlib/random.rs`)

- [x] Call encoding: `[Str(name), Str(label), Bytes(password)]`, with the salt taken as the first 16 bytes of the byte stream (RND#call-encoding, RND#values).
- [x] `password_argon2id`: Argon2id v19, m=19456, t=2, p=1, 32-byte tag, no secret or associated data.
  Written as `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<tag>`, both parts in unpadded standard base64.
- [x] `password_bcrypt`: cost 12 under the derived salt, written as `$2b$12$` followed by 22 salt characters and 31 hash characters.
  Throws on a password over 72 bytes or holding a zero byte, before hashing.
- [x] Both helpers throw on an empty label or an empty password, count toward the derived values and password hash limits, and record the hash as a derived value.

### Redaction

- [x] `blob_forms` also records the Blob's raw bytes and its contiguous lowercase hex, base64 and base64url forms (SEC#redaction: every text encoding BSL offers).
  The raw bytes are what a Blob argument is searched for: random bytes are seldom valid UTF-8, so the UTF-8 reading alone would miss them.
- [x] Add `found_in_bytes` to `DerivedValues`, and have `found_in` delegate to it, so Blob arguments can be tested.
- [x] Have `param.value()` record each value it returns in the app seed frame (static and validator contexts only).
- [x] Add withheld-candidate tracking to the seed frame, as the note above describes.
  Wire it into every ENC helper, and resolve the candidates against the effective secret flags when the frame ends.
  Each candidate that becomes withheld counts toward the derived values limit.
  Resolution needs the final secret flags, so `run_definition` passes them to `take_derived`.
- [x] Check that `oi/handler/apps.rs` env redaction picks up the resolved candidates through `def.derived`, with no change there.

### Tests

- [x] `defs/stdlib/encoding/tests.rs` (unit tests), each citing ENC:
  - RFC 4648 §10 vectors for base16, base64 and base64url;
  - FIPS 180-4 vectors for SHA-256 and SHA-512, RFC 7693 Appendix A for BLAKE2b, and the official BLAKE3 test vectors;
  - RFC 4231 vectors for HMAC-SHA-256/512, including a key longer than the block size;
  - the RFC 9562 Appendix A UUID v5 example;
  - a known answer for the directory encoding.
- [x] Decoder strictness: whitespace, wrong or missing padding, padding in base64url, non-zero trailing bits, odd-length hex, invalid UTF-8.
- [x] Password hash known answers in `random/tests.rs`, cross-checked with the Python recipe the file already uses (extended with `argon2-cffi` and `bcrypt` at an explicit salt).
  Also: the salt changes with the password, the 72-byte and zero-byte refusals, and an empty password.
- [x] Script-level tests in `crates/core/src/tests/stdlib.rs`:
  - each helper is reachable as `std::…`;
  - bytes arguments accept a string, Blob, `app.file` and `app.dir`, and refuse an int;
  - results over the string or array limit fail before allocating;
  - the password hash limit is enforced and counts toward the derived values limit too.
- [x] Redaction tests:
  - a basic-auth env value built from a derived password is withheld;
  - one built from a secret parameter is withheld, including when `secret(true)` comes after `value()`;
  - one built from a non-secret parameter is shown;
  - an empty secret value withholds nothing;
  - a `File` argument withholds nothing;
  - a chain of helpers stays withheld;
  - withheld results count toward the derived values limit;
  - a derived Blob shows withheld in its hex and base64 forms.
- [x] Tests for the password hash limit and for 0 removing it: at script level in `tests/stdlib.rs` and as a daemon option parse test.

### Finish

- [x] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`.
