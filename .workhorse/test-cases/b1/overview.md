# Edge parity: response compression and upstream retries

Scenarios verifying that every reverse-proxy route compresses its responses and
retries an upstream it cannot reach.

## Declaring settings

- [x] `compress(true)` and `compress(false)` switch compression on and off (verifies spec: service.http.compress)
- [x] `compress(map)` accepts `encodings`, `minimum_length` and `content_types` (verifies spec: service.http.compress.fields)
- [x] An unrecognised encoding, an empty `encodings` list and an empty `content_types` list each throw (verifies spec: service.http.compress.fields)
- [x] A negative `minimum_length` throws (verifies spec: service.http.compress.fields)
- [x] `balance(map)` accepts `policy`, `try_duration` and `interval`, as whole numbers or fractions (verifies spec: service.http.balance)
- [x] An unrecognised policy throws (verifies spec: service.http.balance)
- [x] A zero interval against a non-zero try duration throws (verifies spec: service.http.balance)
- [x] A zero interval is allowed when the try duration is also zero (verifies spec: service.http.balance)

## Resolution

- [x] A field set at neither level resolves to its default (verifies spec: service.http.proxy-settings.resolution)
- [x] A route overrides only the fields it names, keeping the service's values for the rest (verifies spec: service.http.proxy-settings.resolution)
- [x] Setting `compress` leaves `balance` alone, and the reverse (verifies spec: service.http.proxy-settings.resolution)
- [x] A route can switch compression off over an enabling service, and back on over a disabling one (verifies spec: service.http.compress)
- [x] A zero interval meeting a non-zero try duration across two levels is never emitted (verifies spec: service.http.balance)

## Emitted proxy config

- [x] Every reverse-proxy route emits `encode` ahead of the proxy handler (verifies spec: service.http.route.compression)
- [x] Compression defaults emit zstd preferred over gzip at a 512-byte floor (verifies spec: service.http.route.compression)
- [x] The content-type matcher is left out unless the app named its own set (verifies spec: service.http.route.compression)
- [x] A route with compression off emits no `encode` handler (verifies spec: service.http.route.compression)
- [x] Every reverse-proxy route emits a selection policy, try duration and try interval (verifies spec: service.http.route.balancing)
- [x] Redirect routes are emitted with neither handler (verifies spec: service.http.route.compression, service.http.route.balancing)
- [x] Layer4 forwarding is emitted with neither handler (verifies spec: service.http.route.balancing)
- [ ] The synthesised `/` route of a service with no HTTP bindings carries the service's settings (verifies spec: service.http.route.balancing)
- [ ] A site-ingress attachment serves a route with the settings of the app that declares the service

## Behaviour against a running proxy

- [ ] A text response over the floor comes back zstd-encoded to a client offering zstd (verifies spec: service.http.route.compression)
- [ ] A response the upstream already encoded is forwarded as it stands (verifies spec: service.http.route.compression)
- [ ] A response under the floor comes back uncompressed (verifies spec: service.http.route.compression)
- [ ] A request arriving during a rolling update is served by a surviving instance rather than failing (verifies spec: service.http.route.balancing)
- [ ] A POST whose connection could not be established is retried, since nothing reached the pod (verifies spec: service.http.route.balancing)
- [ ] A non-GET whose connection was established but yielded no response is not retried (verifies spec: service.http.route.balancing)
- [ ] An error status from a reachable upstream is returned to the client rather than retried (verifies spec: service.http.route.balancing)

## Reporting

Blocked on the visibility decision, see `.workhorse/design/mockups/b1/`.

- [ ] Resolved settings are readable per route when inspecting the app (verifies spec: app.describe.proxy-settings, service.http.route.proxy-settings.visibility)
- [ ] A service with no HTTP route bindings reports its single `/` route (verifies spec: app.describe.proxy-settings)
