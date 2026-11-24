---
id: URL
---

# URLs and hostnames

Applies to the `url`, `url-origin` and `hostname` [param kinds](action.md#param-kinds), their [parsers](action.md#kind-parsers), and the values those parsers return.

## Borrowed terms

| term | meaning |
| --- | --- |
| A-label | the ASCII form of an internationalised domain name label, as defined by [RFC 5890](https://www.rfc-editor.org/rfc/rfc5890) section 2.3.2.1 |
| IDNA processing | the domain name to ASCII conversion of [Unicode Technical Standard #46](https://www.unicode.org/reports/tr46/), nontransitional, with `CheckHyphens`, `CheckBidi` and `CheckJoiners` set, `UseSTD3ASCIIRules` set, and `VerifyDnsLength` set |

## Hostnames

`std::parse_hostname(text)` MUST return a string for a value it accepts.

A value holding any character outside ASCII MUST first be converted by IDNA processing, and MUST be rejected if that conversion fails.
The value, so converted, MUST then meet the rules for an [ingress hostname](ingress.md#hostnames), and MUST be rejected otherwise.
`std::parse_hostname` MUST also reject a value whose last label is made only of ASCII digits.

The result MUST be the value as converted, in lower case.

> [!NOTE]
> Every label of an internationalised name comes back as an A-label, which is ASCII letters, digits and `-`, so the result can be given to `service.ingress` as it is.

> [!NOTE]
> A name whose last label is all digits is read as an IPv4 address by URL parsers, and no top-level domain is all digits, so refusing it keeps IP addresses out of ingress hostnames.

## URLs

### Hosts

The `host` a parsed URL carries MUST be in ASCII and in lower case.
An internationalised domain name MUST be converted by IDNA processing, and the URL MUST be rejected if that conversion fails.
An IPv6 address MUST be carried enclosed in `[` and `]`, as it is written in a URL.

### Query

The `query` a parsed URL carries MUST be the URL's query as written, without its leading `?`, and MUST NOT be decoded.

> [!NOTE]
> A decoded map would lose repeated keys and their order, so the query comes back whole and a script that needs its pairs splits them itself.

## URL origins

The `host` of a parsed URL origin MUST meet what [`std::parse_hostname`](#hostnames) accepts, and MUST be what it returns for that host.
A URL origin whose host is an IP address MUST be rejected.
