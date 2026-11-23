---
id: RND
---

# Derived random values

Applies to the methods through which a [script](overview.md#script) draws random values that the [runtime](../overview.md#runtime) derives from a seed, so that a call returns the same value on every evaluation that shares the seed.

## Borrowed terms

| term | meaning |
| --- | --- |
| UUID | A 128-bit universally unique identifier, [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562). |

## Terminology

### Label

The string a script passes to a derivation method to name the value it wants.

### Derivation method

One of the methods in [methods](#methods), on `app` or on a [runtime handle](rt.md#runtime-handle).

## Seeds

A derivation method on `app` MUST derive its value from the app's [app seed](../runtime/secrets.md#app-seed).
A derivation method on a runtime handle MUST derive its value from the [operation seed](../runtime/secrets.md#operation-seed) of the lifecycle operation or shell session the handle was passed to.

Two calls deriving from the same seed, through the same method, with the same label and the same other arguments, MUST return the same value.
Without the seed, the value of a call MUST NOT be predictable from the values of calls that differ from it in method, label or any argument.

BSL MUST NOT offer a script any way to read a seed.

> [!NOTE]
> A value depends only on the seed and the call itself, so adding, removing or reordering calls leaves every other call's value as it was, and two apps never derive the same values.

> [!NOTE]
> A value derived from an operation seed differs in the next operation.
> A value that has to outlast the operation comes from the app seed, or is written into a volume by the operation that derived it.

## Methods

`App` and `RuntimeInstance` MUST each offer these methods.

| method | returns |
| --- | --- |
| `random_bytes(label: string, n: int)` | a Blob of `n` bytes |
| `random_string(label: string, length: int, alphabet: string)` | a string of `length` characters, each drawn from `alphabet` |
| `uuid_v4(label: string)` | a version 4 UUID, as a string |

A derivation method MUST throw when `label` is the empty string.
`random_bytes` MUST throw when `n` is less than 1, and `random_string` MUST throw when `length` is less than 1.

`random_string` MUST read `alphabet` as a sequence of Unicode characters.
It MUST throw when `alphabet` holds fewer than two characters, or holds any character more than once.
Each character of its result MUST be drawn from `alphabet` with equal probability, independently of the others.

`uuid_v4` MUST return a UUID with the version and variant fields of a version 4 UUID, as [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562#section-5.4) defines, and every other bit derived.
It MUST write the UUID in its hyphenated form with lowercase hexadecimal digits.

> [!NOTE]
> Changing a call's length or alphabet changes its whole value, and two uses that need distinct values need distinct labels.

## Contexts

A derivation method on `app` MUST be available in the [static context](app.md#static-context) and the [action context](app.md#action-context), and MUST throw in the [validator context](app.md#validator-context).
A derivation method on a runtime handle is available wherever the handle is.
