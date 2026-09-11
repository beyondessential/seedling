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

- [ ] `defs/service/proxy.rs` — `HeaderRules` on `ProxySettings`, parser, and the per-header-name resolver into `ResolvedRouteProxy`
- [ ] `defs/service.rs` — `headers()` builder on `HttpService` and `HttpServiceRoute`
- [ ] `system/types.rs` — `RouteHeaders` wire struct and `from_resolved` mapping
- [ ] `system/reconcile/proxy.rs` — carry through `collect_http_routes` / `service_level_proxy`
- [ ] `system/caddy/config.rs` — emit the headers handler, ordered as above
- [ ] `app.describe` — report `headers` per route

Resolution is per `(direction, header-name)`, case-insensitively. Because each name resolves to
exactly one operation, the emitted per-route ops can be flattened into a single `HeaderOps` per
direction with no ordering concerns.

## Persistent connections

`ingress.persistent-connections` is a guarantee rather than a surface: HTTP/1.1 keep-alive is
Caddy's default, so the work is a check that our rendered config does not disable it (no
`idle_timeout: 0`, no `Connection: close`), plus a test pinning it. `etc/ci/` already holds
greps of this kind if a static check is the right shape.
