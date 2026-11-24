---
id: IP
---

# IP addresses, subnets and ranges

Applies to the `ip-address`, `ip-subnet` and `ip-range` [param kinds](action.md#param-kinds), their [parsers](action.md#kind-parsers), and the values those parsers return.

## Borrowed terms

| term | meaning |
| --- | --- |
| IPv6 text form | the textual representation of an IPv6 address, as defined in [RFC 4291](https://www.rfc-editor.org/rfc/rfc4291) section 2.2 |
| IPv4-mapped IPv6 address | an IPv6 address carrying an IPv4 address in its low 32 bits under the prefix `::ffff:0:0/96`, as defined in [RFC 4291](https://www.rfc-editor.org/rfc/rfc4291) section 2.5.5.2 |
| prefix length | the number of leading bits an address block fixes, as in the CIDR notation of [RFC 4632](https://www.rfc-editor.org/rfc/rfc4632) section 3.1 |

## Terminology

### Family

Whether an address, subnet or range is of IPv4 or of IPv6.

### Host bits

The bits of a subnet's address after its prefix length.

## Addresses

`std::parse_ip_address(text)` MUST accept exactly the values below, and MUST reject every other value.

- An IPv4 address, written as four decimal numbers from 0 to 255 separated by dots, none with a leading zero.
- An IPv6 address in the IPv6 text form, with no enclosing brackets and no zone index, that is not an IPv4-mapped IPv6 address.

Hexadecimal digits in an IPv6 address MUST be accepted in either case.

> [!NOTE]
> A zone index names an interface on one host, which means nothing to a workload on a pod network.
> An IPv4-mapped address in configuration is most often a dual-stack socket's view of an IPv4 peer, copied where the IPv4 address was meant.

## Subnets

`std::parse_ip_subnet(text)` MUST accept exactly the values below, and MUST reject every other value.

- An address that `std::parse_ip_address` accepts, followed by `/` and a prefix length.
- An address that `std::parse_ip_address` accepts, standing for the subnet of that address with a prefix length of 32 for IPv4 and of 128 for IPv6.

The prefix length MUST be written in decimal with no leading zero, and MUST be at most 32 for an IPv4 address and at most 128 for an IPv6 address.

`std::parse_ip_subnet` MUST reject a value whose host bits are not all zero.
Its error SHOULD name the subnet with those host bits cleared.

> [!NOTE]
> `10.0.0.5/24` in an allow-list is most often a typo or an interface address pasted in place of a network.
> Clearing the host bits would let it stand for `10.0.0.0/24` without the operator having written that.

## Ranges

`std::parse_ip_range(text)` MUST accept exactly the values below, and MUST reject every other value.

- Two addresses that `std::parse_ip_address` accepts, of the same family, joined by a single `-` with no whitespace, where the first is not greater than the second.
- A subnet that `std::parse_ip_subnet` accepts, standing for the range from its first address to its last.
- An address that `std::parse_ip_address` accepts, standing for the range holding only that address.

Addresses MUST be ordered as unsigned integers of their family's width.

## Family argument

Each of the three kinds MUST take an optional `family` [argument](action.md#kind-arguments), which is valid only if it is the integer `4` or the integer `6`.
Each of the three parsers MUST accept being called with or without `family`.
A parser given `family` MUST reject a value of the other family, and its error SHOULD state the family it expected.

> [!NOTE]
> A script that needs one family, such as an IPv4 address pool, declares it on the kind so that the operator interface and the parser's error say so before any validator runs.
