---
id: FMT
---

# Structured formats

Applies to the helpers in the [std module](std.md#the-std-module) through which a [script](overview.md#script) writes a value as JSON, YAML, TOML or env-file text, such as a configuration file it [writes into a volume](volume.md#declared-file-writes), and reads a value from such text.

## Borrowed terms

| term | meaning |
| --- | --- |
| JSON | The format defined in [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259). |
| YAML, core schema | The format defined in [YAML 1.2.2](https://yaml.org/spec/1.2.2/), and the schema it defines in [section 10.3](https://yaml.org/spec/1.2.2/#103-core-schema). |
| TOML | The format defined in [TOML 1.1.0](https://toml.io/en/v1.1.0), of which every [TOML 1.0.0](https://toml.io/en/v1.0.0) document is also a document. |

## Terminology

### Env file

Text holding one string variable per line, as [env files](#env-files) specifies.

### Position

The map keys and array indices that lead from a helper's argument to a value within it.

## Helpers

The std module MUST offer these helpers.

| helper | returns |
| --- | --- |
| `std::encode_json(value)` | the JSON text of `value` |
| `std::decode_json(text: bytes)` | the value the JSON text `text` holds |
| `std::encode_yaml(value)` | the YAML text of `value` |
| `std::decode_yaml(text: bytes)` | the value the YAML text `text` holds |
| `std::encode_toml(value: map)` | the TOML text of `value` |
| `std::decode_toml(text: bytes)` | the map the TOML text `text` holds |
| `std::encode_env(value: map)` | the env file text of `value` |
| `std::decode_env(text: bytes)` | the map the env file text `text` holds |

`text` is a [`bytes` argument](encoding.md#byte-arguments), so a script decodes a bundled [`File`](bundle.md#bundle-files) by passing it directly.

The string length, array length and object map [limits](../runtime/engine-limits.md#limits) MUST apply to every result of these helpers.

A helper MUST raise a script error when the value it encodes or decodes nests arrays and maps more than 128 levels deep, counting the outermost array or map as the first level.

## Encoding

### Values each format holds

An encoder MUST write each value as the table gives for its format.

| BSL value | JSON | YAML | TOML | env file |
| --- | --- | --- | --- | --- |
| unit | `null` | `null` | refused | refused |
| boolean | boolean | boolean | boolean | `true` or `false` |
| integer | number | integer | integer | its decimal text |
| finite floating-point number | number | float | float | its text, as JSON writes it |
| infinite or NaN floating-point number | refused | `.inf`, `-.inf` or `.nan` | `inf`, `-inf` or `nan` | refused |
| string | string | string | string | the string |
| `Version` or `SemVer` | its text, as a string | its text, as a string | its text, as a string | its text |
| array | array | sequence | array | refused |
| map | object | mapping | table | refused, except as the argument itself |

An encoder MUST refuse a value of any other type, including a Blob.

> [!NOTE]
> No format here has a type for bytes, so a script that wants a Blob in one encodes it first with one of the [encodings](encoding.md#encodings), and chooses which.

An encoder MUST write a floating-point number with a fraction or an exponent, so that decoding it gives a floating-point number and not an integer, and so that decoding it gives the same number.

### Refusal

An encoder MUST raise a script error when the value, or any value within it, is one its format refuses.
The script error MUST name the position of the refused value and the reason its format refuses it.
An encoder MUST NOT drop, convert or substitute a value its format refuses.

### Key order

An encoder MUST write the entries of every map in ascending bytewise order of the UTF-8 encoding of their keys, except where [TOML](#toml) requires another order.

> [!NOTE]
> The text depends on nothing but the value, so the same value always writes the same file.

### Layout

Each encoder MUST write one fixed layout, which no argument changes.
An encoder SHOULD give the same text for the same value in every Seedling version.
Every result that is not empty MUST end with a line feed.

> [!NOTE]
> Upgrading Seedling alone then does not change the files a definition declares, so a definition evaluated before and after an upgrade declares the same resource graph.

### JSON

`encode_json` MUST write each array element and each object member on its own line, indented by two spaces for each level of nesting, and an empty array or object as `[]` or `{}`.
It MUST escape in a string only the characters JSON requires escaped, and write every other character as itself.

The runtime MUST raise a script error, naming `std::encode_json`, when a script calls Rhai's own `to_json` method on a map.

> [!NOTE]
> Rhai's method writes keys, units, Blobs and non-finite numbers by rules of its own, so keeping it would give scripts two JSON writers that disagree.

### YAML

`encode_yaml` MUST write arrays and maps in block style, indented by two spaces for each level of nesting, and an empty array or map as `[]` or `{}`.
It MUST write a string, map keys included, quoted when any of these holds of it:

- it is empty, or begins or ends with whitespace;
- it cannot be written as a plain scalar, or a reader applying the core schema would read it plain as other than that string;
- it is `~`, or one of `yes`, `no`, `on`, `off`, `true`, `false` and `null` written in lower case, upper case, or with only its first letter in upper case;
- it reads as an integer or a floating-point number, including in hexadecimal, octal or exponent form, or begins with `.`.

> [!NOTE]
> A YAML 1.1 reader reads plain `no`, `on` or `0755` as a boolean or a number, and such readers are still common, so quoting the words and numbers such readers most often misread keeps a file meaning the same to them at the cost of a few quotes.

### TOML

`encode_toml` MUST write a TOML 1.0.0 document.
It MUST refuse unit at any depth.
It MUST write a map within the argument as a table under a `[table]` header, and an array that holds at least one element, every one a map, as an array of tables under `[[array]]` headers.
It MUST write any other array inline, with a map within it as an inline table.
It MUST write a key bare when TOML allows it bare, and quoted otherwise.
Within each table, it MUST write the entries written as key/value lines before the tables, each in [key order](#key-order).

### Env files

An env file is a sequence of lines, each ended by a line feed.

`encode_env` MUST write one line for each entry of its argument: the key, `=`, and the value between single quotes, with no whitespace.
It MUST refuse a key that does not begin with an ASCII letter or an underscore followed by only ASCII letters, ASCII digits and underscores.
It MUST refuse a value whose text holds a single quote, a line feed, a carriage return or NUL.
Given an empty map, it MUST return an empty string.

> [!NOTE]
> A shell, systemd and the common dotenv readers all read a single-quoted value that holds no single quote and no line break as exactly the characters between the quotes.
> They disagree on double quotes, escapes, variable expansion and multi-line values, which is why the encoder refuses what it cannot single-quote and the decoder reads none of them.

`decode_env` MUST read text in which each line is one of:

- blank, holding nothing or only spaces and tabs;
- a comment, beginning with `#`;
- an entry: a key of the form `encode_env` accepts, `=`, and a value, with no whitespace before the key or around `=`.

A value MUST be one of:

- empty;
- a single quote, any characters other than a single quote, a carriage return or NUL, and a single quote;
- one or more of the ASCII letters and digits and the characters `_`, `-`, `.`, `/`, `:`, `@`, `%`, `+`, `,` and `=`.

`decode_env` MUST raise a script error at any other line, including one with a carriage return, or a double quote, a backslash or a `$` outside a single-quoted value, an `export` prefix, or anything after a value.
It MUST accept a final line with no line feed.
Each value MUST decode to a string holding the characters between its quotes, or the value itself when unquoted.

## Decoding

A decoder MUST raise a script error when `text` is not valid UTF-8, or not a document of its format.
A decoder MUST ignore a byte order mark at the start of `text`.
A decoder MUST NOT return a partial or corrected value in place of raising a script error.

A decoder MUST return each value as the table gives for its format.

| format | value | BSL value |
| --- | --- | --- |
| JSON | `null` | unit |
| JSON | `true`, `false` | boolean |
| JSON | number with no fraction and no exponent | integer |
| JSON | any other number | floating-point number |
| YAML | null | unit |
| YAML | boolean | boolean |
| YAML | integer | integer |
| YAML | float | floating-point number |
| TOML | boolean | boolean |
| TOML | integer | integer |
| TOML | float | floating-point number |
| TOML | offset date-time, local date-time, local date, local time | string holding the value in the form TOML gives for its type, with `T` between a date and a time |
| any | string | string |
| any | array, sequence | array of the decoded elements |
| any | object, mapping, table, inline table | map of the decoded values |

A decoder MUST raise a script error at an integer outside the range of a signed 64-bit integer.
A decoder MUST raise a script error at any other number too large in magnitude for a finite floating-point number, other than YAML's and TOML's own infinities.
A decoder MUST raise a script error at a map that holds the same key twice.

### YAML documents

`decode_yaml` MUST read YAML under the core schema.
It MUST raise a script error when `text` holds no document or more than one.
It MUST raise a script error at a tag the core schema does not define, and at a tagged node that is not of its tag's type.
It MUST raise a script error at a mapping key that does not decode to a string.
It MUST resolve each alias to a copy of the node its anchor names, and MUST raise a script error when the values that aliases copy number more than 100 000 in total.

> [!NOTE]
> A small document can make a few aliases each copy a node that itself holds aliases, which multiplies without bound unless the copies are counted.

## Withheld arguments

> [!NOTE]
> A result of one of these helpers is withheld from operators when an argument contains a [withheld value](../runtime/secrets.md#withheld-value), as [redaction](../runtime/secrets.md#redaction) specifies.
