# G1: header manipulation rules

Spec-first: `docs/spec/language.md`, `runtime.md`, and `interface.md` are written; implementation follows.
Four tracey rules are currently uncovered and must end up annotated: `service.http.headers`,
`service.http.headers.fields`, `service.http.route.headers`, `ingress.persistent-connections`.

## Mapping onto Caddy

The proxy is Caddy, driven via its JSON admin API. The header handler is
`caddyhttp/headers.Handler`, whose `HeaderOps` is:

- `add` / `set` — both `http.Header`, i.e. `map[string][]string`.
- `delete` — `[]string`.
- `replace` — `map[string][]Replacement`, a substring/regex substitution.

Our vocabulary does **not** map one-to-one onto those names. The BSL `replace` means
"set the field to this value, discarding what was there", which is Caddy's **`set`**, not
Caddy's `replace`. Wiring BSL `replace` to Caddy `replace` would silently produce substring
substitution instead of assignment. Caddy's `replace` is not exposed at all.

Multi-value support is free: `add` and `set` are already `map[string][]string`, so the
string-or-array BSL value normalises to a `Vec<String>` at parse time and passes straight
through. Normalising early also gives `app.describe` its uniform array shape.

`HeaderOps.ApplyToRequest` special-cases `Host`, mutating `r.Host` rather than a map entry,
because Go keeps the request host outside the header map. This is what makes the upstream
`Host` override work through the generic request-header surface with no dedicated affordance.
If a future change stops routing request ops through that path, the `Host` requirement in
`service.http.route.headers` breaks silently — the setting appears to apply and the pods
still observe the client's hostname.

## Handler ordering

`caddy/config.rs:225` `proxy_routes_for_vhost` builds the chain: rate_limit, then encode, then
reverse_proxy. `service.http.route.headers` requires response operations to apply to
proxy-generated responses as well as upstream ones, including the rate limiter's 429. The
headers handler must therefore sit **ahead of the rate_limit handler** in the chain, with
response ops deferred, so a rejection on the way out still passes through it.

## Layers to thread

The route-settings chain is the one `rate_limit` already walks:

- [x] `defs/service/proxy.rs` — `HeaderName`/`HeaderOp`/`HeaderRules`/`HeaderSettings`, `parse_headers`, and the per-header-name resolver
- [x] `defs/service.rs` — `headers()` builder on `HttpService` and `HttpServiceRoute`
- [x] `system/types.rs` — `RouteHeaders`/`RouteHeaderOps` wire structs and `from_resolved` mapping
- [x] `system/reconcile/proxy.rs` — nothing to change: the settings travel inside `RouteProxy`, which reconcile already carries whole
- [x] `system/caddy/config.rs` — emit the headers handler, ordered as above
- [x] `app.describe` — report `headers` per route, plus the web UI's route row and its TS type
- [x] `docker/caddy/required-modules.txt` — declare `http.handlers.headers`, and exercise it in the image fixture

Resolution is per `(direction, header-name)`, case-insensitively. Because each name resolves to
exactly one operation, the emitted per-route ops can be flattened into a single `HeaderOps` per
direction with no ordering concerns.

## Persistent connections

`ingress.persistent-connections` is a guarantee rather than a surface: HTTP/1.1 keep-alive is
Caddy's default, so the work is a check that our rendered config does not disable it (no
`idle_timeout: 0`, no `Connection: close`), plus a test pinning it. `etc/ci/` already holds
greps of this kind if a static check is the right shape.

## Notes from implementation

`HeaderName` carries the spelling the app used but compares and orders
case-insensitively, so per-name resolution treats a route's `cache-control` and
a service's `Cache-Control` as one header. Doing this in the type rather than at
each call site is what stops the next consumer forgetting it. `HeaderRules::grouped`
is the single body that groups operations for both the proxy config and
`app.describe`, so the two cannot describe the headers differently.

Boxing `ProxyRouteHandler::ReverseProxy`'s `proxy` field was forced by the added
settings: the variant reached 304 bytes against the redirect variant's 27, and
clippy's `large_enum_variant` fires past a 200-byte spread. Serde treats a box
transparently, so the cached proxy document is unchanged by it.

`headers()` is a single entry point taking both directions, rather than the two
methods first sketched. It reads better at the call site and keeps everything
header-related in one map.

Still unverified: that a rate-limit rejection actually carries the route's
response headers. The ordering and `deferred` flag are what should deliver it,
but confirming it needs a running Caddy, which this repo has no harness for. It
is recorded unticked in the card's test cases.
