---
id: HTTP
---

# HTTP services

## HTTP Service

<!-- from language: Service / HTTP Service -->

> l[service.http]
> A Service can be _specialised_ into an HTTP Service, using the `service.http(port?: number)` instance method, which returns an `HttpService`.
>
> The `port` argument is optional and defaults to `80`.
>
> An `HttpService` is a per-call view of the underlying Service rather than a distinct resource. As an ergonomic affordance, `httpService.ingress(hostname, port)` is accepted and is equivalent to calling `ingress()` on the wrapped Service: the resulting Ingress is bound to the Service. Calling `ingress()` on an `HttpService` whose Service is external must throw.

> l[service.http.route]
> An HTTP Service Route serves a URL prefix.
>
> HTTP Service Routes are defined using the `http.route(prefix: string)` instance method, which returns an `HttpServiceRoute`. The `prefix` argument must be a non-empty string starting with `/`.
>
> The URL prefix is _not_ stripped for the pod: `GET /api/books` routed through a `route("/api")` will appear as `GET /api/books` to the container.
>
> Prefix-matching is done by length: for any given URL, the longest matching prefix is selected. If more complicated logic is required, an application should embed an HTTP "reverse proxy" container of its choice.
>
> A trailing `/` carries no meaning in a prefix, so `route("/v1/login/")` and `route("/v1/login")` are one route and not two.
> Every spelling of the root is the root, `//` among them.

> l[service.http.route.redirect]
> The `route.redirect(to: string)`, `route.redirect(to: string, code: number)`, and `route.redirect(config: map)` builder methods declare that an [HTTP Service Route](rule:l/service.http.route) answers with an HTTP redirect instead of being served by a pod.
> They are available on the routes of an app-declared Service and of an [External Service](rule:l/service.external) alike, and a redirect route needs no pod bound to it.
>
> The `to` target is either a path beginning with `/`, which redirects within the hostname the request arrived on, or an absolute URL beginning with `http://` or `https://`, which redirects to another host.
> Any other target throws.
> The `code` defaults to 307 and must be one of 301, 302, 307, or 308; any other value throws.
> A permanent move of an endpoint that accepts a request body wants 308, since 301 and 302 permit a client to reissue the request as a `GET` and so lose the body.
>
> The map form carries `to` and an optional `code` and no other field; an unrecognised field throws.
>
> A target may name the parts of the incoming request that carry over to it:
>
> - `<tail>`: the part of the request path following the matched prefix, carrying its leading `/`. Empty when the request path is exactly the prefix.
> - `<query>`: the request's query string, carrying its leading `?`. Empty when the request carries none.
>
> A target naming no token is literal: every request under the prefix redirects to exactly that target, whatever followed the prefix.
> An unrecognised token throws, rather than reaching the client as part of the target.
> So does a stray `<`, a character a URL carries percent-encoded rather than as itself, so there is nothing for it to have meant.
>
> The two positional forms are the map form with `<tail><query>` appended to the target, which is the common case of moving a prefix while keeping everything under it.
> A positional target naming a token itself throws, rather than carrying that part of the request twice: naming one is writing the target out in full, which is the map form.
> `route("/v1/login").redirect("/api/login")` therefore sends `/v1/login/reset?token=x` to `/api/login/reset?token=x`, where `redirect(#{ to: "/api/login" })` sends it to `/api/login`.
>
> The tokens are those named here, and are translated by the runtime.
> A target must not be written in the proxy's own placeholder syntax: the proxy substitutes a braced word naming its own state, its environment among it, and the target reaches the client as a `Location` header, where a [header value](rule:l/service.http.headers.fields) refuses a brace for that same reason.
>
> A path target whose second character is `/` or `\\` throws: it names another host while opening with the `/` that says "within the hostname the request arrived on", and the absolute form says so plainly.
> A target carrying a tab throws for the same reason, a client stripping one before reading the target.
>
> Each token carries its own leading separator, so a target must not end a literal with that separator immediately before the token: `/` before `<tail>` and `?` before `<query>` throw.
> Left to compose, a target of `/` followed by `<tail>` reaches the client as `//` and whatever the request carried, which is the other host above assembled at request time.
>
> A redirect must not be declared on the root prefix, which would answer for the whole hostname; retiring a hostname is a [site ingress redirect attachment](rule:r/ingress.site.attachment), an operator's to make rather than an app's.
> A prefix declared as a redirect and also bound by a pod through `deployment.http(pod_port, svc.route(prefix))` throws: a prefix is either redirected or proxied.
> A second `redirect()` on the same route replaces the first.
>
> [Compression](rule:l/service.http.compress), [balancing](rule:l/service.balance), and [rate limiting](rule:l/service.http.rate-limit) have nothing to act on in a redirect, which reaches no pod: declaring one on a redirect route throws, where the error still names the line that wrote it.
> The same setting declared on the Service and inherited by the route is ignored instead, so a service-wide declaration need not be written around the service's redirect routes.
> [Header](rule:l/service.http.headers) `response` operations do apply, a redirect being a response the route serves; a `request` operation on a redirect route throws, having no onward request to shape.
> A `response` operation naming `Location` on a redirect route throws, since it would displace the target the redirect computed and the route would then not serve what `to` declares.

> l[service.http.compress]
> `compress(enabled: bool)` and `compress(config: map)` are builder methods declaring compression of responses served through the proxy.
> Both are available on an [HTTP Service](rule:l/service.http) and on an [HTTP Service Route](rule:l/service.http.route).
>
> `compress(false)` disables compression. `compress(true)` enables it with the default settings below, as does the map form, which additionally sets whichever fields it names.
>
> Compression is a property of responses proxied to the service's pods. Redirect responses, whether from an [ingress redirect](rule:l/ingress.redirect) or a [route redirect](rule:l/service.http.route.redirect), are never compressed.

> l[service.http.compress.fields]
> All fields of the `config` map are optional:
>
> - `encodings`: the content encodings offered, in descending order of preference. Accepted values are `"zstd"` and `"gzip"`. Default `["zstd", "gzip"]`.
> - `minimum_length`: responses shorter than this many bytes are served uncompressed, because compressing them costs more than it saves. Default 512.
> - `content_types`: the content types eligible for compression. An entry ending in `*` matches any content type with that prefix. Default is a set covering text and text-like formats: markup, stylesheets, scripts, JSON and other structured text, SVG, and fonts.
>
> `minimum_length` must be a non-negative integer. An unrecognised encoding, an empty `encodings` list, and an empty `content_types` list must each throw.

> l[service.http.proxy-settings.resolution]
> [compression](rule:l/service.http.compress) is declared on an HTTP Service, [balancing](rule:l/service.balance) on the Service itself, and either may be overridden on an individual HTTP Service Route.
> [Rate limiting](rule:l/service.http.rate-limit) and [header manipulation](rule:l/service.http.headers) are likewise declared on an HTTP Service or an individual HTTP Service Route.
>
> Compression and balancing resolve field by field: the route's value if the route set that field, otherwise the service's value if the service set it, otherwise the field's default.
> A route setting some fields therefore keeps the service's values for the fields it left unset, and setting any one of `compress`, `balance`, `rate_limit`, or `headers` never disturbs the others.
>
> Rate limiting resolves as a whole rather than field by field: the route's declaration if the route made one, otherwise the service's, otherwise no limit.
> It has no default, so a service and route that both leave it unmentioned are not rate limited, and a route declaring `rate_limit(false)` is not rate limited even where the service declared a limit.
>
> Header manipulation resolves per header name, independently within each direction: a header the route names takes the route's operation, and a header only the service names takes the service's.
> A route therefore adds to the headers the service declared and overrides only those it names, without restating the rest.
> Because each header name resolves to exactly one operation, the order in which operations are applied is not observable.
>
> A Service with no HTTP Service Routes is served through a single `/` route, which takes the service's values.

> l[service.balance]
> `balance(config: map)` is a builder method controlling how the proxy chooses among the backends serving a Service, and how long it may keep trying to reach one.
> It is available on a [Service](rule:l/service.type), on an [External Service](rule:l/service.external), on an [HTTP Service](rule:l/service.http), and on an [HTTP Service Route](rule:l/service.http.route).
> The first three set the same service-wide value; the route form overrides it for one route.
>
> All fields of the `config` map are optional:
>
> - `policy`: how a backend is chosen. One of `"round_robin"`, `"least_conn"`, `"random"`, `"first"`. Default `"round_robin"`.
> - `try_duration`: seconds the proxy may spend finding a usable backend for a single request or connection before giving up. Default 5.
> - `interval`: seconds to wait between successive attempts to find one. Default 0.25.
>
> Both timing fields must be non-negative, and an unrecognised `policy` must throw.
> A `try_duration` of zero disables retrying, so a request that cannot reach its first-chosen backend fails immediately and `interval` is not consulted.
> An `interval` of zero combined with a non-zero `try_duration` must throw, because it would spin without pause whenever every backend is unreachable.

> l[service.http.rate-limit]
> `rate_limit(config: map)` and `rate_limit(false)` are builder methods declaring per-client request rate limiting for a service's routes.
> Both are available on an [HTTP Service](rule:l/service.http) and on an [HTTP Service Route](rule:l/service.http.route).
>
> The map form declares a limit carrying the fields below; `rate_limit(false)` declares no limit at this level, which at a route also suppresses a limit the service declared.
> A limit applies to requests proxied to the service's pods, and is opt-in: a service that never declares one is not rate limited.
> Redirect responses, whether from an [ingress redirect](rule:l/ingress.redirect) or a [route redirect](rule:l/service.http.route.redirect), and non-HTTP forwarding are never rate limited.

> l[service.http.rate-limit.fields]
> Both fields of the `config` map are required, since a limit has no meaning without either:
>
> - `max_events`: the number of requests one client may make within each window. Must be a positive integer, at most one thousand.
> - `window`: the length of the sliding window, in seconds. Must be a finite number between one millisecond and one hour.
>
> An unrecognised field, and either field outside its range, must each throw.
> The ranges are sanity bounds rather than tuning: a limit is charged to a proxy every app on the host shares, holding per-client state sized by `max_events` until the window has elapsed, so a declaration sets both how large that state is and how long it is held. How many clients are tracked is decided by whoever sends the traffic, so the size of each is held to something an app could plausibly mean, and a declaration outside that is refused where the error still names the script that made it.

> l[service.http.headers]
> `headers(config: map)` is a builder method declaring manipulations of the headers carried by requests and responses passing through the proxy.
> It is available on an [HTTP Service](rule:l/service.http) and on an [HTTP Service Route](rule:l/service.http.route).
>
> The `config` map carries two optional fields, `request` and `response`, each a map of the operations applied in that direction.
> `request` operations shape the request as the service's pods receive it, and `response` operations shape the response as the client receives it.
> A `config` naming neither must throw, since it declares nothing.
>
> A second `headers()` call on the same service or route layers over the first by header name, rather than discarding what it declared: the later call's operation wins for the headers it names, and the earlier one's stands for the rest.
>
> Header manipulation is a property of traffic proxied to the service's pods.
> [Ingress redirect](rule:l/ingress.redirect) responses and non-HTTP forwarding carry no header operations.
> A [route redirect](rule:l/service.http.route.redirect) carries the `response` operations alone, those being the ones a response it serves can bear.

> l[service.http.headers.fields]
> The `request` and `response` maps each carry three optional fields, naming the operation applied to a header:
>
> - `replace`: a map of header name to value, setting the header to that value and discarding whatever the message already carried.
> - `add`: a map of header name to value, adding that value and keeping whatever the message already carried.
> - `remove`: an array of header names, discarding those headers entirely.
>
> A map naming none of the three, an empty `remove` array, and an empty `replace` or `add` map, must each throw.
>
> A value in `replace` or `add` is either a string, or an array of strings where the header is to carry several values.
> `Set-Cookie` is the case that requires the array form, since several cookies cannot be folded into one header.
> A value that is neither form, and an empty array, must each throw.
>
> Header names are compared case-insensitively, as HTTP defines them, and are matched exactly: there is no wildcard form, because a pattern could not be resolved against the named headers of an enclosing service.
> A name must be a valid HTTP field name, and must additionally not contain `*`, which the grammar permits but which a proxy may read as a wildcard; a declaration meaning one header must not be able to reach every header.
> A name appearing under more than one operation in the same direction at the same level must throw, whatever its case, since those operations contradict each other.
>
> A value must be a valid HTTP field value: printable characters, spaces, and horizontal tabs.
> A control character must throw, a carriage return or line feed above all, since those would end the value's own header and begin another of the declaration's choosing.
>
> A value must also not contain `{`, which must throw.
> A proxy may read a braced word as a placeholder naming its own state and substitute it, so a value carrying one would reach the other side as something other than what was declared — and the state it names may be the proxy's environment rather than anything belonging to the app.
> Refusing the character is what keeps a value literal, and is refused where the error can still name the script that wrote it.
>
> A direction may carry at most 64 operations, a name at most 256 characters, an operation at most 16 values, and a value at most 4096 characters.
> These bound one declaration rather than the result of resolving it: a service and a route each naming different headers may exceed them between them, and that is not refused.
> They are plausibility bounds like a [rate limit](rule:l/service.http.rate-limit.fields)'s rather than a guarantee about the size of the shared configuration — a declaration far outside anything an app could mean is refused where the error can still name the script that wrote it.
>
> Naming a header that describes the connection rather than the message must throw: `Connection`, `Keep-Alive`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`, `Trailer`, `Transfer-Encoding`, and `Upgrade`.
> `Content-Length`, which describes the message's framing, and `Content-Encoding`, which names the [compression](rule:l/service.http.compress) the proxy applied, must throw for the same reason: a response whose encoding is renamed or removed cannot be decoded by the client it reaches.
> The proxy holds the connection to the client and the connection to the pod as two separate things, and owns the headers describing each; an app setting them corrupts the exchange rather than shaping it.
> [Persistent connections](rule:r/ingress.persistent-connections), the reason an app would otherwise reach for `Connection`, are served without being asked for.
>
> Removing a header the message does not carry, and replacing one it does not yet carry, are both well-formed.
> An operation describes the message as the other side is to receive it, rather than a change to one in hand.
