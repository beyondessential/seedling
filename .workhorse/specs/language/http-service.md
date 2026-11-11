---
id: HTTP
---

# HTTP services

Applies to the HTTP view of a service in BSL: its routes, route redirects, and the proxy settings a script declares on them.

## Borrowed terms

| term | meaning |
| --- | --- |
| field name | the name of an HTTP header field, a `token` as defined by [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) §5.1 and §5.6.2 |
| field value | the value of an HTTP header field, as defined by RFC 9110 §5.5 |
| content coding | a transformation applied to a representation, as defined by RFC 9110 §8.4.1 |
| redirect | a 3xx response carrying a `Location` header, as defined by RFC 9110 §15.4 and §10.2.2 |
| zstd | the Zstandard content coding, as defined by [RFC 8878](https://www.rfc-editor.org/rfc/rfc8878) |
| gzip | the gzip content coding, as defined by RFC 9110 §8.4.1.3 |

## Terminology

### HTTP service

A view of a [service](service.md#declaration) through which HTTP traffic on one of its ports is routed by URL prefix.

### Route

A URL prefix of an HTTP service, served either by the pods bound to it or by a redirect.

### Proxy settings

The compression, balancing, rate-limiting and header settings declared on a service or on one of its routes.

## Declaration

A script obtains an HTTP service with `service.http()` or `service.http(port: number)`, on a Service or on an [External Service](service.md#external-services).
Either method returns an `HttpService`.
The `port` is the [Service Port](service.md#service-ports) on which the HTTP service is reached, and defaults to 80.
A `port` MUST be validated as a [port number](overview.md#ports).

An `HttpService` is a view of its Service and not a resource of its own.
Settings declared through an `HttpService` MUST be recorded on the Service it views.

`httpService.port(port: number)` returns the `ServicePort` of the viewed Service at `port`, as `service.port(port)` does.

`httpService.ingress(hostname: string, port: number)` MUST declare an [ingress](ingress.md) on the viewed Service, exactly as `service.ingress(hostname, port)` does.
Called on an `HttpService` that views an External Service, it MUST throw.

## Routes

`httpService.route(prefix: string)` returns the `HttpServiceRoute` for `prefix`.
A `prefix` that is empty or does not begin with `/` MUST throw.
A `prefix` containing `{`, or an ASCII control character other than horizontal tab, MUST throw.

A Service's routes MUST be shared by every `HttpService` that views it, whatever port each names.
`svc.http(80).route("/a")` and `svc.http(8080).route("/a")` MUST name the same route, with the same proxy settings.

A prefix MUST be normalised by removing every trailing `/`, and a prefix that is empty after that is `/`.
Two spellings with the same normalised form MUST name the same route.

> [!NOTE]
> `route("/v1/login/")` and `route("/v1/login")` are one route, and `route("//")` is the route `/`.

Requests are matched to routes as [routing](../runtime/proxy.md#route-emission) specifies.
The runtime MUST pass a request to a pod with its path unchanged, including the matched prefix.

> [!NOTE]
> `GET /api/books` served through `route("/api")` reaches the pod as `GET /api/books`.

Where no pod binds a route of a Service, the runtime serves the Service through a [fallback route](../runtime/proxy.md#route-emission) at `/`.
That fallback route MUST take the proxy settings declared on `route("/")`, as any other route takes its own.

## Route redirects

A route declared as a redirect answers every request it matches with a redirect, and reaches no pod.
A script declares one with these methods of `HttpServiceRoute`:

| method | target | code |
| --- | --- | --- |
| `redirect(to: string)` | `to` followed by `<tail><query>` | 307 |
| `redirect(to: string, code: number)` | `to` followed by `<tail><query>` | `code` |
| `redirect(config: map)` | `config.to` as written | `config.code`, or 307 |

The `config` map carries `to`, which is REQUIRED, and `code`, which is OPTIONAL.
A `config` with any other key MUST throw.

The methods MUST be accepted on the routes of a Service and of an External Service alike.
A redirect route MUST NOT require a pod bound to it.

### Redirect targets

A target MUST begin with `/`, `http://` or `https://`, and any other target MUST throw.
A target beginning with `/` redirects within the hostname the request arrived on.
A target beginning with `http://` or `https://` redirects to the host it names.

A target may name these parts of the request, each a token the runtime replaces when it serves the redirect:

| token | replaced with |
| --- | --- |
| `<tail>` | the part of the request path following the matched prefix, with its leading `/`, or nothing when the path is exactly the prefix |
| `<query>` | the request's query string, with its leading `?`, or nothing when the request carries none |

A target naming no token is literal, and every request under the prefix MUST be redirected to exactly that target.

A target MUST throw if any of the following holds:

- it contains a `<` that does not begin `<tail>` or `<query>`;
- it contains `{`;
- it begins with `/` and its second character is `/` or `\`;
- it contains a tab or any other ASCII control character;
- a literal ending in `/` is immediately followed by `<tail>`, or a literal ending in `?` is immediately followed by `<query>`;
- it is longer than 4096 characters.

A positional `to` that contains `<` MUST throw.

> [!NOTE]
> The positional forms keep everything under the prefix, the common case of moving a prefix.
> `route("/v1/login").redirect("/api/login")` sends `/v1/login/reset?token=x` to `/api/login/reset?token=x`, whereas `redirect(#{ to: "/api/login" })` sends it to `/api/login`.
> `redirect("/")` throws, since `/` followed by `<tail>` is a literal ending in `/` before `<tail>`.

> [!NOTE]
> A target reaches the client as a `Location` header.
> A path whose second character is `/` or `\`, including one composed at request time from `/` and a tail, names another host while reading as a path within this one.
> The proxy substitutes a braced word in a `Location` with its own state, its environment included, so a brace is refused for the reason a [header value](#header-operations) refuses one.

### Redirect codes

A `code` MUST be one of 301, 302, 307 or 308, and any other value MUST throw.

> [!NOTE]
> A client receiving 301 or 302 may reissue the request as a `GET` and lose its body, so a permanent move of an endpoint that accepts a request body wants 308.

### Redirect route constraints

`route("/").redirect(…)` MUST throw.

> [!NOTE]
> A redirect on `/` would answer for the whole hostname.
> Retiring a hostname is a [site ingress redirect attachment](../runtime/site-resources.md#attachments), made by an operator.

A prefix MUST NOT be both a redirect and bound by a pod.
Declaring a redirect on a prefix a pod binds through [`pod.http`](pod.md#port-bindings) MUST throw, and so MUST binding a pod to a prefix declared as a redirect, in either order.
A second `redirect()` on the same route MUST replace the first.

Compression, balancing and rate limiting have nothing to act on in a redirect.
Declaring `compress`, `balance` or `rate_limit` on a redirect route MUST throw, in either order.
The same setting declared on the Service and inherited by a redirect route MUST be ignored for that route.

A redirect route MUST apply its resolved `response` [header operations](#header-operations) to the redirect it serves.
Declaring a `request` header operation on a redirect route MUST throw, in either order.
Declaring a `response` operation naming `Location` on a redirect route MUST throw, in either order.
`request` operations, and `response` operations naming `Location`, that a redirect route inherits from its Service MUST be ignored for that route.

## Compression

A script declares compression of proxied responses with `compress(enabled: bool)` or `compress(config: map)`, on an `HttpService` or an `HttpServiceRoute`.
`compress(false)` switches compression off.
`compress(true)` switches it on with the default settings.
`compress(config)` switches it on with the fields `config` names, and [compression resolution](#compression-resolution) gives the rest.

| field | type | default | meaning |
| --- | --- | --- | --- |
| `encodings` | array of `"zstd"` and `"gzip"` | `["zstd", "gzip"]` | the content codings offered, most preferred first |
| `minimum_length` | integer | 512 | the length in bytes below which a response is served uncompressed |
| `content_types` | array of strings | text-like types, below | the media types eligible for compression |

Every field is OPTIONAL.
An `encodings` entry other than `"zstd"` or `"gzip"` MUST throw, and so MUST an empty `encodings` array.
A negative `minimum_length` MUST throw.
An empty `content_types` array MUST throw.
A `config` with any other key MUST throw.

A `content_types` entry ending in `*` matches every media type beginning with the text before the `*`.
Any other entry matches that media type exactly.

The default `content_types` MUST be the entries below.

| media type family | entries |
| --- | --- |
| text | `text/*` |
| application | `application/atom+xml*`, `application/eot*`, `application/font*`, `application/geo+json*`, `application/graphql+json*`, `application/graphql-response+json*`, `application/javascript*`, `application/json*`, `application/ld+json*`, `application/manifest+json*`, `application/opentype*`, `application/otf*`, `application/rss+xml*`, `application/truetype*`, `application/ttf*`, `application/vnd.api+json*`, `application/vnd.ms-fontobject*`, `application/wasm*`, `application/x-httpd-cgi*`, `application/x-javascript*`, `application/x-opentype*`, `application/x-otf*`, `application/x-perl*`, `application/x-protobuf*`, `application/x-ttf*`, `application/xhtml+xml*`, `application/xml*` |
| font | `font/ttf*`, `font/otf*` |
| image | `image/svg+xml*`, `image/vnd.microsoft.icon*`, `image/x-icon*` |
| multipart | `multipart/bag*`, `multipart/mixed*` |

How the runtime compresses is specified by [route compression](../runtime/proxy.md#compression).

## Balancing

A script declares how the proxy chooses among a service's backends with `balance(config: map)`, on a Service, an External Service, an `HttpService` or an `HttpServiceRoute`.
On the first three, `balance` sets one service-wide value.
On a route, it overrides that value for the route.

| field | type | default | meaning |
| --- | --- | --- | --- |
| `policy` | string | `"round_robin"` | how a backend is chosen |
| `try_duration` | number of seconds | 5 | how long the proxy may spend finding a usable backend for one request or connection |
| `interval` | number of seconds | 0.25 | the pause between successive attempts to find one |

| `policy` | backend chosen |
| --- | --- |
| `"round_robin"` | each backend in turn |
| `"least_conn"` | the backend with the fewest requests in progress |
| `"random"` | a backend chosen at random |
| `"first"` | the first available backend in the order the runtime lists them |

Every field is OPTIONAL.
A `policy` not in the table above MUST throw.
`try_duration` and `interval` MUST be finite and non-negative, and MAY be written as integers or as floating-point numbers.
A `config` naming an `interval` of zero MUST throw unless the same `config` names a `try_duration` of zero.
A `config` with any other key MUST throw.

A `try_duration` of zero disables retrying: a request that cannot reach its first-chosen backend fails at once, and `interval` is not consulted.

If resolution yields an `interval` of zero and a non-zero `try_duration` for a route, evaluation of the definition MUST fail once the definition has evaluated, with an error naming the route.

> [!NOTE]
> A zero interval with a non-zero try duration retries without pause, spinning whenever every backend is unreachable.
> A route can reach that pairing only by taking one value from its service and the other from itself, which no single declaration can see, so the check waits until every declaration is in.

How the runtime balances is specified by [route balancing](../runtime/proxy.md#balancing).

## Rate limiting

A script declares per-client request rate limiting with `rate_limit(config: map)` or `rate_limit(false)`, on an `HttpService` or an `HttpServiceRoute`.
`rate_limit(config)` declares a limit.
`rate_limit(false)` declares that this level is not limited.
`rate_limit(true)` MUST throw.

| field | type | range | meaning |
| --- | --- | --- | --- |
| `max_events` | integer | 1 to 1000 | the number of requests one client may make within each window |
| `window` | number of seconds | 0.001 to 3600 | the length of the sliding window |

Both fields are REQUIRED.
A missing field, a field outside its range, a non-finite `window`, and any other key MUST each throw.
`window` MAY be written as an integer or as a floating-point number.

> [!NOTE]
> The ranges bound plausibility, not tuning.
> A limit is enforced by a proxy every app on the host shares, which holds `max_events` timestamps per client until the window has elapsed, for as many clients as send traffic.

How the runtime enforces a limit is specified by [route rate limiting](../runtime/proxy.md#rate-limiting).

## Header operations

A script declares operations on the headers of proxied traffic with `headers(config: map)`, on an `HttpService` or an `HttpServiceRoute`.

| key | applies to |
| --- | --- |
| `request` | the request as the service's pods receive it |
| `response` | the response as the client receives it |

Both keys are OPTIONAL, and a `config` naming neither MUST throw.
A `config` with any other key MUST throw.

The value of `request` or `response` is a map of operations:

| operation | form | effect |
| --- | --- | --- |
| `replace` | map of field name to value | sets the header to the value, discarding whatever the message carried |
| `add` | map of field name to value | adds the value, keeping whatever the message carried |
| `remove` | array of field names | discards the header |

Each operation is OPTIONAL, and a map naming none of the three MUST throw.
A map with any other key MUST throw.
An empty `replace` map, an empty `add` map, and an empty `remove` array MUST each throw.

A value in `replace` or `add` is a string, or an array of strings for a header that carries several values.
A value of any other type MUST throw, and so MUST an empty array.

> [!NOTE]
> `Set-Cookie` needs the array form, since several cookies cannot be folded into one header.

Removing a header the message does not carry, and replacing one it does not yet carry, are both well-formed.

### Header names

A name MUST be a [field name](#borrowed-terms), and any other name MUST throw.
A name containing `*` MUST throw.
Names MUST be compared case-insensitively and matched exactly.

A name that describes the connection or the message's framing MUST throw.
These names, in any case, are `Connection`, `Keep-Alive`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`, `Trailer`, `Transfer-Encoding`, `Upgrade`, `Content-Length` and `Content-Encoding`.

> [!NOTE]
> A proxy may read `*` in a field name as a wildcard, which would let a declaration naming one header reach every header.
> The proxy holds the connection to the client and the connection to the pod as two separate things, and owns the headers describing each.
> `Content-Encoding` names the compression the proxy applied, and a response whose encoding is renamed or removed cannot be decoded by the client.
> The proxy keeps connections persistent on its own account, as [persistent connections](../runtime/proxy.md#persistent-connections) specifies.

Within one direction of one `headers()` call, a name appearing under more than one operation MUST throw, whatever its case.

### Header values

A value MUST be a [field value](#borrowed-terms) containing no ASCII control character other than horizontal tab.
A value containing a carriage return, a line feed or any other control character MUST throw.
A value containing `{` MUST throw.

> [!NOTE]
> A carriage return or line feed would end the value's own header and begin another of the declaration's choosing.
> The proxy reads a braced word as a placeholder naming its own state, its environment included, and would substitute it.

### Header declaration bounds

A `headers()` call MUST throw if either of its directions names more than 64 headers.
A name longer than 256 characters MUST throw.
A value array longer than 16 entries MUST throw.
A value longer than 4096 characters MUST throw.

These bounds apply to one declaration and not to the result of resolving it.
A service and a route that each name different headers MAY exceed them between them.

### Repeated declarations

A second `headers()` call on the same service or route MUST layer over the first by header name, within each direction.
The later call's operation MUST win for each header it names, and the earlier call's operation MUST stand for the rest.

How the runtime applies header operations is specified by [route headers](../runtime/proxy.md#header-operations).

## Resolution

Each kind of proxy setting MUST resolve independently of the others, so that declaring one never disturbs another.

### Compression resolution

Whether a route is compressed MUST be decided by the nearest level that declared `compress`: the route, then its Service.
If neither level declared `compress`, the route MUST be compressed.
Each field of a compressed route MUST take the route's value if the route's `compress` named it, otherwise the Service's value if the Service's `compress` named it, otherwise the field's default.
A level that declared `compress(false)` contributes no field values.

### Balancing resolution

Each balancing field of a route MUST take the route's value if the route set it, otherwise the Service's value if the Service set it, otherwise the field's default.

### Rate-limit resolution

A route's rate limit MUST be the route's declaration if it made one, otherwise the Service's declaration if it made one, otherwise no limit.
A declaration of `rate_limit(false)` resolves to no limit, so a route declaring it is not limited even where its Service declared a limit.

### Header resolution

Header operations MUST resolve per header name, independently within each direction.
A header the route names MUST take the route's operation.
A header only the Service names MUST take the Service's operation.

> [!NOTE]
> Each header name resolves to exactly one operation, so the order in which a route's operations are applied is not observable.
