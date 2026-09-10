# Per-route rate limiting

## Decisions (from interview)

- **BSL surface, opt-in**, mirroring `compress`/`balance`. `rate_limit(config)` and `rate_limit(false)` on `HttpService` and `HttpServiceRoute`. No values are baked into the emitter.
- **Config shape**: `#{ max_events: <int>, window: <seconds> }`. Window as a number of seconds (mirrors `balance`'s seconds convention); the emitter formats it to the module's duration string. Both fields required — no sensible default for either.
- **Resolution as a whole unit** (not field-by-field): route's declaration, else service's, else no limit. `rate_limit(false)` at a route suppresses an inherited service limit.
- **Per-IP key** on the client IP the proxy attributes to the request (`{http.request.client_ip}`) — equals the connection peer today with no trust config, and automatically follows the recovered client once the front-proxy card lands.
- **IPv6 /64 grouping, IPv4 per address.** `ipv6_prefix: 64` is emitted; `ipv4_prefix` is left unset, which the module reads as "count IPv4 individually" via an explicit guard rather than as a /0 that would bucket every IPv4 client together. Both fields exist only on caddy-ratelimit master, so the Containerfile pins commit `5625512f` rather than `v0.1.0` — the only release ever cut, which has neither. The commit needs Caddy >= 2.10 and we pin 2.11.4. Image tag moved to `2.11.4-2` in the Containerfile, the workflow `TAG`, and `CADDY_IMAGE`.
- **Over-limit**: 429 + Retry-After (emitted automatically by caddy-ratelimit).
- **Scope**: HTTP reverse-proxy routes only. Redirects, non-HTTP forwarding, and the L4 path carry no limit.
- Module in the image since D1. Single instance → local sliding window, no distributed storage. Reclamation is automatic: the module sweeps every minute by default, with the sweeper started unconditionally, so nothing is emitted to switch it on.

## Load-bearing existing behaviour

`proxy_routes_for_vhost` (crates/core/src/system/caddy/config.rs:224) sorts routes longest-prefix-first and emits them `terminal: true`. Each request hits exactly one route, so `/api/login` (10/s) counts only against its own zone and `/api` (1000/s) against the rest. Do not disturb this ordering.

## Implementation checklist

- [x] Spec: `service.http.rate-limit` + `.fields` (language.md), `service.http.route.rate-limiting` + visibility (runtime.md), `app.describe.proxy-settings` rate_limit field (interface.md)
- [x] BSL parse: `parse_rate_limit` in defs/service/proxy.rs — tri-state decl (`Disabled` / `Enabled{max_events, window_secs}`), reject_unknown, validation (max_events positive int, window positive finite). Wire `rate_limit(false)` and `rate_limit(map)` onto the HttpService and HttpServiceRoute builders alongside compress/balance
- [x] Resolve: whole-unit resolution (route decl else service decl else none) → `ResolvedRouteProxy.rate_limit: Option<ResolvedRateLimit>`
- [x] Wire type: add `rate_limit` to `RouteProxy` (system/types.rs) and the `From<ResolvedRouteProxy>` impl
- [x] Emitter: in `proxy_routes_for_vhost`, prepend a `rate_limit` handler to the reverse-proxy chain (ahead of `encode`) with one zone — key `{http.request.client_ip}`, `window` in nanoseconds, `max_events`. No prefix-masking fields: the pinned module declares none. Zone name is the declaring `app/service{prefix}`, carried from the translate layer, so vhosts fronting one declaration share its budget
- [x] Describe: include resolved `rate_limit` in `app.describe` proxy-settings output, and a per-route chip in the web UI
- [x] Demo defs: reverted. Declaring limits there presented a protective control the implementation cannot yet deliver (per-address, not per-party) on the real Tamanu API surface, and every review round read them as production config. The mechanism is demonstrated by the tests instead. Values belong to the production definition, which is not this repo
- [x] Tests: parse/resolution unit tests (proxy/tests.rs), emitter tests covering the handler, terminal ordering, zone sharing across vhosts, the pinned-module field set, and validation-throws cases
- [x] tracey: annotate impls/tests against the new spec items

## Settled

- **Path normalisation.** The proxy matches a prefix against a normalised path: duplicate separators collapsed, relative segments resolved, letter case folded, percent-encoding decoded (Caddy 2.11.4 `MatchPath`). `/api//login`, `/api/./login` and `/api/Login` therefore reach a `/api/login` route rather than falling through to `/api`, so the nested-prefix bypass does not work. Caddy normalises more than a typical backend router, so residual disagreement over-applies the stricter limit rather than escaping it.
- **Describe reports whether a route is served.** A prefix no pod binds carries `served: false`, so a limit declared on an unbound route no longer reads as a control in force. Threaded the AppDef into `Resource::summary` and its four call sites.
- **B3 needs no values in this repo.** The requirement is met by the capability; the production Tamanu definition is not this repo, per the project's prime directive.

## Deferred (own cards)

- **Real client IP behind a front proxy** — PROXY protocol (L4 listener wrapper) *and* X-Forwarded-For trusted_proxies (HTTP). Both wanted. E1 keys on `client_ip` so it consumes the recovered client with no emitter change. Direct-to-host is the norm today, so E1 is correct without it. → card A2.
- **Validate the emitted config against the real image** — every emitter test asserts our JSON against our own expectations, so a document the running binary rejects passes the suite. That is how the `ipv6_prefix` outage got in. → card P2.
- **L4 per-source rate limiting** — caddy-ratelimit is HTTP-only; caddy-l4 `throttle` is bandwidth, not connection-rate. The kernel path (nftables `ct count` / `limit rate` per source, ahead of the ingress DNAT) is the candidate. → card B2.
