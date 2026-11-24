# IP address, subnet and range parameter kinds

## Notes

- Depends on B7's kind arguments: the `family` argument and the IP spec's link to `action.md#kind-arguments` only resolve once B7 has merged. Make these `action.md` edits after B7 lands, rather than conflicting with its rewrite now:
  - three rows in the kind table: `ip-address`, `ip-subnet`, `ip-range`, each with `family` optional, parser results `IpAddress`, `IpSubnet`, `IpRange` linking to the IP spec;
  - `invoke` accepting the three types in `params`, taken as their canonical text, as it takes `Version` and `SemVer`;
  - a single-line input for the three kinds under presentation of kinds.
- Lists of values (allow-lists, trusted proxies) are N7's `list` argument, not this card.
- The workspace already depends on `ipnet`; its parsers are lenient where this spec is strict (host bits kept, mapped addresses and zones handled differently), so those checks are explicit. `Ipv6Addr`'s `Display` follows RFC 5952, including mixed notation for mapped addresses.
