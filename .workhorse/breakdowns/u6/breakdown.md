---
base: default
---

# Standard helpers for BSL

Built-in helpers for parsing, checking and encoding values in BSL, so definitions stop carrying their own copies. Each card is one subject and ships on its own. Every helper lives in a `std` module, called as `std::parse_url(v)`, so a helper added later cannot collide with a function or variable a script defines. Every parameter kind comes with a `std::parse_{kind}` function (hyphens in the kind written as underscores), and the runtime's check for a kind is that function succeeding, so a script's parsed value and the runtime's check can never disagree. A parse function throws a script error on malformed input. There are no separate `is_{kind}` predicates: a validator calls the parse function and lets it throw, and a script that wants a yes-or-no answer catches the error. A parse function returns a map for compound text such as a URL or an email address, a plain string or integer where that is all the value is, and an opaque type only where the value needs methods, such as a version or a subnet. Helpers are replicable: a definition is re-evaluated often and must declare the same thing from the same inputs, so nothing reads the clock or queries the network, and randomness comes only from the replicable randomness card's app-scoped derivation. `col` stays a free-standing function outside `std`, as part of BSL's core vocabulary rather than a helper.

## Set up the std module for BSL helpers · W6

Add the `std` module that every standard helper lives in, reachable as `std::name` from every context a script runs in (top-level evaluation, actions, validators and gate bodies), and give it a spec of its own that the helper cards add to. The spec carries the conventions every helper follows: a helper throws a script error on malformed input and never coerces it, its result shapes follow the rule in this breakdown's intro, and it is replicable, reading neither the clock nor the network. The card ships the module with no helpers in it, or with only what is needed to test it. The BSL scripting guide gains a section introducing the module. To settle: whether a script can list the module's helpers.

## Pair every parameter kind with a parse function · Y6

Establish the convention the kind cards build on: a `std::parse_k(text)` function for each parameter kind `k`, which the runtime runs to check a value of kind `k`. Builds on the std module card. Apply it to every existing kind without exception, such as `parse_email` returning the local part and domain, and `parse_password` applying the strength check, even where the function returns its input unchanged, as `parse_multiline` does. `text` becomes single-line: `parse_text` refuses a line break, so the runtime refuses one in a `text` value, and multi-line text is what `multiline` is for. The Actions spec's kind table and the Parameters spec carry the rule. To settle: which characters count as a line break, and whether `parse_text` refuses other control characters too.

## URL and hostname parameter kinds · H7

Add `url`, `url-origin` and `hostname` kinds with `parse_url`, `parse_url_origin` and `parse_hostname`. `parse_url` returns a map of scheme, host, port, path, query, fragment, username and password, with the port filled from the scheme's default when the URL gives none. `url-origin` accepts an `http` or `https` URL with a host and an optional port and nothing else (a single trailing `/` allowed), which is what an ingress is declared from. `hostname` accepts a name valid as an ingress hostname and returns it in lower case. To settle: IPv6 literals and internationalised hostnames in `url`, given an ingress hostname is ASCII letters, digits and hyphens (could add IDN/punycode handling, or teach ingress about it); and whether the query comes back as a raw string or decoded into a map.

## Package URL parameter kind · E7

Add a `purl` kind with `std::parse_purl`, accepting a [Package URL](https://github.com/package-url/purl-spec) such as `pkg:docker/library/postgres@16.4` and returning a map of type, namespace, name, version, qualifiers and subpath, so a definition can take a package or image reference in one standard form. To settle: whether the type-specific rules for known types (such as `docker` and `oci`) are checked or only the generic syntax, and how a purl's version relates to the version kind.

## Version parameter kind and requirement matching · A7

Add a `version` kind with `parse_version`, returning a `Version` with major, minor, patch, pre-release and build getters, ordered by SemVer 2.0.0 precedence... or an alternative design supporting purl's wider version definition (which could also be useful for e.g. handling deb/rpm versions, or python versions, etc) - in which case we'd probably have a `version-semver` kind to opt into the stricter behaviour. `version.matches(requirement)` takes the same requirement syntax as the `seedling` field of `seedling.toml`, such as `~2.71`. A leading `v` is accepted.

## Definition version in the bundle metadata · D7

Add a `version` field to `seedling.toml` and expose it to scripts as a constant holding a `Version`, so a definition can default its image tag to the release it was published for and refuse versions outside a compatible range. Builds on the version kind card. To settle: the constant's name and its value when the field is absent; whether build metadata (such as a commit tag for an unreleased build) carries the image tag, or the tag is declared separately.

## Duration parameter kind · J7

Add a `duration` kind with `parse_duration`, for timeouts, intervals and retention parameters. To settle: the accepted syntax (single unit like `90s`, or compound like `1h30m`), the units, and what it returns (Seedling's existing duration options take a number of seconds). Probably use the friendly format from jiff.

## Byte-size and bit-size parameter kinds

Add `byte-size` and `bit-size` kinds with `parse_byte_size` and `parse_bit_size`, returning integers of bytes and bits, comparable with `AVAILABLE_MEMORY`. To settle: SI (`MB`, 1000) and IEC (`MiB`, 1024) suffixes and their case, and whether container memory limits, which read `512m` as mebibytes, move to the same syntax.

## IP address, subnet and range parameter kinds

Add `ip-address`, `ip-subnet` and `ip-range` kinds with their parse functions, for allow-lists, trusted proxies and address pools. Parsed values tell their family and test containment of an address or another subnet. To settle: whether a subnet with host bits set is refused or masked, the syntax of a range, and whether IPv4-mapped IPv6 addresses and zones are accepted.

## Boolean, integer and choice parameter kinds · B7

Add `boolean`, `integer` and `choice` kinds with their parse functions, so a script stops comparing parameter values against `"true"` and operator interfaces can offer a toggle, a number field and a dropdown. An integer's range and a choice's options are arguments of the kind, which the schema API takes today as a bare string, so this card changes that API.

## Structured output encoders

Add encoders producing JSON from any value (not only a map, as Rhai's `to_json` is), YAML, TOML, INI and env-file text, for writing configuration files into volumes. To settle: key ordering, how a value the format cannot hold (such as a nested map in INI) is refused, and whether decoders come with them.

## Quoting for shells, SQL and URLs

Add functions that quote a string as a shell argument, a SQL string literal and a SQL identifier, and that percent-encode a URL component, so values put into commands and statements cannot change their meaning. To settle: the SQL dialects covered, and whether shell quoting always quotes or only when needed.

## Digests and binary encodings · C7

Add SHA-256, SHA-512, BLAKE2b and BLAKE3 digests, HMAC, base64 and hex encoding and decoding, name-based (v5) UUIDs, and salted password hashes such as Argon2id, bcrypt and htpasswd entries, for basic-auth headers, keys that move when content moves, and stable identifiers.

## Replicable randomness and password hashing · V6

Give scripts random bytes, random strings, random (v4 not v7) UUIDs, which return the same value on every evaluation of the same app. Each value is derived from a seed the runtime holds per app, protected like a secret parameter and never visible to the script, together with a label the script passes and the call's own inputs. Deriving by label rather than drawing from a stream means adding, removing or reordering a call does not change the values other calls return, and two apps never share values. To settle: whether the seed travels with an app when it is migrated or restored; whether an operator can rotate it, knowing every derived value changes; and whether derived values are redacted in logs and history like secret parameters.

## Redact secret values in volume file writes and sub-action parameters · X6

Extend redaction to the two paths that still show secret values to operators: the file writes reported when an app is described, and the parameter map of each `Action.invoke` call in the action execution log. Both use the same "withheld" marker as other redacted values. Values derived from an app's seed by the replicable randomness card are redacted wherever secret parameter values are, so both paths cover them too.

## Regular expressions

Add regular expression matching, capture, replacement and splitting, with an engine whose running time is linear in its input, or supports execution budgets, so a pattern from a parameter cannot stall evaluation.