---
base: default
---

# Standard helpers for BSL

Built-in helpers for parsing, checking and encoding values in BSL, so definitions stop carrying their own copies. Each card is one subject and ships on its own. Every parameter kind comes with a `parse_{kind}` function (hyphens in the kind written as underscores), and the runtime's check for a kind is that function succeeding, so a script's parsed value and the runtime's check can never disagree. A parse function throws a script error on malformed input, and a script that wants a yes-or-no answer catches it. Helpers are deterministic: nothing reads the clock, draws randomness or queries the network, because a definition is re-evaluated often and must declare the same thing from the same inputs.

## Pair every parameter kind with a parse function

Establish the convention the other cards build on: each parameter kind `k` has a global `parse_k(text)` function, and the runtime checks a value of kind `k` by running it. Apply it to the existing kinds, such as `parse_email` returning the local part and domain, and `parse_password` applying the strength check. The Actions spec's kind table and the Parameters spec carry the rule. To settle: whether the kinds that accept any string (`text`, `multiline`, `random`, `weak-password`) get a parse function that returns its input, or are exempt.

## URL and hostname parameter kinds

Add `url`, `url-origin` and `hostname` kinds with `parse_url`, `parse_url_origin` and `parse_hostname`. `parse_url` returns a map of scheme, host, port, path, query, fragment, username and password, with the port filled from the scheme's default when the URL gives none. `url-origin` accepts an `http` or `https` URL with a host and an optional port and nothing else (a single trailing `/` allowed), which is what an ingress is declared from. `hostname` accepts a name valid as an ingress hostname. To settle: IPv6 literals and internationalised hostnames in `url`, given an ingress hostname is ASCII letters, digits and hyphens; and whether the query comes back as a raw string or decoded into a map.

## Version parameter kind and requirement matching

Add a `version` kind with `parse_version`, returning a `Version` with major, minor, patch, pre-release and build getters, ordered by SemVer 2.0.0 precedence. `version.matches(requirement)` takes the same requirement syntax as the `seedling` field of `seedling.toml`, such as `~2.71`. A leading `v` is accepted. To settle: whether one- and two-part versions are accepted (strictness keeps an image tag like `16.4-bookworm` from reading as a pre-release); how pre-releases match a requirement.

## Definition version in the bundle metadata

Add a `version` field to `seedling.toml` and expose it to scripts as a constant holding a `Version`, so a definition can default its image tag to the release it was published for and refuse versions outside a compatible range. Builds on the version kind card. To settle: the constant's name and its value when the field is absent; whether build metadata (such as a commit tag for an unreleased build) carries the image tag, or the tag is declared separately.

## Duration parameter kind

Add a `duration` kind with `parse_duration`, for timeouts, intervals and retention parameters. To settle: the accepted syntax (single unit like `90s`, or compound like `1h30m`), the units, and what it returns (Seedling's existing duration options take a number of seconds).

## Byte-size and bit-size parameter kinds

Add `byte-size` and `bit-size` kinds with `parse_byte_size` and `parse_bit_size`, returning integers of bytes and bits, comparable with `AVAILABLE_MEMORY`. To settle: SI (`MB`, 1000) and IEC (`MiB`, 1024) suffixes and their case, and whether container memory limits, which read `512m` as mebibytes, move to the same syntax.

## IP address, subnet and range parameter kinds

Add `ip-address`, `ip-subnet` and `ip-range` kinds with their parse functions, for allow-lists, trusted proxies and address pools. Parsed values tell their family and test containment of an address or another subnet. To settle: whether a subnet with host bits set is refused or masked, the syntax of a range, and whether IPv4-mapped IPv6 addresses and zones are accepted.

## Boolean, integer and choice parameter kinds

Add `boolean`, `integer` and `choice` kinds with their parse functions, so a script stops comparing parameter values against `"true"` and operator interfaces can offer a toggle, a number field and a dropdown. An integer's range and a choice's options are arguments of the kind, which the schema API takes today as a bare string, so this card changes that API.

## Structured output encoders

Add encoders producing JSON from any value (not only a map, as Rhai's `to_json` is), YAML, TOML, INI and env-file text, for writing configuration files into volumes. To settle: key ordering, how a value the format cannot hold (such as a nested map in INI) is refused, and whether decoders come with them.

## Quoting for shells, SQL and URLs

Add functions that quote a string as a shell argument, a SQL string literal and a SQL identifier, and that percent-encode a URL component, so values put into commands and statements cannot change their meaning. To settle: the SQL dialects covered, and whether shell quoting always quotes or only when needed.

## Digests and binary encodings

Add SHA-256 and SHA-512 digests, base64 and hex encoding and decoding, and name-based (v5) UUIDs, for basic-auth headers, keys that move when content moves, and stable identifiers. To settle: whether password hashing (bcrypt, htpasswd) belongs here, since its random salt makes it non-deterministic.

## Regular expressions

Add regular expression matching, capture, replacement and splitting, with an engine whose running time is linear in its input, so a pattern from a parameter cannot stall evaluation.
