---
id: QUOT
---

# Quoting

Applies to the helpers in the [std module](std.md#the-std-module) through which a [script](overview.md#script) quotes a string, so that a program parsing text the string is placed in reads back the same string.

## Borrowed terms

| term | meaning |
| --- | --- |
| shell, word, simple command | as defined by the [Shell Command Language](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html) of POSIX.1-2024 |

## Results

The string length [limit](../runtime/engine-limits.md#limits) MUST apply to every result of these helpers.

## Shell words

The std module MUST offer `std::quote_shell(text: string)`, which returns a string that the shell reads as one word whose value is `text`, in any position a word takes in a simple command.

`std::quote_shell` MUST return `text` unchanged when it is not empty and every character of it is an ASCII letter, an ASCII digit, or one of `_`, `-`, `.`, `/`, `:`, `,`, `+`, `@` and `%`.
Otherwise it MUST return `text` enclosed in single quotes, with each single quote within it written as `'\''`.
`std::quote_shell` MUST throw when `text` holds a NUL character.

> [!NOTE]
> The characters left unquoted are those the shell gives no meaning in any position of a word, so a plain value such as a path or a version reads in a command as it would have been typed.
> `=` is quoted because a word holding it can be read as a variable assignment where the command name would stand, and `~` because a word beginning with it is expanded to a home directory.

> [!NOTE]
> The shell reads every character between single quotes literally, a newline included, so the quoted form holds any text but NUL, which no word or program argument can hold.

> [!NOTE]
> Quoting keeps the shell from changing a value, not the program that receives it from reading it.
> A value beginning with `-` still reaches the program as an option unless the script puts `--` before it.

## Withheld arguments

> [!NOTE]
> A result of one of these helpers is withheld from operators when an argument contains a [withheld value](../runtime/secrets.md#withheld-value), as [redaction](../runtime/secrets.md#redaction) specifies.
