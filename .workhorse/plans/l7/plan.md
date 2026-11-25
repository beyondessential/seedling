# IP address, subnet and range parameter kinds

Adds the `ip-address`, `ip-subnet` and `ip-range` kinds, their parsers and the `IpAddress`, `IpSubnet` and `IpRange` BSL types, following the IP spec, and the edits to the actions, apps and templates specs they need now that B7's kind arguments have merged.

## Notes

- B7 has merged, so the `family` argument and the IP spec's link to kind arguments resolve. The actions, apps and templates spec edits below close the two issues held for it.
- Lists of values (allow-lists, trusted proxies) are N7's `list` argument, not this card.
- The workspace already depends on `ipnet`; its parsers are lenient where this spec is strict (host bits kept, mapped addresses and zones handled differently), so those checks are explicit. `Ipv6Addr`'s `Display` follows RFC 5952, including mixed notation for mapped addresses.
- The grammar is a split on `/` or `-` around the standard library's address parsers, plus explicit refusals, so it stays hand-written rather than winnow. The parsers' tests pin the standard library's strictness the spec relies on: no leading zeros in a dotted part, no zone, no brackets, no `+` before a prefix.
- A range is held as a family and two addresses; containment and overlap reduce every value to an inclusive interval of `u128` in address order, so one comparison serves all nine pairings.
- `kind_with` takes `range` and `options` positionally; with `family` on three kinds it takes a struct of the optional arguments instead.
- The refusal of comparisons with any other type in `version.rs`'s `register_comparisons` moves to a shared helper both modules use, with the error text given by the caller.
- `ParamMap` keeps each typed parser result's type under `versions` (`VersionType`), and the action log payload under `"versions"`, which the IP types make wrong. The Rust type, field and serialised key are renamed together, to `ParsedType` and `types`. Seedling is unshipped, so an operation persisted under the old key before the upgrade replaying its version params as text is accepted.

## Specs

- [ ] Actions spec, param kinds: three rows, each with `family` optional, value as the IP spec accepts, parser results `IpAddress`, `IpSubnet` and `IpRange` linking to the IP spec's value sections
- [ ] Actions spec, kind arguments: an IP kind's `family` is valid as the IP spec's family argument says
- [ ] Actions spec, action invocation: `IpAddress`, `IpSubnet` and `IpRange` accepted in `params`, taken as their canonical text
- [ ] Actions spec, presentation of kinds: the three kinds in the single-line list
- [ ] Apps spec, parameters: a `family` member, `4` or `6` for an IP kind given a `family` and `null` otherwise; add it to the action and shell `params` member list
- [ ] Templates spec: `family` in the `params` entry member list
- [ ] Record both held issues fixed against their commits

## Values (`crates/core/src/defs/ip.rs`)

- [ ] `Family` (4 or 6) and `take_family`, refusing anything but the integers `4` and `6`
- [ ] `IpAddress`, `IpSubnet` and `IpRange` with `Display` giving the canonical text
- [ ] `parse_address`: standard library parse, refusing an IPv4-mapped IPv6 address
- [ ] `parse_subnet`: `address/prefix` or a bare address; prefix as a decimal number within the family's width, sharing the no-leading-zero check with `param_def.rs`'s `parse_decimal`; host bits refused with an error naming the cleared subnet
- [ ] `parse_range`: two addresses joined by `-` (mapped ends allowed, same family, in order, not both in `::ffff:0:0/96`), else a subnet, else an address
- [ ] `CustomType` for each: the methods in the IP spec's value tables, `to_subnet(prefix_len)` throwing out of bounds, `contains` and `overlaps` taking any of the three and throwing for anything else
- [ ] Operators: `IpAddress` ordered within a family, `==` false and ordering throwing across families; `IpSubnet` and `IpRange` with `==` and `!=` only and ordering throwing; every comparison with another type throwing through the shared helper
- [ ] `on_print` and `on_debug` giving the canonical text
- [ ] Register the three types in `defs.rs`

## Kinds (`param_def.rs`, `stdlib/kinds.rs`)

- [ ] `ParamKindName` variants, `as_str`, and `family` in `arguments`
- [ ] `ParamKind` variants carrying `family: Option<Family>`; `parse` refusing the other family with an error naming the expected one
- [ ] `take_kind` reading `family`, and `kind_with` taking the arguments struct
- [ ] `ParsedValue` variants and `into_dynamic`
- [ ] Parsers registered with and without `family`

## Delivery, persistence and invoke

- [ ] `runtime/param_map.rs`: rename `VersionType` to `ParsedType` and `versions` to `types` (field, accessor, `with_versions`, serialised name), then add the three IP types, held as their canonical text and restored by their parsers
- [ ] `runtime/barrier/action_call.rs`: write and read the payload key `"types"`, and update `tests/action_call.rs`'s assertion on it
- [ ] `defs/json.rs` `from_dynamic`: the three types as their canonical text

## Wire and web

- [ ] `oi/handler/appdef_json.rs` `param_schema_json`: the `family` member
- [ ] `web/frontend/src/lib/types.ts`: `family: 4 | 6 | null` on both param schema interfaces; `family: null` in the fixtures that set `range` and `options`

## Tests

- [ ] `crates/core/src/tests/ip.rs`: parser accept and refuse tables for addresses, subnets and ranges; family argument; methods; operators within and across families; containment and overlap across the nine pairings and across families; canonical text and its round trip; mixing types
- [ ] `tests/stdlib.rs` parser list and `tests/param.rs` kind list, including kind maps with a valid and an invalid `family`
- [ ] `runtime/action_params.rs`: IP params checked and delivered by their kind, and a `family` refusal giving the parser's reason
- [ ] Persistence: an IP param survives the param map and action log round trip as its type; `invoke` takes an `IpAddress` as its text
- [ ] `oi/handler/apps/tests.rs` and `templates/tests.rs`: `family` is `4` or `6` where given and `null` otherwise
- [ ] `ParamFields.test.tsx`: the three kinds get a single-line text input

## Docs

- [ ] `docs/bsl-scripting.md`: the kinds list, the parsers list, and a section on the IP kinds with an allow-list example

## Finish

- [ ] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh`, and the frontend tests touched
