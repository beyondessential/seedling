---
id: PRX
---

# Proxy

Applies to the proxy through which the [runtime](../overview.md#runtime) serves ingress traffic, the HTTP routes the runtime configures on it, and the proxy's own lifecycle on a [host](../overview.md#host).

## Borrowed terms

| term | meaning |
| --- | --- |
| request-target | The target of a request as it appears on the request line, [RFC 9112 §3.2](https://www.rfc-editor.org/rfc/rfc9112#section-3.2). |
| query | The query component of a URI, [RFC 3986 §3.4](https://www.rfc-editor.org/rfc/rfc3986#section-3.4). |
| content coding | An encoding applied to a representation, named by `Content-Encoding`, [RFC 9110 §8.4](https://www.rfc-editor.org/rfc/rfc9110#section-8.4). |
| persistent connection | An HTTP/1.1 connection reused for successive requests, [RFC 9112 §9.3](https://www.rfc-editor.org/rfc/rfc9112#section-9.3). |
| connection-specific field | A header field describing the connection rather than the message, such as `Connection` or `Keep-Alive`, [RFC 9110 §7.6.1](https://www.rfc-editor.org/rfc/rfc9110#section-7.6.1). |
| QUIC | The UDP-based transport that carries HTTP/3, [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000). |
| redirect | An HTTP response with a 3xx status code, [RFC 9110 §15.4](https://www.rfc-editor.org/rfc/rfc9110#section-15.4). |

## Terminology

### Proxied route

A route on an ingress that forwards each request it claims to one of its upstreams.

### Redirect route

A route that answers each request it claims with a redirect, declared on an [HTTP service route](../language/http-service.md#route-redirects) rather than bound by a pod.

### Upstream

A backend address, with its port, to which a proxied route forwards requests.

### Fallback route

The proxied route at `/` through which a service is served when no pod binds any of its prefixes.

### Normalised path

A request path with duplicate `/` separators collapsed, dot segments removed as in [RFC 3986 §5.2.4](https://www.rfc-editor.org/rfc/rfc3986#section-5.2.4), percent-encoding decoded, and letter case folded.

## Ingress serving

### Port scope

The runtime MUST serve an ingress on the port it was declared on and on no other.
The runtime MUST route a request to an ingress's target only when the request arrives on that ingress's port, whatever hostname the request names.

Two ingresses for the same hostname on different ports MUST be served independently, each port serving the routes of the ingress declared on it.
A redirect an ingress declares MUST answer only on the port it names.

An ingress's [HTTP redirect](../language/ingress.md#redirect-to-https) claims the ingress's hostname on the port the redirect names.
On that port, the proxy MUST answer an ACME HTTP-01 challenge request for a certificate it is acquiring, ahead of the redirect.
That claim and a plaintext ingress's claim on the same hostname and port conflict, and the runtime MUST resolve the conflict as [claims across apps](../language/ingress.md#claims-across-apps) and [ingress conflicts](site-resources.md#ingress-conflicts) specify.

### Plaintext ingresses

An ingress that terminates HTTP without terminating TLS is a plaintext ingress.
A plaintext ingress is an `http` or `http2` [attachment](site-resources.md#attachments) on a site ingress whose TLS provisioning mode is `none`.
An app's ingress terminates HTTP only together with TLS, as [termination](../language/ingress.md#termination) specifies.

The runtime MUST serve a plaintext ingress as HTTP without TLS on its port, routing matching requests to the ingress's target.
For a plaintext ingress, the runtime MUST NOT request or provision a certificate for the hostname.
It MUST NOT accept QUIC connections for a plaintext ingress.
It MUST NOT answer a plaintext ingress's requests with a redirect the ingress does not itself declare.

The runtime MUST forward to an ingress only the transport protocols the ingress serves.
A plaintext HTTP ingress therefore MUST NOT cause UDP traffic on its port to be forwarded.

A hostname MAY carry plaintext and TLS-terminating ingresses on different ports.
The runtime MUST serve each from a listener matching its own termination.
A TLS-terminating ingress on a hostname MUST NOT cause a plaintext ingress on the same hostname to be dropped or served only over TLS.

## Route emission

The runtime MUST emit routes for every HTTP-terminating [ingress](../language/ingress.md), whether declared by an app or attached as a [site ingress](site-resources.md#attachments), from the service the ingress fronts.
The routes MUST be the same whichever kind of ingress fronts the service.
Each route's settings MUST be resolved against the app that declares the service.

For each prefix that a [pod binding](../language/pod.md#port-bindings) on any pod of the declaring app binds to the service, the runtime MUST emit one proxied route at that prefix, however many pods bind it.
Its upstreams MUST be the addresses of the running instances of every pod binding the prefix, each at the pod port of that pod's binding.
A prefix none of whose binding pods has a running instance MUST still be emitted, with no upstreams.

If no pod binds any prefix of the service, the runtime MUST emit a fallback route.
Its single upstream MUST be the service's address, through the service's [routing pool](lifecycle.md#routing-pool), at the [Service Port](../language/service.md#service-ports) on which the service is viewed as an [HTTP service](../language/http-service.md#declaration).
Where the service is viewed as an HTTP service on several Service Ports, the fallback route's upstream MUST be at the lowest-numbered of them.
Where the service is not viewed as an HTTP service on any Service Port, the fallback route's upstream MUST be at the ingress's own port.

The runtime MUST emit each redirect route the service declares, whether or not any pod binds a prefix of the service.
A redirect route MUST NOT displace the fallback route.

## Route selection

The proxy MUST serve each request on an ingress by exactly one route: the route with the longest prefix among those that claim the request.
A route that claims a request MUST answer it, whether or not it has upstreams.
A proxied route with no upstreams MUST answer with status 503 (Service Unavailable), [RFC 9110 §15.6.4](https://www.rfc-editor.org/rfc/rfc9110#section-15.6.4).

A proxied route MUST claim every request whose normalised path begins with its prefix.
A redirect route MUST claim a request whose normalised path equals its prefix or begins with its prefix followed by `/`.
Prefixes MUST be compared with the normalised path, so that requests differing only in the respects normalisation removes are claimed by the same route.

> [!NOTE]
> A proxied route claims on characters, so `/api` claims `/apis`.
> A redirect route claims on segment boundaries, because the remainder of the path is spliced into the `Location` it answers with: under `/v1/login`, a request for `/v1/login.example.net` would otherwise contribute a remainder that names a host.

On the plaintext listener of an ingress that declares an [HTTP redirect](../language/ingress.md#redirect-to-https), that redirect MUST answer every request, and the ingress's other routes MUST NOT be served there.

> [!NOTE]
> A plaintext request under a redirect route's prefix is therefore moved to HTTPS first and meets the redirect route on arrival.
> The extra round trip keeps a client from being handed the redirect's target before it has been moved to TLS.

## Redirect routes

A redirect route MUST answer with its declared status code ([RFC 9110 §15.4](https://www.rfc-editor.org/rfc/rfc9110#section-15.4)) and a `Location` field ([RFC 9110 §10.2.2](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.2)) built from its declared target.
It MUST NOT forward the request to any pod.

The runtime MUST substitute the target's `<tail>` and `<query>` from the request-target as it arrived, without decoding or normalising it:

| token | value |
| --- | --- |
| `<tail>` | the part of the request-target's path after the prefix, up to the first `?`; empty when the path is exactly the prefix |
| `<query>` | `?` and everything after it; empty when the request-target carries no `?` |

The prefix MUST be compared with the request-target case-insensitively for this extraction, as it is for route selection.
If the request-target as it arrived does not begin with the prefix, both values MUST be empty.
If the request-target contains `{` or `}`, both values MUST be empty.
A target naming neither token MUST be served as declared.

> [!NOTE]
> Reading the tail from the decoded path would let a `%3F` in a path segment reach the client as the `?` that opens a query.
> A brace is excluded because a proxy may read a braced word as a placeholder naming its own state, the reason a [header value](../language/http-service.md#header-operations) refuses one, and the request-target is written by the client rather than the app.

## Settings scope

Compression, balancing and rate limiting MUST apply to proxied routes only.
Request header operations MUST apply to proxied routes only.
Response header operations MUST apply to proxied routes and redirect routes.

The proxy MUST NOT apply compression, balancing, rate limiting or header operations to the response of an ingress redirect or of a site ingress redirect attachment, or to non-HTTP forwarding.

## Compression

Every proxied route MUST compress the responses it serves according to its resolved [compression settings](../language/http-service.md#compression).
Where a client's `Accept-Encoding` ([RFC 9110 §12.5.3](https://www.rfc-editor.org/rfc/rfc9110#section-12.5.3)) gives several of the route's encodings equal preference, the proxy SHOULD choose the earliest in the route's declared order.

The proxy MUST forward a response that already carries a content coding as it stands, without applying a further one.

## Balancing

Every proxied route MUST distribute requests across its upstreams according to its resolved [balancing settings](../language/http-service.md#balancing).

Within the route's try duration, a request whose connection to an upstream cannot be established MUST be attempted against another upstream from the route's pool, whatever the request's method.
The proxy MUST wait the route's interval between attempts.

A request whose connection was established but which received no response, because the upstream closed the connection or failed before responding, MUST be attempted again only if its method is `GET`.
A response carrying an error status MUST be returned to the client as it stands.

> [!NOTE]
> A request that never connected cannot have been acted on, so retrying it is safe for any method.
> Retrying on another upstream lets a request arriving while an instance shuts down during a [rolling update](update-strategies.md#rolling-updates) reach a surviving instance.
> On a fallback route, with its single upstream, the policy has no effect but the try duration and interval still do.

The proxy MUST NOT remove an upstream from a route's pool on the strength of its own health checks.
The upstreams a route is given MUST be the only judgement of which backends are eligible.

> [!NOTE]
> Eligibility is decided by the routing pool from the runtime's own [healthchecks](../language/deployment.md#health-checks), and the pool keeps every running backend in service when no healthy one exists.
> Health checking in the proxy would contradict the pool by reducing capacity to nothing in that case.
> Retrying an unreachable upstream covers only the interval between a backend becoming unusable and the pool being recomputed.

## Rate limiting

A proxied route with a resolved [rate limit](../language/http-service.md#rate-limiting) MUST reject a client's requests beyond that limit before proxying them.
A route whose resolved limit is absent MUST NOT be rate limited.

A limit declared on a route MUST be one budget for that route.
A limit declared on a service and inherited by several of its routes MUST be one budget shared by those routes.
A budget MUST NOT be divided by the hostname, port, ingress or site ingress a request arrived through.

A client MUST be identified by the IP address the proxy attributes to the request.
All IPv6 addresses within one /64 MUST be counted as one client.
Each IPv4 address MUST be counted as one client.

> [!NOTE]
> A party holds its whole IPv6 /64, so counting its addresses separately would give it one budget per address and the limit would not bind it.

The proxy MUST allow each client the limit's `max_events` requests within any sliding window of the limit's `window`.
It MUST answer a request beyond that with status 429 ([RFC 6585 §4](https://www.rfc-editor.org/rfc/rfc6585#section-4)) and a `Retry-After` field ([RFC 9110 §10.2.3](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3)) indicating when the client may retry.

A request MUST be counted only against the budget of the route that serves it, as [route selection](#route-selection) chooses it.

> [!NOTE]
> A tighter limit on a longer prefix therefore governs its own traffic independently of a looser limit on a shorter prefix covering the rest.
> The proxy's normalisation may be broader than a backend's, and where the two disagree a request is counted against the more specific prefix while the backend treats it as belonging to a less specific one, which over-applies the stricter limit rather than escaping it.

## Header operations

Every proxied route MUST apply its resolved request [header operations](../language/http-service.md#header-operations) to each request before proxying it, and its resolved response operations to each response before returning it.
A redirect route MUST apply its resolved response operations to the redirect it serves.

A request operation MUST reach the upstream.
A request operation replacing `Host` MUST change the host the upstream receives ([RFC 9110 §7.2](https://www.rfc-editor.org/rfc/rfc9110#section-7.2)), whatever hostname the client used.

A response operation MUST be applied to every response the route serves, including a response the proxy produces in the upstream's place, such as a rate-limit rejection or a failure to reach any upstream.

The proxy MUST apply each operation's values literally, without substituting anything into them.
A header carrying several values MUST reach the other side as several field lines, not folded into one.

The runtime MUST NOT apply to a redirect route a response operation naming `Location` that the route inherits from its service.
A redirect route's `Location` MUST be the one computed from its target.

## Settings inspection

The resolved compression, balancing, rate-limit and header settings of a service and of each of its routes MUST be readable when [describing the app](../interface/apps.md#proxy-settings) that declares the service.
A route's proxy settings MUST come only from the app's definition.

## Persistent connections

The proxy MUST keep an HTTP/1.1 connection to an ingress persistent, serving successive requests on it rather than closing it after each exchange.
The proxy MUST hold an idle HTTP/1.1 connection open long enough for a client to reuse it.
It SHOULD hold one open for at least five minutes.

The proxy MUST NOT add connection-specific fields to responses to keep a connection persistent.

> [!NOTE]
> Connection-specific fields make a response malformed on HTTP/2 ([RFC 9113 §8.2.2](https://www.rfc-editor.org/rfc/rfc9113#section-8.2.2)) and HTTP/3 ([RFC 9114 §4.2](https://www.rfc-editor.org/rfc/rfc9114#section-4.2)).
> An app [cannot declare them](../language/http-service.md#header-operations) as header operations, and needs no declaration for persistence.

## Proxy lifecycle

The runtime MUST attempt to bring the proxy up when it starts.
A failure to bring the proxy up MUST NOT prevent the runtime from starting.
The [Operator Interface](../overview.md#operator-interface), database, scheduler and reconciliation loop MUST come up whether or not the proxy does.

While any app is installed or installing, the runtime MUST check the proxy's health on each [reconciliation](reconciliation.md#reconciliation-loop) iteration and restart it if it is not running or not healthy.
The proxy is healthy when it answers the runtime's health query on its administrative interface.
The runtime MUST stop the proxy when no app is installed or installing.

On an iteration that fails to bring the proxy up, or does not finish doing so, the runtime MUST file a [`caddy_failed`](faults.md#host-faults) fault, and MUST clear it as that fault's row specifies.

## Proxy image

The proxy image the runtime runs MUST accept, in full, every configuration the runtime can emit.

> [!NOTE]
> The proxy rejects a configuration as a whole, so a component or field the image lacks does not degrade one route.
> It leaves every ingress on the host without service, or aborts an [upgrade](#proxy-upgrades) and leaves the proxy on its previous configuration indefinitely.

The runtime MUST declare the set of proxy components its configuration can reference.
Before a proxy image is published, and whenever the runtime's configuration output changes, the image MUST be checked against that declaration.
It MUST also be checked, with the proxy's own validation, against configurations the runtime emits for every feature it can emit.
That check MUST be shown to reject a configuration carrying a field the image does not accept.

The proxy MUST store the certificates it obtains where the runtime [reads them](observation.md#ingresses).

> [!NOTE]
> A proxy that obtains certificates but stores them elsewhere serves traffic correctly while the runtime reports that no certificate exists.
> The [certificate metadata](tls/certificates.md#hostname-state) is then missing too, and every [`rt.warm_certs`](../language/rt.md#certificate-warming) barrier waiting on the certificate stalls.

## Proxy upgrades

If the image of the running proxy differs from the proxy image the runtime runs, the runtime MUST replace the proxy with an instance of that image.
The runtime MUST start the replacement alongside the running proxy and apply the last successfully applied configuration to it.
The runtime MUST direct traffic to the replacement only after the replacement has accepted that configuration.

The runtime MUST persist which proxy instance is active.
It MUST record the replacement as active only after the replacement has accepted the configuration.
On startup, the runtime MUST treat the persisted active instance as authoritative, whatever point an interrupted upgrade reached.

The runtime MUST persist the last configuration it successfully applied to the proxy.
The persisted configuration MUST be in the runtime's own form, rendered into the proxy's configuration format each time it is applied.
A configuration persisted by an earlier version of the runtime MUST remain applicable, reproducing the serving behaviour that version configured.
The runtime SHOULD also apply the persisted configuration to a proxy it starts afresh.

> [!NOTE]
> A configuration persisted in the proxy's own format would be invalidated by an upgrade that changes that format.

If the replacement rejects the persisted configuration, the runtime MUST abort the upgrade.
It MUST stop the replacement, leave the running proxy active and serving, and not record the replacement as active.
It MUST file the [`caddy_failed`](faults.md#host-faults) fault naming the rejection, and retry the upgrade on a later iteration.
