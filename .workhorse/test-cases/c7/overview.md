# Digests, encodings and password hashes: test cases

## Encodings

- [x] Encode `""`, `f` … `foobar` to hex, base64 and base64url, and decode each back; the results match RFC 4648 §10, with base64url unpadded (verifies spec: ENC)
- [x] Encode bytes `fb ff`: base64 gives `+/8=`, base64url gives `-_8` (verifies spec: ENC)
- [x] Decode `aBcD` as hex; it re-encodes as `abcd` (verifies spec: ENC)
- [x] Decode odd-length hex, a non-hex digit, padded base64url, unpadded or wrongly padded base64, base64 with a space, non-zero trailing bits, or invalid UTF-8; each throws (verifies spec: ENC)
- [x] Empty input to every encoder and decoder returns an empty string or Blob (verifies spec: ENC)
- [x] `encode_hex` of a string past the string limit, or `encode_utf8` past the array limit, fails with a size error (verifies spec: ENC, LIM)

## Digests, HMAC and UUIDs

- [x] SHA-256, SHA-512, BLAKE2b and BLAKE3 of `abc` (and BLAKE3 of the empty string) match their published vectors (verifies spec: ENC)
- [x] A digest of a string equals the digest of its UTF-8 bytes (verifies spec: ENC)
- [x] HMAC-SHA-256 and HMAC-SHA-512 match RFC 4231 case 2, and HMAC-SHA-256 matches case 6 with its 131-byte key (verifies spec: ENC)
- [x] `uuid_v5` of the DNS namespace and `www.example.com` is `2ed6657d-e927-568b-95e1-2665a8aea6a2`, with the namespace in either case (verifies spec: ENC)
- [x] `uuid_v5` refuses a namespace in simple, braced, URN or misplaced-hyphen form (verifies spec: ENC)

## Byte arguments

- [x] Pass a string, Blob, `app.file(...)` and `app.dir(...)` as bytes; an int throws, naming the argument (verifies spec: ENC)
- [x] The digest of `app.dir("conf")` holding `a.txt` = `abc` equals SHA-256 of the directory encoding written out by hand (verifies spec: ENC)

## Password hashes

- [x] `password_argon2id` and `password_bcrypt` under a known seed match hashes computed by `argon2-cffi` and `bcrypt` from the same derivation (verifies spec: RND)
- [x] Two calls with the same label and different passwords give different salts (verifies spec: RND)
- [x] An empty password, an empty label, a non-bytes password, a bcrypt password over 72 bytes or holding a zero byte each throw; a 72-byte bcrypt password hashes (verifies spec: RND)
- [x] The same call gives the same hash under one seed and a different one under another, and the hash is withheld (verifies spec: RND, SEC)
- [x] Two hashes under a password hash limit of 1 fail, under 2 or 0 succeed, and under a derived values limit of 1 fail too (verifies spec: LIM)

## Withholding

- [x] `"Basic " + encode_base64("admin:" + pw)` with `pw` derived is withheld (verifies spec: SEC)
- [x] A hex of a parameter's value is withheld when `secret(true)` comes after `value()`, and shown when the parameter is not secret (verifies spec: SEC)
- [x] A digest of a hex of a secret value, HMAC'd and encoded, is still withheld (verifies spec: SEC)
- [x] An empty secret value withholds nothing (verifies spec: SEC)
- [x] A hex of a bundle file whose contents equal a secret value is shown (verifies spec: SEC)
- [x] A derived Blob is withheld in its hex, base64 and base64url forms (verifies spec: SEC)
- [x] Withheld results push an evaluation past its derived values limit, whether withheld at once or when a parameter's flag settles (verifies spec: LIM, SEC)
- [x] Register an app whose container env holds `"Basic " + std::encode_base64("admin:" + pw)` with `pw` from `std::random_string`; `apps show` reports that env value as `null` (verifies spec: APPS)
