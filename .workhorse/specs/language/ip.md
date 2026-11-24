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
| canonical IPv6 text | the text representation of an IPv6 address recommended by [RFC 5952](https://www.rfc-editor.org/rfc/rfc5952) section 4 |

## Terminology

### Family

Whether an address, subnet or range is of IPv4 or of IPv6.

### Host bits

The bits of a subnet's address after its prefix length.

## Addresses

`std::parse_ip_address(text)` MUST return an `IpAddress` for a value it accepts.
It MUST accept exactly the values below, and MUST reject every other value.

- An IPv4 address, written as four decimal numbers from 0 to 255 separated by dots, none with a leading zero.
- An IPv6 address in the IPv6 text form, with no enclosing brackets and no zone index, that is not an IPv4-mapped IPv6 address.

Hexadecimal digits in an IPv6 address MUST be accepted in either case.

> [!NOTE]
> A zone index names an interface on one host, which means nothing to a workload on a pod network.
> An IPv4-mapped address in configuration is most often a dual-stack socket's view of an IPv4 peer, copied where the IPv4 address was meant.

## Subnets

`std::parse_ip_subnet(text)` MUST return an `IpSubnet` for a value it accepts.
It MUST accept exactly the values below, and MUST reject every other value.

- An address that `std::parse_ip_address` accepts, followed by `/` and a prefix length.
- An address that `std::parse_ip_address` accepts, standing for the subnet of that address with a prefix length of 32 for IPv4 and of 128 for IPv6.

The prefix length MUST be written in decimal with no leading zero, and MUST be at most 32 for an IPv4 address and at most 128 for an IPv6 address.

`std::parse_ip_subnet` MUST reject a value whose host bits are not all zero.
Its error SHOULD name the subnet with those host bits cleared.

> [!NOTE]
> `10.0.0.5/24` in an allow-list is most often a typo or an interface address pasted in place of a network.
> Clearing the host bits would let it stand for `10.0.0.0/24` without the operator having written that.

## Ranges

`std::parse_ip_range(text)` MUST return an `IpRange` for a value it accepts.
It MUST accept exactly the values below, and MUST reject every other value.

- Two addresses of the same family joined by a single `-` with no whitespace, where the first is not greater than the second, and that do not both lie within `::ffff:0:0/96`.
  Each address is written as `std::parse_ip_address` requires, except that it MAY be an IPv4-mapped IPv6 address.
- A subnet that `std::parse_ip_subnet` accepts, standing for the range from its first address to its last.
- An address that `std::parse_ip_address` accepts, standing for the range holding only that address.

Addresses MUST be ordered as unsigned integers of their family's width.

> [!NOTE]
> An IPv6 subnet that holds `::ffff:0:0/96`, such as `::/80`, ends on an IPv4-mapped address, so a range written as two addresses may end on one, which lets that range's canonical text parse again.
> A range lying wholly among IPv4-mapped addresses is refused as a mapped address is.

## Family argument

Each of the three kinds MUST take an optional `family` [argument](action.md#kind-arguments), which is valid only if it is the integer `4` or the integer `6`.
Each of the three parsers MUST accept being called with or without `family`.
A parser given `family` MUST reject a value of the other family, and its error SHOULD state the family it expected.

> [!NOTE]
> A script that needs one family, such as an IPv4 address pool, declares it on the kind so that the operator interface and the parser's error say so before any validator runs.

## Address values

| method | returns |
| --- | --- |
| `family()` | `4` or `6`, as an integer |
| `to_subnet()` | the `IpSubnet` holding only the address, with a prefix length of 32 for IPv4 and of 128 for IPv6 |
| `to_subnet(prefix_len: int)` | the `IpSubnet` of that prefix length that contains the address |
| `to_range()` | the `IpRange` holding only the address |

`to_subnet(prefix_len)` MUST clear the address's bits after `prefix_len`.
It MUST throw when `prefix_len` is negative, or exceeds 32 for an IPv4 address or 128 for an IPv6 address.

> [!NOTE]
> The parser refuses host bits because an operator who typed them may have meant another network.
> A script calling `to_subnet` with a prefix length asks for the network the address sits in, so clearing them is the operation it requested.

Two `IpAddress` values of the same family MUST compare by the [address order](#ranges) through `<`, `<=`, `>`, `>=`, `==` and `!=`.
For two `IpAddress` values of different families, `==` MUST return `false` and `!=` MUST return `true`, and `<`, `<=`, `>` and `>=` MUST raise a script error.

## Subnet values

| method | returns |
| --- | --- |
| `family()` | `4` or `6`, as an integer |
| `address()` | the subnet's first address, as an `IpAddress` |
| `prefix_len()` | the subnet's prefix length, as an integer |
| `contains(other)` | whether every address of `other` is in the subnet |
| `overlaps(other)` | whether at least one address of `other` is in the subnet |
| `to_range()` | the `IpRange` from the subnet's first address to its last |

Two `IpSubnet` values MUST be equal through `==` and `!=` when they have the same family, address and prefix length.

## Range values

| method | returns |
| --- | --- |
| `family()` | `4` or `6`, as an integer |
| `start()` | the range's first address, as an `IpAddress` |
| `end()` | the range's last address, as an `IpAddress` |
| `contains(other)` | whether every address of `other` is in the range |
| `overlaps(other)` | whether at least one address of `other` is in the range |

Two `IpRange` values MUST be equal through `==` and `!=` when they have the same family, first address and last address.

## Containment and overlap

`contains` and `overlaps` MUST accept an `IpAddress`, an `IpSubnet` or an `IpRange` as `other`, and MUST raise a script error for any other value, including a string.
Both MUST return `false` when `other` is of the other family.

> [!NOTE]
> Taking only parsed values means a script parses an operator's text once, with the kind's own rules, rather than having each call read a string by rules of its own.

## Text of a value

An `IpAddress`, `IpSubnet` or `IpRange` MUST convert to a string in its canonical form:

- an IPv4 address as four decimal numbers separated by dots, none with a leading zero;
- an IPv6 address as canonical IPv6 text;
- a subnet as its first address, `/`, and its prefix length in decimal, whether or not the prefix length was written;
- a range as its first address, `-`, and its last address, whether it was written as two addresses, a subnet or a single address.

The parser of a value's kind MUST accept its canonical form, and MUST return a value equal to it.

> [!NOTE]
> These values are most often handed on to software with a parser of its own, such as a proxy's trusted addresses or a DHCP server's pool, which may not accept every spelling an operator can enter.
> `10.0.0.0/30` taken as a range is written `10.0.0.0-10.0.0.3`.

## Mixing types

Comparing an `IpAddress`, `IpSubnet` or `IpRange` with a value of any other type, including another of these three types, MUST raise a script error.
