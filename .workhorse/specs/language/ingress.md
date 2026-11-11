---
id: ING
---

# Ingresses

## Ingress

<!-- from language: Ingress -->

> l[ingress.type]
> An Ingress is an externally-accessible endpoint to the application.
>
> Ingresses are created from [Services](rule:l/service.type) using the `service.ingress(hostname: string, port: number)` instance method, which returns a [builder](rule:l/bsl.builder).
>
> Traffic from an Ingress is matched by the hostname and port, and sent to the associated Service at the same port.
>
> There can be multiple ingresses for a Service.
>
> Only a named Service can have an ingress: calling `ingress` on an anonymous Service must throw, and no Ingress is created.

> l[ingress.hostname]
> The `hostname` argument must be a valid fully-qualified domain name: one or more labels separated by `.`, where each label is 1–63 ASCII alphanumeric or hyphen characters, must not start or end with a hyphen, and the overall length (including dots) must not exceed 253 characters.
>
> Wildcard labels (e.g. `*.example.com`) are not permitted. If wildcard matching is needed in the future it will be designed as a separate feature.
>
> If the hostname is not valid, the method must throw.
>
> A DNS name is case-insensitive, so the hostname is held in lower case whatever case it is written in, and everything derived from it — the ingress's identity, the proxy's configuration, the name a certificate is obtained for — uses that one form.

> l[ingress.conflicts]
> If more than one ingress matches the same (hostname, port) tuple...
> - ...within the same application: the latter definition in execution order will throw, and not be registered against the ingress. This can be caught (with `try..catch`) and handled.
> - ...between two or more applications: this is a control plane concern.

> l[ingress.certificates]
> This rule applies to all ingress spec rules that deal with certificates.
>
> Certificates will be automatically obtained whenever possible for the ingress's hostnames. The application will not have access to the key material.

> l[ingress.service]
> The `ingress.service()` instance method returns the Service that the ingress was created from.

> l[ingress.termination]
> The `ingress.tls(terminate: Terminate, output: Output)` builder method declares both what is terminated at the edge and what protocol the ingress hands to the bound Service.
>
> Exactly four `(terminate, output)` pairings are accepted; any other pairing must throw:
>
> | `terminate`        | `output`        | Behaviour |
> | ------------------ | --------------- | --------- |
> | `Terminate.Tls`    | `Output.Tcp`    | Terminate TLS for incoming TCP, re-emit as plaintext TCP. The ingress does not interpret the application protocol; non-TLS TCP traffic is rejected. |
> | `Terminate.Dtls`   | `Output.Udp`    | Terminate DTLS for incoming UDP, re-emit as plaintext UDP. Non-DTLS UDP traffic is rejected. |
> | `Terminate.Https`  | `Output.Http1`  | Terminate HTTPS (HTTP/1.1 and HTTP/2 over TCP, HTTP/3 over UDP), re-emit as plaintext HTTP/1.1. Non-HTTP traffic is rejected. |
> | `Terminate.Https`  | `Output.Http2`  | Terminate HTTPS, re-emit as plaintext HTTP/2 (`h2c`). Non-HTTP traffic is rejected. |
>
> Calling `tls()` without arguments, or with any other pairing, must throw with a clear message naming the supplied pair and the valid set.
>
> An ingress with no termination call is left as plain TCP passthrough.

> l[const.terminate.tls]
> The `Terminate.Tls` constant tags an ingress termination mode that strips TLS from incoming TCP traffic and exposes the inner bytes as plaintext TCP, without interpreting any application protocol.

> l[const.terminate.dtls]
> The `Terminate.Dtls` constant tags an ingress termination mode that strips DTLS from incoming UDP traffic and exposes the inner datagrams as plaintext UDP.

> l[const.terminate.https]
> The `Terminate.Https` constant tags an ingress termination mode that terminates HTTPS (HTTP/1.1 and HTTP/2 for TCP, HTTP/3 for UDP) and re-emits HTTP traffic to the bound Service.

> l[const.output.tcp]
> The `Output.Tcp` constant tags an ingress output protocol of plaintext TCP.

> l[const.output.udp]
> The `Output.Udp` constant tags an ingress output protocol of plaintext UDP datagrams.

> l[const.output.http1]
> The `Output.Http1` constant tags an ingress output protocol of plaintext HTTP/1.1.

> l[const.output.http2]
> The `Output.Http2` constant tags an ingress output protocol of plaintext HTTP/2 (`h2c`).

> l[ingress.redirect]
> The `ingress.redirect(port?: number, code?: number)` builder method emits an HTTP redirect on the `port` given if and when the ingress has obtained a TLS certificate.
> The redirect sends the client to the same host and request path over HTTPS on the ingress's own port, whatever other HTTPS ports the node listens on.
>
> The `port` defaults to 80.
> The `code` defaults to 307 ([Temporary Redirect](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/307)) and must be a redirection status code in the range 300–399; any other value throws.
>
> Calling this on an ingress whose termination is not `Terminate.Https` throws.
