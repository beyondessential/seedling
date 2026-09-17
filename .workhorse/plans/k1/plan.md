# K1 — Path-level redirect within an ingress

Notes from the spec interview. Spec rules added: `l[service.http.route.redirect]`
(docs/spec/language.md) and `r[service.http.route.redirect]` (docs/spec/runtime.md),
plus amendments to the compress / rate-limit / headers rules in both files.

## The structural problem

The emitter already has the vocabulary: `ProxyRouteHandler::Redirect` carries a url,
a code and a preserve-path flag, and `proxy_routes_for_vhost`
(crates/core/src/system/caddy/config.rs:262) emits it. The gap is upstream of that.

`build_proxy_config` (crates/core/src/system/translate/proxy.rs:156) builds a vhost's
routes from `upstream.routes`, which are pod `http_bindings` — a prefix becomes a route
only because some pod bound it. A redirect route has no pod. Worse, the
`if upstream.routes.is_empty()` arm falls back to a single `/` catch-all through the
service IP, so a service whose only declaration were a redirect would silently proxy
everything instead of redirecting.

So a redirect route has to reach the translator from the service declaration
(`HttpServiceDef.routes`, crates/core/src/defs/service.rs:344) rather than from pod
bindings. That is the substance of this card, not the builder method.

## Tail carrying needs two handlers

Caddy has no placeholder for "the path remainder after a matched prefix". The tail is
obtained by stripping the matched prefix ahead of the redirect — what the Caddyfile's
`handle_path` is sugar for (`uri strip_prefix`) — and then reading the request URI.
So a tail-carrying redirect emits a two-handler chain within the route.

The `ReverseProxy` arm of `proxy_routes_for_vhost` already builds a chain of up to four
handlers, so the shape is established; the `Redirect` arm currently emits a single
handler and grows to match.

A redirect naming neither `<tail>` nor `<query>` needs no strip and stays one handler.

## Why our own token syntax

`header_value` (crates/core/src/defs/service/proxy.rs:1021) already refuses any `{` in a
BSL header value, because the proxy substitutes a braced word naming its own state and
the names it answers to include its environment. A redirect target lands in the
`Location` header — the same wire position that refusal protects. Accepting Caddy
placeholders there would let a script write `{env.SECRET}` into a `Location` and hand the
daemon's environment to any client hitting the path.

Hence `<tail>` and `<query>`, translated by the runtime, with an unrecognised token
throwing during script evaluation where the error still names the line.

Angle brackets were chosen over `${...}` because rhai's own string interpolation is
`${` inside backtick strings (rhai-1.25.1/src/tokenizer.rs:1437, with `\${` as the
escape): a script writer reaching for backticks out of habit would have `tail` evaluated
as an undefined variable before the target ever reached us.

## Build steps

- [x] `Redirect` carried on the declaration: a redirect arm on `ProxySettings` (or a
      sibling map on `HttpServiceDef`) so a prefix can hold a redirect instead of
      proxy settings
- [x] `redirect()` on `HttpServiceRoute` in three forms, with target and code
      validation, token parsing, and the `/`-prefix refusal
- [x] Refuse a prefix that is both a redirect and a pod binding. Note the ordering
      trap: the binding may be declared after the redirect or before it, so the check
      needs to fire from both sides
- [x] Refuse route-level compress / balance / rate_limit, and request-direction
      headers, on a redirect route; ignore the same settings when inherited from the
      service. Refuse a `Location` response operation on a redirect route
- [x] Carry redirect routes through `ServiceUpstream` into `build_proxy_config`
      independently of pod bindings, and stop the empty-bindings `/` fallback firing
      for a service that has them
- [x] Emit the strip-prefix + redirect chain, and keep the ingress-level HTTP→HTTPS
      redirect ahead of redirect routes on the plaintext vhost
- [x] Surface redirect routes in `app.describe` alongside the proxy settings
      (`r[service.http.route.proxy-settings.visibility]` covers settings; a redirect
      route reports a target and code instead)
- [x] Tracey annotations on each of the above, and `tracey query status` clean

## What landed

The handler's vocabulary did grow after all: a route redirect carries a parsed
target and the route's response header operations, so it is a variant of its
own (`ProxyRouteHandler::RouteRedirect`) rather than the site-ingress
`Redirect` reused. The site-ingress one answers for a whole hostname with a URL
fixed at declaration and preserves the path through `{http.request.uri}`
without stripping anything; neither half of that fits a prefix-bound redirect.

### The strip-prefix approach was abandoned

The first cut took the prefix off the request with a `rewrite`
(`strip_path_prefix`) and read the remainder back as `{http.request.uri}`. That
is wrong for the second-most-common request there is: one for exactly the
prefix. Caddy's `changePath` leaves the path empty, then `canonicalizePath`
re-anchors it, so `/v1/login` under prefix `/v1/login` reports a path of `/`
rather than nothing, and the target gains a trailing slash the spec says it
must not have. Reading the tail off `{http.request.uri.path}` had a second
fault: that placeholder is the *decoded* path, so a `%3F` in a path segment
would reach the client as the `?` that starts a query.

Both are fixed by cutting the tail and the query straight out of the escaped
request line with one `map` handler, anchored on the prefix. It is also one
handler rather than two, and it defines both tokens at once because they are
one cut of one string. Only `http.handlers.map` is required; `rewrite` is not
used.

The pattern is case-insensitive, because the path matcher that selects the
route is, and excludes braces from both captures, because the request line is
written by a client and a braced word in a `Location` is read as naming the
proxy's own state.

### A prefix is one prefix

`HttpServiceDef` keys one map on the prefix, to a `RouteDecl` carrying the
declared settings and a `RouteKind` of `Proxied { bound }` or `Redirect`. An
earlier cut had three prefix-keyed collections (`routes`, `redirects`,
`bound_prefixes`) whose disjointness four separate checks had to defend. The
prefix is normalised once, at `route()`, so the declaration checks and the
emitted matcher agree on what one prefix is — without that, `/v1/login/` and
`/v1/login` passed the either-redirected-or-proxied check as two prefixes and
then claimed the same requests, and `//` slipped past the root guard to match
every request on the hostname.

`with_route_settings` is the one place a route-level setting is applied, so a
setting added later cannot reach a redirect route without its author saying
which it is.

### The `/` fallback stays

A redirect sits above the fallback rather than in place of it. Suppressing the
fallback because a redirect exists took the catch-all away from a service
served through its routing pool, leaving every path but the redirected one
unanswered. Routes are emitted longest-prefix-first and terminal and a redirect
can never be declared at the root, so the fallback cannot shadow one.
