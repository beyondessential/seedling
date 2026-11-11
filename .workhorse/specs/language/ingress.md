---
id: ING
---

# Ingresses

Applies to the BSL `Ingress` type, through which an app exposes one of its services outside the [host](../overview.md#host).

## Borrowed terms

| term | meaning |
| --- | --- |
| TLS | Transport Layer Security, as defined by [RFC 9846](https://www.rfc-editor.org/rfc/rfc9846) |
| DTLS | Datagram Transport Layer Security, as defined by [RFC 9147](https://www.rfc-editor.org/rfc/rfc9147) |
| HTTPS | HTTP over TLS, as defined by [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) §4.2.2 |
| HTTP/1.1 | as defined by [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112) |
| HTTP/2 | as defined by [RFC 9113](https://www.rfc-editor.org/rfc/rfc9113); in cleartext, started with prior knowledge as in §3.3 |
| HTTP/3 | as defined by [RFC 9114](https://www.rfc-editor.org/rfc/rfc9114) |
| label | one dot-separated component of a domain name, as defined by [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035) §2.3.1 and [RFC 1123](https://www.rfc-editor.org/rfc/rfc1123) §2.1 |

## Terminology

### Ingress

An externally reachable endpoint of an app, identified by a hostname and a port, that delivers traffic to one of the app's services.

### TLS-terminating ingress

An ingress whose [termination](#termination) is `Terminate.Tls`, `Terminate.Dtls` or `Terminate.Https`.

## Declaration

A script declares an ingress with `service.ingress(hostname: string, port: number)`, which returns an `Ingress` [builder](overview.md#builder).
`httpService.ingress(hostname, port)` is equivalent, as [HTTP services](http-service.md#declaration) specifies.
A `port` MUST be validated as a [port number](overview.md#ports).

`ingress` called on an anonymous Service MUST throw, and MUST declare no ingress.
A Service MAY have any number of ingresses.

Where an ingress's [termination](#termination) is `Terminate.Tls` or `Terminate.Dtls`, or it declares none, the runtime MUST deliver traffic arriving at the ingress's hostname and port to the ingress's Service, at the same port number.
The runtime MUST deliver traffic arriving at an ingress whose termination is `Terminate.Https` through the Service's routes, as [route emission](../runtime/proxy.md#route-emission) specifies.

`ingress.service()` returns the Service the ingress was declared on.

### Identity within an app

An ingress is identified within its app by its hostname and port.
An ingress's name MUST be its [hostname](#hostnames) in lower case, followed by `:` and its port in decimal, such as `example.org:443`.
Declaring an ingress whose hostname and port match an ingress already declared in the same app MUST throw, and MUST leave the earlier ingress as it was.

> [!NOTE]
> A script may catch the error with `try`/`catch`, as [evaluation](overview.md#evaluation) allows.

### Claims across apps

When ingresses of two apps claim the same hostname and port, the runtime MUST serve the ingress of the app whose install completed first, and MUST leave the other out of the proxy configuration.
The runtime MUST file an [`ingress_conflict`](../runtime/faults.md#ingress-and-service-faults) fault against the app whose ingress it leaves out.
Once the served ingress no longer claims the hostname and port, the runtime MUST serve the other.
How an app's ingress and a site ingress attachment claiming the same hostname and port are handled is specified by [ingress conflicts](../runtime/site-resources.md#ingress-conflicts).

## Hostnames

A `hostname` MUST meet all of the following, and any other hostname MUST throw:

- it is 1 to 253 characters long;
- it is one or more [labels](#borrowed-terms) separated by `.`;
- each label is 1 to 63 characters long, of ASCII letters, digits and `-`;
- no label begins or ends with `-`.

A hostname containing `*` MUST throw.

The runtime MUST hold a hostname in lower case, whatever case the script wrote it in.
The ingress's identity, the proxy's configuration and the name a certificate is obtained for MUST all use that lower-case form.

## Termination

`ingress.tls(terminate: Terminate, output: Output)` declares the protocol the ingress terminates and the protocol it hands to its Service.
Exactly four pairings are valid:

| `terminate` | `output` | the ingress accepts | the Service receives |
| --- | --- | --- | --- |
| `Terminate.Tls` | `Output.Tcp` | TLS over TCP | the plaintext TCP stream inside the TLS session |
| `Terminate.Dtls` | `Output.Udp` | DTLS over UDP | the plaintext datagrams inside the DTLS session |
| `Terminate.Https` | `Output.Http1` | HTTP/1.1 and HTTP/2 over TLS on TCP, and HTTP/3 on UDP | plaintext HTTP/1.1 |
| `Terminate.Https` | `Output.Http2` | HTTP/1.1 and HTTP/2 over TLS on TCP, and HTTP/3 on UDP | cleartext HTTP/2 |

Any other pairing MUST throw.
The error MUST name the pairing supplied and the four valid pairings.
A second `tls()` call MUST replace the first.

An ingress terminating TLS or DTLS MUST NOT interpret the application protocol inside it.
An ingress with a termination MUST reject traffic that is not of the protocol it accepts.

An ingress on which `tls()` is never called MUST forward TCP connections to its Service unmodified, and MUST NOT forward UDP traffic.

### Terminate

`Terminate` is an [enum](overview.md#enums) of the protocols an ingress terminates.

| variant | terminates |
| --- | --- |
| `Tls` | TLS on TCP |
| `Dtls` | DTLS on UDP |
| `Https` | HTTPS, as HTTP/1.1 and HTTP/2 on TCP and HTTP/3 on UDP |

### Output

`Output` is an [enum](overview.md#enums) of the protocol an ingress hands to its Service.

| variant | protocol |
| --- | --- |
| `Tcp` | plaintext TCP |
| `Udp` | plaintext UDP datagrams |
| `Http1` | plaintext HTTP/1.1 |
| `Http2` | cleartext HTTP/2 |

## Redirect to HTTPS

`ingress.redirect()`, `ingress.redirect(port: number)` and `ingress.redirect(port: number, code: number)` declare that plaintext HTTP on `port` is redirected to the ingress.
The `port` defaults to 80, and MUST be validated as a [port number](overview.md#ports).
The `code` defaults to 307, and MUST be one of 301, 302, 307 or 308, as for [route redirect codes](http-service.md#redirect-codes).
Any other `code` MUST throw.

A definition in which an ingress declares a redirect and, once the definition has evaluated, its termination is not `Terminate.Https`, MUST fail to evaluate, with an error naming the ingress.
The order of the `tls()` and `redirect` calls MUST NOT matter.

The runtime MUST answer every plaintext HTTP request for the ingress's hostname on `port` with a redirect of `code`, except an ACME HTTP-01 challenge request, which the [ingress proxy](../runtime/proxy.md#ingress-serving) answers for a certificate it is acquiring.
The redirect's `Location` MUST be `https://`, the request's hostname without its port, the ingress's own port preceded by `:` unless that port is 443, and the request's path and query string.

> [!NOTE]
> The redirect always targets the ingress's own port, whatever other HTTPS ports the host listens on.

## Certificates

Whenever it is able to, the runtime MUST obtain a certificate for the hostname of every TLS-terminating ingress, as [certificates](../runtime/tls/certificates.md) specifies.
The runtime MUST NOT make a certificate's private key available to the app.
