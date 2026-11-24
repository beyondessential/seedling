---
id: URL
---

# URLs and hostnames

Applies to the `url`, `url-origin` and `hostname` [param kinds](action.md#param-kinds), their [parsers](action.md#kind-parsers), and the values those parsers return.

## Borrowed terms

| term | meaning |
| --- | --- |
| A-label | the ASCII form of an internationalised domain name label, as defined by [RFC 5890](https://www.rfc-editor.org/rfc/rfc5890) section 2.3.2.1 |
| URL parser, host, host parser, host serialiser, opaque host, special scheme | as defined by the [URL Standard](https://url.spec.whatwg.org/) |
| IDNA processing | the domain name to ASCII conversion of [Unicode Technical Standard #46](https://www.unicode.org/reports/tr46/), nontransitional, with `CheckHyphens`, `CheckBidi` and `CheckJoiners` set, `UseSTD3ASCIIRules` set, and `VerifyDnsLength` set |

> [!NOTE]
> The URL Standard and UTS #46 are referenced by their current editions, and the parsers follow the editions the implementation tracks, so a Seedling release can change how an unusual value parses.

## Hostnames

`std::parse_hostname(text)` MUST return a string for a value it accepts.

`std::parse_hostname` MUST first convert the value by IDNA processing, and MUST reject it if that conversion fails.
The value, so converted, MUST then meet the rules for an [ingress hostname](ingress.md#hostnames), and MUST be rejected otherwise.
`std::parse_hostname` MUST also reject a value whose last label is made only of ASCII digits.

The result MUST be the value as converted, in lower case.

> [!NOTE]
> Every label of an internationalised name comes back as an A-label, which is ASCII letters, digits and `-`, so the result can be given to `service.ingress` as it is.

> [!NOTE]
> A name whose last label is all digits is read as an IPv4 address by URL parsers, and no top-level domain is all digits, so refusing it keeps IP addresses out of ingress hostnames.

## URLs

`std::parse_url(text)` MUST return an object map for a value it accepts.
It MUST parse the value with the URL parser, with no base URL, and MUST reject a value the URL parser fails on.
It MUST accept a URL of any scheme, including one with no host, such as `file:///etc/hosts` or `mailto:someone@example.org`.

> [!NOTE]
> The URL parser is the one browsers use, so a URL an operator copies from a browser's address bar parses as the browser read it.

### Components

| key | present | value |
| --- | --- | --- |
| `scheme` | always | the scheme, in lower case |
| `host` | when the URL's host is neither null nor the empty host | the host, as [hosts](#hosts) specify |
| `port` | when the URL gives a port or its scheme has a default port | the port, as [ports](#ports) specify |
| `path` | always | the path as the URL parser leaves it, possibly empty |
| `query` | when the URL has a non-empty query | the query as the URL parser leaves it, without its leading `?` |
| `fragment` | when the URL has a non-empty fragment | the fragment as the URL parser leaves it, without its leading `#` |
| `username` | when the URL has a non-empty username | the username, percent-decoded |
| `password` | when the URL has a non-empty password | the password, percent-decoded |

`std::parse_url` MUST NOT percent-decode the path, the query or the fragment.

> [!NOTE]
> A decoded query map would lose repeated keys and their order, so the query comes back whole and a script that needs its pairs splits them itself.

### Hosts

Where the URL's host is not an opaque host, `host` MUST be that host as the host serialiser writes it.

Where the URL's host is an opaque host, `std::parse_url` MUST give that host, as the URL parser leaves it, to the host parser, not as opaque.
Where that succeeds, `host` MUST be the host it gives, as the host serialiser writes it, and otherwise the opaque host.

> [!NOTE]
> The host serialiser writes a domain in ASCII and in lower case, with each internationalised label as an A-label, and an IPv6 address enclosed in `[` and `]`.
> The URL parser leaves the host of a URL whose scheme is not a special scheme opaque, so without the second parse `postgres://DB.Example.org` and `https://db.example.org` would carry different hosts for the same server.

### Userinfo

`std::parse_url` MUST reject a URL whose percent-decoded username or password is not valid UTF-8, or holds a character of a general category that [kind parsers](action.md#kind-parsers) reject in a value.

> [!NOTE]
> A password holding `@`, `:` or `/` can only be written percent-encoded in a URL, and a script passing it on needs the password itself.

### Ports

`port` MUST be the URL's port, as an integer, where it gives one, and otherwise the [default port](https://url.spec.whatwg.org/#default-port) of its scheme.
`std::parse_url` MUST reject a URL whose port is 0, as the [port rules](overview.md#ports) require.

> [!NOTE]
> Only the URL Standard's special schemes have a default port: `http` and `ws` 80, `https` and `wss` 443, and `ftp` 21.

## URL origins

`std::parse_url_origin(text)` MUST return an object map of `scheme`, `host` and `port` for a value it accepts, each as [`std::parse_url`](#components) gives it.
It MUST reject a value `std::parse_url` rejects.
It MUST also reject a URL whose scheme is not `http` or `https`, that has a username, a password, a query or a fragment, even an empty one, or whose path is anything but `/`.

> [!NOTE]
> The URL parser gives an `http` or `https` URL written with no path the path `/`, so `https://example.org` and `https://example.org/` are both accepted.

`host` MUST meet what [`std::parse_hostname`](#hostnames) accepts, and MUST be what it returns for that host.
A URL origin whose host is an IP address MUST be rejected.
