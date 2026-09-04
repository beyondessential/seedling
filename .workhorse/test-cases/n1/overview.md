# Test cases: plaintext ingress is served

Coverage for a plaintext HTTP ingress reaching traffic, so a `.local` host can be served by a
site ingress while the app declares no ingress of its own. Scenarios use generic hostnames; the
Tamanu topology appears only as sample data.

## Listener derivation (translate layer)

- [ ] A plaintext HTTP-terminating ingress on :80 registers an HTTP listener on :80.
      verifies spec: actuate.ingress.plaintext
- [ ] A plaintext HTTP-terminating ingress registers no QUIC listener on its port.
      verifies spec: actuate.ingress.plaintext
- [ ] A TLS-terminating HTTP ingress on :443 still registers both an HTTPS and a QUIC listener,
      so the fix does not regress the existing path.
      verifies spec: actuate.ingress.plaintext
- [ ] A TLS-passthrough ingress (`Terminate.Tls + Output.Tcp`, no HTTP termination) is unchanged.

## Caddy config (actuation layer)

- [ ] A plaintext vhost plus its HTTP listener produces a `seedling_http` server whose routes
      carry the vhost's upstreams.
      verifies spec: actuate.ingress.plaintext
- [ ] A plaintext-only configuration emits no `apps.tls` block, so no certificate is requested
      for the hostname.
      verifies spec: actuate.ingress.plaintext
- [ ] A plaintext vhost gets no redirect route, since it declares no redirect.
      verifies spec: actuate.ingress.plaintext

## End to end through the site-ingress path

- [ ] A site ingress with `tls_provider: none` and an HTTP forward attachment on :80 produces a
      Caddy config that serves the hostname over plaintext HTTP to the target app service.
      This is the scenario the card exists for, and it currently yields an empty config.
      verifies spec: actuate.ingress.plaintext
- [ ] The same site ingress with `tls_provider: internal` still terminates TLS and requests a
      certificate from the internal CA rather than public ACME.
      verifies spec: tls.strategy.default
- [ ] An app that declares no ingress installs successfully with a site ingress carrying its
      traffic, and `rt.warm_certs(app)` does not block the install.
      verifies spec: rt.warm-certs

## Mixed termination on one hostname

- [ ] One hostname with a plaintext :80 ingress and a TLS-terminating :443 ingress yields two
      separate vhosts, each served from the listener matching its own termination.
      verifies spec: actuate.ingress.plaintext
- [ ] In that mixed case the plaintext route is not served from the HTTPS server, and the
      plaintext hostname is not added to the TLS automation subjects.
      verifies spec: actuate.ingress.plaintext

## Forwarding rules

- [ ] A plaintext HTTP ingress forwards TCP only; no UDP forwarding rule is emitted for its port.
      verifies spec: actuate.ingress.plaintext
- [ ] A TLS-terminating HTTP ingress still forwards both TCP and UDP, covering HTTP/3.
      verifies spec: actuate.ingress.plaintext

## Manual verification

- [ ] On a host with a `.local` hostname, install an app that declares no ingress, attach a
      plaintext site ingress, and confirm the hostname serves over HTTP with no certificate
      issued and no redirect to HTTPS.
