# Edge parity: response compression and upstream retries

Notes from the spec interview. The acceptance criteria live in
`docs/spec/language.md` (`l[service.http.compress]`, `l[service.http.balance]`,
`l[service.http.proxy-settings.resolution]`) and `docs/spec/runtime.md`
(`r[service.http.route.compression]`, `r[service.http.route.balancing]`,
`r[service.http.route.proxy-settings.visibility]`).

## Why the retry knobs are duration + interval, not a count

The card asked for `lb_retries 2` alongside `lb_try_duration 5s`. Caddy documents
the duration as taking precedence over the count when both are set, so
`lb_retries 2` would have been silently ignored in the configuration the card
described. At the default 250ms interval a 5s budget is roughly twenty attempts,
not two. The BSL surface therefore exposes `try_duration` and `interval`, which
between them actually determine the attempt count, and no count field.

## What a retry does and does not cover

Caddy always allows a retry when the connection to an upstream could not be
established, regardless of method. After a successful connection, a failed
round-trip is retried only if `lb_retry_match` permits, and its default is
GET-only.

This is why the spec is careful to say retries cover unreachable upstreams
rather than failed requests generally:

- The draining-container case from the card description is a dial failure, so it
  is covered, including for POST. Nothing reached the pod, so nothing can have
  been acted on twice.
- A 502 from a *reachable* upstream is not retried by this mechanism at all.
  Steering traffic away from a backend that answers with errors is passive
  health checking (`fail_duration`, `unhealthy_status`), which was deliberately
  left out of this card's scope.

Do not widen `lb_retry_match` to pick up the 502 case without revisiting the
idempotency question first.

## Emitter mapping

Both settings attach to the HTTP `reverse_proxy` handler in
`proxy_routes_for_vhost` (`crates/core/src/system/caddy/config.rs`). The layer4
`proxy` handler and the `static_response` redirect handler stay bare.

Note the two upstream shapes that flow through the same handler, per
`build_proxy_config` in `crates/core/src/system/translate/proxy.rs`:

- No `http_bindings`: one upstream, the service IP, with kernel ECMP spreading
  connections. The balancing policy is a no-op here; the try duration still
  covers the reconnect window.
- Per-prefix routes: one upstream per backend pod, where the policy does real
  work.

Both take the service-level settings when the route sets nothing, so neither
shape needs special-casing beyond that.

## Defaults are the proxy's own defaults

`encode` with no arguments is already zstd-preferred + gzip, a 512-byte floor,
and a text-like content-type matcher. The card's `encode zstd gzip` is the
default case, so the emitter should be able to omit fields left at their
defaults rather than spelling them out.

## Consequential edit

`l[service.routing]` claimed traffic to multiple targets is "distributed
round-robin" without qualification. Now that an app can select `least_conn`,
that item has been scoped: round-robin remains the default and the L4 behaviour,
with HTTP traffic through an ingress following the route's policy.
