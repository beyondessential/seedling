---
id: RND
---

# Derived random values

Applies to the helpers in the [std module](std.md#the-std-module) through which a [script](overview.md#script) draws random values, and salted password hashes, that the [runtime](../overview.md#runtime) derives from a seed, so that a call returns the same value on every evaluation that shares the seed.

## Borrowed terms

| term | meaning |
| --- | --- |
| UUID | A 128-bit universally unique identifier, [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562). |
| HMAC-SHA-256 | HMAC, [RFC 2104](https://www.rfc-editor.org/rfc/rfc2104), over SHA-256, [FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final). |
| ChaCha20 | The stream cipher defined in [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439). |
| Argon2id | The password hash defined in [RFC 9106](https://www.rfc-editor.org/rfc/rfc9106). |
| PHC string | The text form of a password hash defined by the [PHC string format](https://github.com/P-H-C/phc-string-format/blob/master/phc-sf-spec.md). |
| bcrypt | The password hash defined in [A Future-Adaptable Password Scheme](https://www.usenix.org/legacy/events/usenix99/provos/provos.pdf), written in its `$2b$` modular crypt form as OpenBSD defines it. |

## Terminology

### Label

The string a script passes to a derivation helper to name the value it wants.

### Derivation helper

One of the [helpers](std.md#helper) that [the helpers](#helpers) list.

### Password hash helper

One of the derivation helpers that [password hashes](#password-hashes) list.

## Seeds

A derivation helper called in the [static context](app.md#static-context) or the [validator context](app.md#validator-context) MUST derive its value from the app's [app seed](../runtime/secrets.md#app-seed).
A derivation helper called in the [action context](app.md#action-context) MUST derive its value from the [operation seed](../runtime/secrets.md#operation-seed) of the lifecycle operation or shell session whose [action closure](action.md#action-closure) is running.

Two calls deriving from the same seed, through the same helper, with the same label and the same other arguments, MUST return the same value.
Without the seed, the value of a call MUST NOT be predictable from the values of calls that differ from it in helper, label or any argument.

BSL MUST NOT offer a script any way to read a seed.

> [!NOTE]
> A value depends only on the seed and the call itself, so adding, removing or reordering calls leaves every other call's value as it was, and two apps never derive the same values.

> [!NOTE]
> The same call derives a different value inside an action closure than outside it.
> A closure that needs a value the static context derived, such as a password the app's workloads are configured with, captures that value rather than deriving it again.

> [!NOTE]
> A value derived from an operation seed differs in the next operation.
> A value that has to outlast the operation is derived in the static context, or is written into a volume by the operation that derived it.

## Helpers

The std module MUST offer these helpers.

| helper | returns |
| --- | --- |
| `std::random_bytes(label: string, n: int)` | a Blob of `n` bytes |
| `std::random_string(label: string, length: int, alphabet: string)` | a string of `length` characters, each drawn from `alphabet` |
| `std::uuid_v4(label: string)` | a version 4 UUID, as a string |

A derivation helper MUST throw when `label` is the empty string.
`random_bytes` MUST throw when `n` is less than 1, and `random_string` MUST throw when `length` is less than 1.

`random_string` MUST read `alphabet` as a sequence of Unicode characters.
It MUST throw when `alphabet` holds fewer than two characters, or holds any character more than once.
Each character of its result MUST be drawn from `alphabet` with equal probability, independently of the others.

`uuid_v4` MUST return a UUID with the version and variant fields of a version 4 UUID, as [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562#section-5.4) defines, and every other bit derived.
It MUST write the UUID in its hyphenated form with lowercase hexadecimal digits.

> [!NOTE]
> Changing a call's length or alphabet changes its whole value, and two uses that need distinct values need distinct labels.

### Password hashes

The std module MUST also offer these derivation helpers, each of which hashes a password under a salt derived from the seed.

| helper | returns |
| --- | --- |
| `std::password_argon2id(label: string, password: bytes)` | an Argon2id hash of `password`, as a PHC string |
| `std::password_bcrypt(label: string, password: bytes)` | a bcrypt hash of `password`, in modular crypt form |

`password` is a [`bytes` argument](encoding.md#byte-arguments).
A password hash helper MUST throw when `password` is empty.
`password_bcrypt` MUST throw when `password` is longer than 72 bytes or holds a zero byte.

The runtime MUST bound the number of password hash helper calls in one evaluation, as the [engine limits](../runtime/engine-limits.md#limits) specify.

> [!NOTE]
> A hash of the empty password guards nothing, and an empty password most often comes from a value that was never set.

> [!NOTE]
> bcrypt reads a password only up to its 72nd byte or its first zero byte, so a longer password, or one holding a zero byte, would verify against every password sharing its prefix.

> [!NOTE]
> A password hash verifies its password whichever seed its salt was derived from.
> Called in an action closure, the same call returns a different hash in each operation, each of which verifies the same password.

> [!NOTE]
> A password hash is slow to compute on purpose, and a definition is evaluated many times over an app's life, so the work factors are fixed low enough to be paid on every evaluation and the number of hashes is bounded.

## Derivation

The runtime MUST derive each value exactly as this section specifies.

> [!NOTE]
> A derivation that changed between runtime versions would change every derived value on upgrade, as if every seed had been rotated.

### Call encoding

A call's encoding is the concatenation of these fields, in order:

1. the helper's name without its `std::` prefix, which is one of `random_bytes`, `random_string`, `uuid_v4`, `password_argon2id` or `password_bcrypt`, as a string field;
2. the label, as a string field;
3. for `random_bytes`, `n` as an integer field;
4. for `random_string`, `length` as an integer field, then `alphabet` as a string field;
5. for a password hash helper, `password` as a bytes field.

A string field MUST be the length of the string's UTF-8 encoding in bytes, as an unsigned 32-bit big-endian integer, followed by that encoding.
An integer field MUST be the integer as an unsigned 64-bit big-endian integer.
A bytes field MUST be the number of bytes as an unsigned 64-bit big-endian integer, followed by the bytes.

> [!NOTE]
> The password is part of the call, so two calls under the same label that hash different passwords draw different salts, and the salt one hash shows says nothing about another's.

### Byte stream

A call's key MUST be the HMAC-SHA-256 of the call's encoding, keyed with the seed.
A call's byte stream MUST be the ChaCha20 keystream under the call's key, with a nonce of twelve zero bytes and an initial block counter of 0.

### Values

`random_bytes` MUST return the first `n` bytes of the call's byte stream.

`random_string` MUST take successive four-byte words from the call's byte stream, each read as an unsigned 32-bit little-endian integer `w`.
Where `k` is the number of characters in `alphabet`, it MUST discard each `w` at or above `2^32 - (2^32 mod k)`, and otherwise append the character at zero-based position `w mod k` in `alphabet`, until the result holds `length` characters.

`uuid_v4` MUST take the first 16 bytes of the call's byte stream as the UUID's bytes in order, then set the high four bits of the seventh byte to `0100` and the high two bits of the ninth byte to `10`.

A password hash helper MUST take the first 16 bytes of the call's byte stream as its salt.

`password_argon2id` MUST compute an Argon2id tag of 32 bytes over `password` and the salt, at version 19, with a memory size of 19456 KiB, 2 passes and 1 lane, and no secret or associated data.
It MUST write the hash as the PHC string `$argon2id$v=19$m=19456,t=2,p=1$`, followed by the salt, a `$`, and the tag, the salt and the tag each in unpadded base64.

`password_bcrypt` MUST compute a bcrypt hash of `password` under the salt at cost 12.
It MUST write the hash as `$2b$12$`, followed by the salt in 22 characters and the hash in 31 characters, both in bcrypt's base64 alphabet.

