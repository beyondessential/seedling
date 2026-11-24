# IP address, subnet and range parameter kinds

## Notes

- Depends on B7's kind arguments: the `family` argument and the IP spec's link to `action.md#kind-arguments` only resolve once B7 has merged. Add the three rows to the kind table in `action.md` (with its arguments column) after B7 lands, rather than conflicting with it now.
- Lists of values (allow-lists, trusted proxies) are N7's `list` argument, not this card.
- The workspace already depends on `ipnet`; its parsers are lenient where this spec is strict (host bits kept, mapped addresses accepted), so those checks are explicit.
