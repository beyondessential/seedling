# IP address, subnet and range parameter kinds

Automated cases live in `crates/core/src/tests/ip.rs` unless noted.

## Parsing

- [x] An IPv4 or IPv6 address in any accepted spelling parses, and prints in canonical text that parses back to an equal value (verifies spec: IP)
- [x] An address with a leading zero, a zone, brackets, surrounding whitespace or a prefix is refused (verifies spec: IP)
- [x] An IPv4-mapped IPv6 address is refused, naming the IPv4 address (verifies spec: IP)
- [x] A subnet with or without a prefix length parses; a bare address is /32 or /128 (verifies spec: IP)
- [x] A subnet with host bits set is refused, naming the subnet it lies in (verifies spec: IP)
- [x] A prefix length that is not a decimal number within the family's width is refused (verifies spec: IP)
- [x] A range as two addresses, a subnet or one address parses; `::/80` gives a range ending on a mapped address that parses back (verifies spec: IP)
- [x] A range out of order, across families, wholly among mapped addresses or with whitespace is refused (verifies spec: IP)
- [x] Every single-line parser, the IP ones included, refuses control characters (`tests/stdlib.rs`; verifies spec: ACN)

## Family argument

- [x] A parser given `family` refuses the other family with an error naming the expected one (verifies spec: IP)
- [x] A parser given a family other than the integer 4 or 6 throws (verifies spec: IP)
- [x] A kind map takes `family` on the IP kinds only, and refuses an invalid one (`tests/param.rs`; verifies spec: ACN, IP)
- [x] An app description and a template preview report `family` as 4, 6 or null (`oi/handler/apps/tests.rs`, `oi/handler/templates/tests.rs`; verifies spec: APPS, TPL)

## Values

- [x] Each value tells its family, an address mapped from a range end included (verifies spec: IP)
- [x] An address converts to its own subnet, to the subnet of a prefix length with host bits cleared, and to a range; an out-of-bounds prefix length throws (verifies spec: IP)
- [x] A subnet gives its address, prefix length and range; a range gives its start and end (verifies spec: IP)
- [x] `contains` and `overlaps` hold across every pairing of address, subnet and range, edges included (verifies spec: IP)
- [x] `contains` and `overlaps` are false across families, and throw for a string or any other value (verifies spec: IP)
- [x] Addresses order numerically within a family; across families `==` is false and ordering throws (verifies spec: IP)
- [x] Subnets and ranges compare for equality only (verifies spec: IP)
- [x] Comparing an IP value with any other type, another IP type included, throws on either side (verifies spec: IP)
- [x] Values print as canonical text through `to_string`, interpolation and concatenation (verifies spec: IP)

## Delivery

- [x] Action params of the IP kinds are checked by their kind and reach the closure as their types after the param map is persisted (`runtime/action_params.rs`; verifies spec: ACN)
- [x] A validator receives IP values, and a value of the wrong family is refused before validators run (`tests/param.rs`; verifies spec: PARAM)
- [x] `invoke` takes an IP value as its canonical text (`tests/action_call.rs`; verifies spec: ACN)
- [x] A sub-action's IP param is recorded with its type and replays as that type (`tests/action_call.rs`; verifies spec: OPS)
- [x] The operator interface offers a single-line text input for the IP kinds (`ParamFields.test.tsx`; verifies spec: ACN)
- [ ] An operator sets an `ip-range` parameter with `family: 4` in the web UI, sees an IPv6 range refused with the family named, and sees an IPv4 range accepted
