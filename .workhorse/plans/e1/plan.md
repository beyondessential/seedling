# Per-route rate limiting

## Decisions (from interview)

- **BSL surface, opt-in**, mirroring `compress`/`balance`. `rate_limit(config)` and `rate_limit(false)` on `HttpService` and `HttpServiceRoute`. The 1000/s and 10/s values live in the demo Tamanu def, not the emitter.
- **Config shape**: `#{ max_events: <int>, window: <seconds> }`. Window as a number of seconds (mirrors `balance`'s seconds convention); the emitter formats it to the module's duration string. Both fields required — no sensible default for either.
- **Resolution as a whole unit** (not field-by-field): route's declaration, else service's, else no limit. `rate_limit(false)` at a route suppresses an inherited service limit.
- **Per-IP key** on the client IP the proxy attributes to the request (`{http.request.client_ip}`) — equals the connection peer today with no trust config, and automatically follows the recovered client once the front-proxy card lands. **IPv6 grouped by /64**, IPv4 per exact address.
- **Over-limit**: 429 + Retry-After (emitted automatically by caddy-ratelimit).
- **Scope**: HTTP reverse-proxy routes only. Redirects, non-HTTP forwarding, and the L4 path carry no limit.
- Module already present: `caddy-ratelimit@v0.1.0` in `seedling-caddy:2.11.4-1` (D1, complete). Single-instance → local sliding window, no distributed storage.

## Load-bearing existing behaviour

`proxy_routes_for_vhost` (crates/core/src/system/caddy/config.rs:224) sorts routes longest-prefix-first and emits them `terminal: true`. Each request hits exactly one route, so `/api/login` (10/s) counts only against its own zone and `/api` (1000/s) against the rest. Do not disturb this ordering.

## Implementation checklist

- [ ] Spec: `service.http.rate-limit` + `.fields` (language.md), `service.http.route.rate-limiting` + visibility (runtime.md), `app.describe.proxy-settings` rate_limit field (interface.md) — **drafted**
- [ ] BSL parse: `parse_rate_limit` in defs/service/proxy.rs — tri-state decl (`Disabled` / `Enabled{max_events, window_secs}`), reject_unknown, validation (max_events positive int, window positive finite). Wire `rate_limit(false)` and `rate_limit(map)` onto the HttpService and HttpServiceRoute builders alongside compress/balance
- [ ] Resolve: whole-unit resolution (route decl else service decl else none) → `ResolvedRouteProxy.rate_limit: Option<ResolvedRateLimit>`
- [ ] Wire type: add `rate_limit` to `RouteProxy` (system/types.rs) and the `From<ResolvedRouteProxy>` impl
- [ ] Emitter: in `proxy_routes_for_vhost`, prepend a `rate_limit` handler to the reverse-proxy chain (ahead of `encode`) with one zone — key `{http.request.client_ip}`, `ipv6_prefix: 64`, `window` formatted from seconds, `max_events`. Unique zone name per (hostname, prefix)
- [ ] Describe: include resolved `rate_limit` in `app.describe` proxy-settings output
- [ ] Demo def: add `rate_limit` to the tamanu def — `/api` at 1000/s, and a `/api/login` route at 10/s to demonstrate the tighter-prefix case (illustrative; demo defs are not canonical)
- [ ] Tests: parse/resolution unit tests (proxy/tests.rs), emitter snapshot showing the handler + /64 masking + terminal ordering, validation-throws cases
- [ ] tracey: annotate impls/tests against the new spec items

## Deferred (own cards)

- **Real client IP behind a front proxy** — PROXY protocol (L4 listener wrapper) *and* X-Forwarded-For trusted_proxies (HTTP). Both wanted. E1 keys on `client_ip` so it consumes the recovered client with no emitter change. Direct-to-host is the norm today, so E1 is correct without it. → card A2.
- **L4 per-source rate limiting** — caddy-ratelimit is HTTP-only; caddy-l4 `throttle` is bandwidth, not connection-rate. The kernel path (nftables `ct count` / `limit rate` per source, ahead of the ingress DNAT) is the candidate. → card B2.
