# G1: header manipulation rules

Scenarios verifying the `headers()` surface, its resolution, what the proxy is
told to do, and the HTTP/1.1 persistent-connection guarantee.

Ticked cases are covered by automated tests in this branch. The unticked ones
need a running proxy, which this repo has no harness for: the unit tests assert
the document Seedling emits, not what Caddy does with it.

## Declaring headers

- [x] A service declares request and response operations and each direction keeps its own (verifies spec: `service.http.headers`)
- [x] A declaration naming neither direction is refused, and a misspelled direction is reported as the unknown key it is (verifies spec: `service.http.headers`)
- [x] A direction naming no operation is refused (verifies spec: `service.http.headers.fields`)
- [x] A value is accepted as a bare string or as an array, and both produce the same shape (verifies spec: `service.http.headers.fields`)
- [x] An empty value array is refused, pointing at `remove`; an empty `remove` is refused (verifies spec: `service.http.headers.fields`)
- [x] A value containing CR or LF is refused, so a declaration cannot inject a header of its own (verifies spec: `service.http.headers.fields`)
- [x] A name outside the HTTP token characters, and an empty name, are refused (verifies spec: `service.http.headers.fields`)
- [x] Every proxy-owned header is refused: the eight connection headers and `Content-Length` (verifies spec: `service.http.headers.fields`)
- [x] One name under two operations in a direction is refused, including when the two spellings differ in case (verifies spec: `service.http.headers.fields`)
- [x] A name is reported back with the spelling the app used (verifies spec: `service.http.headers.fields`)
- [x] `headers()` is reachable on both an HTTP Service and an HTTP Service Route from a script (verifies spec: `service.http.headers`)

## Resolving service against route

- [x] A route overrides the headers it names and keeps the rest of the service's (verifies spec: `service.http.proxy-settings.resolution`)
- [x] A route may override a service's operation with a different operation, replacing it rather than combining (verifies spec: `service.http.proxy-settings.resolution`)
- [x] An override is recognised when route and service spell the header differently, leaving one operation (verifies spec: `service.http.proxy-settings.resolution`)
- [x] A route declaring no headers inherits the service's (verifies spec: `service.http.proxy-settings.resolution`)
- [x] Declaring headers leaves compression, balancing, and the rate limit undisturbed (verifies spec: `service.http.proxy-settings.resolution`)

## What the proxy is told

- [x] `replace` is emitted as the proxy's `set`, never its substring-substituting `replace` (verifies spec: `service.http.route.headers`)
- [x] `add` and `remove` emit as `add` and `delete`; several values stay several (verifies spec: `service.http.route.headers`)
- [x] A `Host` replacement travels as a request operation (verifies spec: `service.http.route.headers`)
- [x] The handler is ordered ahead of the rate limiter with its response operations deferred (verifies spec: `service.http.route.headers`)
- [x] A direction with no operations is left out, and a route with no headers carries no handler at all (verifies spec: `service.http.route.headers`)
- [x] `http.handlers.headers` is exercised by the image fixture, so it is caught if missing from the image (verifies spec: `infra.proxy.image.modules`)
- [x] A config cached before header manipulation existed still deserialises (verifies spec: `infra.proxy.upgrade.cache`)

## Visibility

- [x] Each route reports the headers in force on it, whether it or the service declared them (verifies spec: `app.describe.proxy-settings`)
- [x] A direction with nothing declared reports its three operations empty rather than absent (verifies spec: `app.describe.proxy-settings`)
- [x] A value declared as a bare string reads back as a one-element array (verifies spec: `app.describe.proxy-settings`)
- [ ] The web UI's route row shows the per-direction operation counts, and "headers: none" where there are none

## Against a running proxy

None of these are automated: the repo has no harness that starts Caddy, so
every case below asserts behaviour the unit tests can only approximate by
checking the emitted document.

- [ ] A pod behind a route replacing `Host` observes the name the route set, not the hostname the client used (verifies spec: `service.http.route.headers`)
- [ ] A rate-limit rejection carries the route's response headers. This is what the deferred, limiter-ahead ordering is for, and it is the one part of the design not confirmable from the emitted document alone (verifies spec: `service.http.route.headers`)
- [ ] A failure to reach any upstream carries the route's response headers (verifies spec: `service.http.route.headers`)
- [ ] Two `Set-Cookie` values arrive at the client as two header lines rather than one folded value (verifies spec: `service.http.route.headers`)
- [ ] A redirect response carries no header operations (verifies spec: `service.http.route.headers`)
- [ ] An HTTP/1.1 client issues successive requests on one connection without the proxy closing it between them (verifies spec: `ingress.persistent-connections`)
- [ ] An HTTP/2 response carries no connection-management header, so it stays well-formed (verifies spec: `ingress.persistent-connections`)
