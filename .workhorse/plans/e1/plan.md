# Per-route rate limiting

## Decisions (from interview)

- **BSL surface, opt-in**, mirroring `compress`/`balance`. `rate_limit(config)` and `rate_limit(false)` on `HttpService` and `HttpServiceRoute`. No values are baked into the emitter.
- **Config shape**: `#{ max_events: <int>, window: <seconds> }`. Window as a number of seconds (mirrors `balance`'s seconds convention); the emitter formats it to the module's duration string. Both fields required — no sensible default for either.
- **Resolution as a whole unit** (not field-by-field): route's declaration, else service's, else no limit. `rate_limit(false)` at a route suppresses an inherited service limit.
- **Per-IP key** on the client IP the proxy attributes to the request (`{http.request.client_ip}`) — equals the connection peer today with no trust config, and automatically follows the recovered client once the front-proxy card lands.
- **IPv6 /64 grouping is NOT implemented**, though it was the decision at interview. `ipv4_prefix`/`ipv6_prefix` exist only on caddy-ratelimit master; the pinned `v0.1.0` is the sole released tag and declares neither. Caddy decodes module config strictly, so emitting them fails the whole document and drops ingress for every vhost on the host. Addresses are counted individually; restoring /64 needs an unreleased module in the fleet image. See the open question below.
- **Over-limit**: 429 + Retry-After (emitted automatically by caddy-ratelimit).
- **Scope**: HTTP reverse-proxy routes only. Redirects, non-HTTP forwarding, and the L4 path carry no limit.
- Module already present: `caddy-ratelimit@v0.1.0` in `seedling-caddy:2.11.4-1` (D1, complete). Single-instance → local sliding window, no distributed storage.

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

## Open question

- **Does the card need shipped values at all?** The capability is complete and tested; the demo defs no longer declare limits. If B3 is only satisfied by 1000/s and 10/s existing somewhere in this repo, they need to go back — and then the per-address gap below has to be closed first, because those values read as a security control.

- **Restore IPv6 /64 grouping?** This is now the load-bearing one. The ceilings bound what each tracked address costs and how long it is held, but not how many addresses are tracked — that is the sender's choice. Counting addresses individually therefore leaves the proxy's total state unbounded, and the proxy fronts every app on the host, so turning a limit on is itself a cost. Grouping a /64 to one key is what closes it. It needs `caddy-ratelimit` pinned to a master commit rather than a released tag, and the fleet image rebuilt and republished — which the Containerfile's own versioning discipline argues against ("Every `--with` is pinned to an exact tag"). Without it an attacker holding a /64 has 2^64 budgets against the login limit. Options: pin a commit, wait for a release, or accept per-address counting and revisit. Not decided.

## Deferred (own cards)

- **Real client IP behind a front proxy** — PROXY protocol (L4 listener wrapper) *and* X-Forwarded-For trusted_proxies (HTTP). Both wanted. E1 keys on `client_ip` so it consumes the recovered client with no emitter change. Direct-to-host is the norm today, so E1 is correct without it. → card A2.
- **L4 per-source rate limiting** — caddy-ratelimit is HTTP-only; caddy-l4 `throttle` is bandwidth, not connection-rate. The kernel path (nftables `ct count` / `limit rate` per source, ahead of the ingress DNAT) is the candidate. → card B2.
