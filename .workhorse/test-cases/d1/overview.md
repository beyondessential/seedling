# Test cases: Caddy image, rate-limit module

Coverage this card owes. An unticked box is a scenario not yet covered, not one decided against.

## The image provides what the runtime emits

- [x] Every module id the emitted proxy configuration can carry is in the runtime's declared required set, asserted from a fixture exercising every feature `build_caddy_config` supports (verifies spec: `infra.proxy.image.modules`)
- [ ] The built image registers every module in the required set, checked before publication rather than after (verifies spec: `infra.proxy.image.modules`)
- [x] Adding a handler to the emitted configuration without declaring its module fails the test suite (verifies spec: `infra.proxy.image.modules`)
- [ ] A configuration naming `http.handlers.rate_limit` is accepted by the pinned image
- [ ] Dropping a `--with` from the build fails the image check rather than producing a publishable image

## Certificates survive the base-image change

The base change is the one part of this card that can regress silently: the proxy keeps serving and keeps obtaining certificates while the runtime stops seeing them.

- [x] Certificates obtained by the proxy land where the runtime observes them, so `cert_valid` observations are emitted after the base-image change (verifies spec: `infra.proxy.image.cert-cache`)
- [ ] An `rt.warm_certs` barrier resolves against a certificate obtained by the new image (verifies spec: `infra.proxy.image.cert-cache`)
- [ ] Default-strategy certificate metadata, issuer and expiry, is still derived from the proxy's cache (verifies spec: `tls.cert.metadata`)
- [ ] The proxy can complete an ACME issuance from the new base, which requires a CA bundle in the final stage

## Version discipline

- [ ] The built image reports the expected Caddy version, so a base-image change that moves `CADDY_VERSION` is caught rather than shipped
- [ ] The image's Caddy version matches the BES `third-party-builds` build, so the two drift only deliberately
- [ ] A pull request touching `docker/caddy/Containerfile` without moving `TAG` fails

## Release mechanics

- [ ] `main` never references a `seedling-caddy` tag that has not been published
- [ ] A pull request from a fork builds the image without attempting a push
- [ ] The published image is attested

## Upgrade path

- [ ] Bumping `CADDY_IMAGE` drives a blue/green upgrade rather than a restart (verifies spec: `infra.proxy.upgrade`)
- [ ] The cached configuration replays onto the new container without a schema migration (verifies spec: `infra.proxy.upgrade.cache`)
- [ ] The image contains exactly one Caddy binary

## Manual

- [ ] Pull the published image on a slow link and confirm the transfer is roughly half what the previous tag cost
- [ ] `podman exec` into a running proxy container still works for diagnosis after the base change
