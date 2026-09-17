# K1 — Path-level redirect within an ingress

Scenarios verifying that one path inside a vhost can redirect while the vhost
otherwise proxies. The production case is `route("/v1/login").redirect("/api/login", 308)`
on a service whose `/` is served by a pod.

## Declaration

- [x] The positional form carries the path tail and the query, and defaults to 307 (verifies spec: `l[service.http.route.redirect]`)
- [x] The two-argument positional form takes the code it names
- [x] The map form is served exactly as written: naming no token redirects every request under the prefix to that one target
- [x] The map form accepts `<tail>` and `<query>` written out, in any arrangement
- [x] A redirect is declarable on an external service's routes
- [x] A second `redirect()` on the same route replaces the first
- [x] A target that is neither a path nor an absolute URL throws
- [x] A target beginning `//` throws, naming another host while reading as a path
- [x] A target containing `{` throws, and the error names `<tail>` / `<query>`
- [x] An unrecognised token throws and names it; a stray `<` throws and points at `%3C`
- [x] A code outside 301 / 302 / 307 / 308 throws
- [x] The map form throws on an unrecognised field, and on a missing `to`
- [x] A redirect on `/` throws

## Clashes

- [x] A prefix declared as a redirect and then bound by a pod throws
- [x] A prefix bound by a pod and then declared as a redirect throws
- [x] Route-level `compress` / `balance` / `rate_limit` on a redirect route throws, whichever order they are written in
- [x] A `request` header operation on a redirect route throws, whichever order
- [x] A `response` operation naming `Location` on a redirect route throws, whichever order
- [x] The same settings declared on the Service are ignored on its redirect routes rather than refused
- [x] A `response` operation not naming `Location` is accepted on a redirect route

## Emission

- [x] A tail-carrying redirect emits a strip-prefix handler and a static response whose `Location` is the target plus the request line (verifies spec: `r[service.http.route.redirect]`)
- [x] A target naming nothing emits one handler and serves the target as declared
- [x] A tail carried without a query drops the query rather than smuggling it through the combined placeholder
- [x] A query carried on its own maps to nothing when the request has none, and to `?` plus itself when it does
- [x] A redirect route's response header operations are emitted first, deferred (verifies spec: `r[service.http.route.headers]`)
- [x] A redirect on a longer prefix is emitted ahead of a proxied route on a shorter one, and is terminal
- [x] A service whose only routes are redirects does not take the `/` fallback
- [x] A redirect is emitted alongside the prefixes a pod binds
- [x] On the plaintext vhost of an ingress declaring an HTTP redirect, that redirect answers every path and the redirect route is not emitted there
- [x] A cached proxy document carrying a redirect route round-trips
- [x] Every module the emitted document names is declared in `required-modules.txt`

## Visibility

- [x] `app.describe` reports a redirect route's target and code, reports it `served` without a pod, and reports its `compress` and `rate_limit` null (verifies spec: `i[app.describe.proxy-settings]`)

## Manual

- [ ] Against a real proxy image: `/v1/login/reset?token=x` under `redirect("/api/login")` arrives at `/api/login/reset?token=x`
- [ ] Against a real proxy image: a request to exactly `/v1/login` arrives at `/api/login`
- [ ] Against a real proxy image: `<query>` on its own adds no bare `?` when the request carries no query
- [ ] A plaintext request to `http://host/v1/login` is moved to HTTPS first, then redirected
