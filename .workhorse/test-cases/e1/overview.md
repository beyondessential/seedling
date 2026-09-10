# Per-route rate limiting

Scenarios verifying that an app can declare a per-client request limit on an HTTP service or one of its routes, and that the proxy enforces it.

## Declaring a limit

- [x] A service declaring `rate_limit(#{ max_events, window })` applies it to a route that declares none (verifies spec: service.http.proxy-settings.resolution)
- [x] Routes inheriting one service-level declaration share a single budget, so serving more routes does not raise what the pods can be sent (verifies spec: service.http.route.rate-limiting)
- [x] A route declaring its own limit replaces the service's outright, window included, rather than merging field by field (verifies spec: service.http.proxy-settings.resolution)
- [x] A route declaring `rate_limit(false)` is not limited even where the service declared one (verifies spec: service.http.proxy-settings.resolution)
- [x] A service and route that both leave it unmentioned produce no limit (verifies spec: service.http.rate-limit)
- [x] Declaring a limit disturbs neither compression nor balancing (verifies spec: service.http.proxy-settings.resolution)

## Rejecting bad declarations

- [x] A map missing `max_events` or `window` throws (verifies spec: service.http.rate-limit.fields)
- [x] A `max_events` of zero or negative throws (verifies spec: service.http.rate-limit.fields)
- [x] A `window` of zero, negative, or non-finite throws (verifies spec: service.http.rate-limit.fields)
- [x] An unrecognised field throws, naming the key rather than the field it displaced (verifies spec: service.http.rate-limit.fields)
- [x] A `max_events` above the ceiling throws (verifies spec: service.http.rate-limit.fields)
- [x] A `window` small enough to round to zero nanoseconds throws, rather than emitting a document the proxy rejects (verifies spec: service.http.rate-limit.fields)
- [x] A `window` large enough to saturate the nanosecond conversion throws (verifies spec: service.http.rate-limit.fields)
- [x] Values exactly at each bound are accepted (verifies spec: service.http.rate-limit.fields)
- [x] `rate_limit(true)` throws, since a limit has no default to enable (verifies spec: service.http.rate-limit)

## Emitted proxy configuration

- [x] A limited route carries the rate-limit handler ahead of the proxy, so excess costs a backend nothing (verifies spec: service.http.route.rate-limiting)
- [x] An unlimited route carries no rate-limit handler at all (verifies spec: service.http.route.rate-limiting)
- [x] A redirect route is never rate limited (verifies spec: service.http.route.rate-limiting)
- [x] The emitted zone uses only fields the pinned rate-limit module declares, since an unknown one fails the whole document (verifies spec: service.http.route.rate-limiting, infra.proxy.image.modules)
- [x] A longer prefix is emitted first and terminal, so its tighter limit governs its own traffic alone (verifies spec: service.http.route.rate-limiting, service.http.route.routing)
- [x] Two hostnames fronting one declared route share its budget, rather than granting a budget each (verifies spec: service.http.route.rate-limiting)
- [x] One hostname terminating both TLS and plaintext shares one budget, so alternating schemes does not double the limit (verifies spec: service.http.route.rate-limiting)
- [x] A cached proxy config written before rate limiting still loads on startup (verifies spec: infra.proxy.upgrade.cache)
- [x] The emitted rate-limit module is declared in the image's required-modules contract (verifies spec: infra.proxy.image.modules)

## Visibility

- [x] The web UI shows each route's resolved limit alongside its compression and balancing (verifies spec: service.http.route.proxy-settings.visibility)

- [x] Describing an app reports each route's resolved limit, inherited limits included (verifies spec: app.describe.proxy-settings)
- [x] A route that is not limited reports null rather than a zero-valued object (verifies spec: app.describe.proxy-settings)

## Against a running proxy

Scenarios needing a real Caddy rather than the emitted document. Not covered by the automated suite.

- [ ] A client exceeding the limit receives 429 with a Retry-After header
- [ ] Requests under the limit are proxied unaffected
- [ ] Login requests are counted against the login limit alone, and do not consume the wider API budget
- [ ] Two clients on different addresses are limited independently
- [ ] A service whose `/` route opts out is not limited on the fallback path taken before any pod has bound
- [ ] Limiter state survives a proxy config reload, so a reconcile does not reset a client's budget
- [ ] The emitted document is accepted by the real pinned proxy image, which no assertion on our own JSON can establish
