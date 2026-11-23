---
id: ENC
---

# Digests and encodings

Applies to the helpers in the [std module](std.md#the-std-module) through which a [script](overview.md#script) encodes bytes as text and decodes them again, digests bytes, authenticates them under a key, and names them with a UUID.

## Borrowed terms

| term | meaning |
| --- | --- |
| SHA-256, SHA-512 | The hash functions defined in [FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final). |
| BLAKE2b | The hash function defined in [RFC 7693](https://www.rfc-editor.org/rfc/rfc7693). |
| BLAKE3 | The hash function defined in the [BLAKE3 specification](https://github.com/BLAKE3-team/BLAKE3-specs/blob/master/blake3.pdf). |
| HMAC | The keyed-hash message authentication code defined in [RFC 2104](https://www.rfc-editor.org/rfc/rfc2104). |
| base16, base64, base64url | The encodings defined in [RFC 4648](https://www.rfc-editor.org/rfc/rfc4648) sections 8, 4 and 5. |
| UUID | A 128-bit universally unique identifier, [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562). |

## Byte arguments

Every argument to which a [helper](std.md#helper)'s signature gives the type `bytes` MUST accept a string, read as its UTF-8 encoding, a Blob, read as its bytes, a [`File`](bundle.md#bundle-files), read as its contents, or a [`Directory`](bundle.md#bundle-files), read as its [directory encoding](#directory-encoding).
A `bytes` argument of any other type MUST raise a script error, as the [argument rules](overview.md#arguments) require.

### Directory encoding

A directory's encoding is the concatenation, for each file in the directory in ascending bytewise order of the UTF-8 encoding of its path relative to the directory, of these fields:

1. the file's path relative to the directory, with `/` as its separator, as the length of its UTF-8 encoding in bytes as an unsigned 32-bit big-endian integer, followed by that encoding;
2. the file's contents, as their length in bytes as an unsigned 64-bit big-endian integer, followed by the contents.

> [!NOTE]
> Every file's path is part of the encoding, so renaming or moving a file within the directory changes a digest of it as surely as changing a file's contents.
> The order and the length prefixes are fixed so that the same directory has the same encoding in every Seedling version.

## Results

The string length and Blob length [limits](../runtime/engine-limits.md#limits) MUST apply to every result of these helpers.

## Encodings

The std module MUST offer these helpers.

| helper | returns |
| --- | --- |
| `std::encode_hex(data: bytes)` | a string encoding `data` in base16, with lowercase digits |
| `std::decode_hex(text: string)` | a Blob of the bytes `text` encodes in base16 |
| `std::encode_base64(data: bytes)` | a string encoding `data` in base64, with padding |
| `std::decode_base64(text: string)` | a Blob of the bytes `text` encodes in base64 |
| `std::encode_base64url(data: bytes)` | a string encoding `data` in base64url, without padding |
| `std::decode_base64url(text: string)` | a Blob of the bytes `text` encodes in base64url |
| `std::encode_utf8(text: string)` | a Blob of the UTF-8 encoding of `text` |
| `std::decode_utf8(data: bytes)` | the string whose UTF-8 encoding is `data` |

Given an empty string or no bytes, each of these helpers MUST return an empty string or an empty Blob, as its result type is.

`decode_hex` MUST accept the base16 alphabet in either case.
It MUST throw when `text` holds an odd number of characters, or any character outside the base16 alphabet in either case.

`decode_base64` MUST throw when `text` holds a character outside the base64 alphabet, other than one or two padding characters at its end, or when its length is not a multiple of four.
`decode_base64url` MUST throw when `text` holds a character outside the base64url alphabet, including padding, or when its length leaves a remainder of one when divided by four.
Both MUST throw when the last character before any padding encodes bits that are not zero beyond the final byte.

`decode_utf8` MUST throw when `data` is not valid UTF-8.

> [!NOTE]
> A strict decoder gives every byte sequence exactly one encoding, so text that decodes is the text the matching encoder would have written.

## Digests

The std module MUST offer these helpers, each returning a Blob.

| helper | returns |
| --- | --- |
| `std::digest_sha256(data: bytes)` | the 32-byte SHA-256 digest of `data` |
| `std::digest_sha512(data: bytes)` | the 64-byte SHA-512 digest of `data` |
| `std::digest_blake2b(data: bytes)` | the 64-byte unkeyed BLAKE2b digest of `data` |
| `std::digest_blake3(data: bytes)` | the 32-byte BLAKE3 digest of `data`, in its default hash mode |

> [!NOTE]
> A digest returns bytes so that it can be passed on as an HMAC key or a UUID name unchanged.
> A script that wants text passes it to one of [the encodings](#encodings).

## Message authentication

The std module MUST offer these helpers, each returning a Blob.

| helper | returns |
| --- | --- |
| `std::hmac_sha256(key: bytes, message: bytes)` | the 32-byte HMAC over SHA-256 of `message` under `key` |
| `std::hmac_sha512(key: bytes, message: bytes)` | the 64-byte HMAC over SHA-512 of `message` under `key` |

Both MUST accept a key of any length.

## Name-based UUIDs

The std module MUST offer `std::uuid_v5(namespace: string, name: bytes)`, which returns the version 5 UUID that [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562#section-5.5) defines for `namespace` and `name`.

`uuid_v5` MUST throw when `namespace` is not a UUID written in hyphenated form, as five groups of 8, 4, 4, 4 and 12 hexadecimal digits.
It MUST accept the digits of `namespace` in either case.
It MUST write its result in hyphenated form with lowercase hexadecimal digits.

> [!NOTE]
> A version 5 UUID depends on nothing but its namespace and name, so the same call returns the same UUID in every app and on every host, unlike a [derived](random.md#helpers) version 4 UUID.

## Withheld arguments

> [!NOTE]
> A result of one of these helpers is withheld from operators when an argument contains a [withheld value](../runtime/secrets.md#withheld-value), as [redaction](../runtime/secrets.md#redaction) specifies.
