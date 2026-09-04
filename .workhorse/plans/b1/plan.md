# Edge parity: response compression and upstream retries

Notes from the spec interview. The acceptance criteria live in
`docs/spec/language.md` (`l[service.http.compress]`, `l[service.http.balance]`,
`l[service.http.proxy-settings.resolution]`), `docs/spec/runtime.md`
(`r[service.http.route.compression]`, `r[service.http.route.balancing]`,
`r[service.http.route.proxy-settings.visibility]`), and `docs/spec/interface.md`
(`i[app.describe.proxy-settings]`).

## Why the retry knobs are duration + interval, not a count

The card asked for `lb_retries 2` alongside `lb_try_duration 5s`. Caddy documents
the duration as taking precedence over the count when both are set, so
`lb_retries 2` would have been silently ignored in the configuration the card
described. At the default 250ms interval a 5s budget is roughly twenty attempts,
not two. The BSL surface therefore exposes `try_duration` and `interval`, which
between them actually determine the attempt count, and no count field.

## Why round-robin stays the default

The PRD names `lb_policy least_conn`. The spec keeps `round_robin` as the default
and offers `least_conn` as one selectable policy, which is deliberate rather than
an oversight.

This project's remit is to give Seedling the surface a production Tamanu needs, not
to adopt Tamanu's own configuration choices as Seedling's defaults. Round-robin is
already Seedling's defined behaviour in `l[service.routing]`; the card owes Tamanu
the *ability* to select least-conn, and nothing more. Values quoted in the PRD's
requirement table should be read the same way throughout this project: as
capabilities to expose, not defaults to change.

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
  That is by design, and passive health checking in the proxy is not the answer
  to it. See below.

Do not widen `lb_retry_match` to pick up the 502 case without revisiting the
idempotency question first.

## Why no passive health checks in the proxy

Caddy's passive health checking (`fail_duration`, `max_fails`,
`unhealthy_status`) must not be enabled, and this is a correctness constraint
rather than a scoping decision.

`r[lifecycle.service.routing-pool]` removes an unhealthy backend from the pool
only when another backend is currently healthy. When none is, every running
backend stays in the pool in degraded mode and a `service_degraded` fault is
filed, on the principle that a single-server platform should never reduce
serving capacity below what is actually available.

Caddy cannot see that rule. It would mark the last backend down after
`max_fails` and then have no upstream at all, answering 502 itself, which is the
precise outcome degraded mode exists to avoid. The runtime's healthchecks are
the mechanism that decides eligibility, and they already do so before the
emitter sees an upstream list.

The two mechanisms compose instead of overlapping:

- The routing pool decides *which* backends Caddy is given.
- Dial-failure retries cover the window between a backend becoming unusable and
  the pool being recomputed on a later reconciliation tick, where Caddy still
  holds an upstream that has since gone away.

## Where the resolved settings surface

`r[service.http.route.proxy-settings.visibility]` is discharged by the `routes`
array on the `http_service` def in `/apps/show`, not by a new endpoint. Routes hang
off the HttpService because that is where `compress` and `balance` are declared and
where `http.route(prefix)` creates the route.

Two things the emitter and the summary must agree on:

- The array reports *resolved* values, so a field the app never set is reported at
  its default rather than omitted. An operator reading the response should not need
  to know the defaults to interpret it.
- The synthesised `/` route for a service with no `http_bindings` appears in the
  array like any other, so the array is never empty for an HTTP service. This is the
  same fallback route `r[service.http.route.routing]` describes.

`ServiceSummary`/`HttpServiceSummary` in `crates/core/src/defs/summary.rs` are where
this lands; `compress` serialises as `null` when compression is off for the route.

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
