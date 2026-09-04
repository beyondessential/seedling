# Caddy image: add the rate-limit module

Technical notes and build steps for D1. Reasoning behind the decisions is in the working doc at `.workhorse/working-docs/d1/working-doc.md`.

## Where the build lives, and why

The build stays in this repo rather than moving to `beyondessential/third-party-builds`.

Consolidating would buy one place to decide the Caddy version and one recipe to bump on a CVE. Seedling cannot take that deal, because it cannot cede the version: `CADDY_IMAGE` is a compile-time constant, so a Caddy bump is a Seedling release, and it drives a blue/green container swap on every host. `caddy-l4` pins a minimum Caddy, so Seedling gates on the plugin compatibility window whoever builds. Consuming a floating tag means inheriting ops' version decisions on a fleet-wide proxy swap; pinning a version means doing Seedling's own bumps anyway. What is left of the prize is not maintaining four lines of `xcaddy` invocation.

The two builds also want opposite things, measured: `caddy-l4` costs 0.8 MB and the ad-hoc hosts do not use it; the BES plugin set costs 11.7 MB and Seedling does not use it. A shared binary taxes both or grows variants until it is two builds sharing a file.

The real cost of the split is a second place to bump on a Caddy CVE. That is mitigated by asserting version parity with the BES build in the capability check, not by depending on it. Revisit consolidation if ops ever needs L4 proxying, which is the point where the two builds genuinely converge.

## Version compatibility

`caddy-l4@v0.1.2` requires Caddy 2.11.4 exactly; `caddy-ratelimit@v0.1.0` requires 2.8.0 and is the only tag. Both `caddy:2.11.4` and `caddy:2.11.4-builder` exist.

Caddy pinning is implicit: the Containerfile runs `xcaddy build` with no version argument, and the official builder image sets `CADDY_VERSION`, so the `FROM` tag is what fixes the version. Nothing in this repo says so and nothing fails if it stops being true, which is part of what the capability check is for.

## The base-image change has one trap

The official image sets `XDG_DATA_HOME=/data`, which is what puts certmagic's storage at `/data/caddy`, which is the path `cert_observation.rs` globs. A final stage that does not carry that env forward relocates every certificate silently: Caddy still serves, certificates are still obtained, and the runtime simply stops seeing them, stalling `rt.warm_certs` barriers. `XDG_CONFIG_HOME=/config` matters for the same reason, and `/etc/caddy` must exist because the admin config is bind-mounted into a read-only rootfs.

The binary is statically linked, so the final stage needs no matching libc, but it does need a CA bundle carried over: ACME talks TLS to the issuer. `alpine` costs 3.84 MB and keeps a shell for `podman exec`; `scratch` is viable and cheaper.

Expected result is about 21.7 MB compressed against 41.5 MB today.

## Tag scheme

`2.11.4-N`: Caddy version, then a serial bumped when the build changes without the Caddy version changing. The Containerfile stays the record of what is in the build. The serial is forgettable by construction, so a check enforces it.

## Build steps

- [ ] Move `docker/caddy/Containerfile` to Caddy 2.11.4 and `caddy-l4` to v0.1.2
- [ ] Add `--with github.com/mholt/caddy-ratelimit@v0.1.0`, and `http.handlers.rate_limit` to `required-modules.txt` in the same change: the list describes a contract the image satisfies, not one it is expected to grow into
- [ ] Replace the final stage with one that does not already contain a Caddy, carrying `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, a CA bundle, and `/etc/caddy`
- [ ] Update the Containerfile header: it currently describes one plugin and the version discipline in prose
- [x] Declare the required module set, in `docker/caddy/required-modules.txt`, read by the runtime and by the image check so the two cannot disagree
- [x] Declare the certificate cache directory once, in `crates/core/src/system/caddy/image.rs`, and have `cert_observation.rs` read it from there rather than hardcoding it
- [x] Add the Rust test: build an everything-on `ProxyConfig`, walk the emitted JSON for module ids, assert each is declared. The fixture is maximal by intent and says so, because a feature it does not exercise contributes no module id
- [ ] Add the image check to `caddy-image.yml`: assert the built image registers the required set, and reports the expected Caddy version, before the push
- [ ] Assert the image's Caddy version matches the BES build's
- [ ] Adopt the `2.11.4-N` tag in the workflow and in `CADDY_IMAGE`
- [ ] Add a pull-request job that builds on changes to `docker/caddy/**`, pushes only when the head repository is this repository (a fork PR gets a read-only `GITHUB_TOKEN`), and fails when the Containerfile moved but `TAG` did not
- [ ] Attest the published image, as `third-party-builds` does for its artefacts
- [ ] Bump `CADDY_IMAGE` at `crates/core/src/system/caddy/startup.rs:22`

## Not in this card

- **Emitting rate-limit configuration.** B3. This card only makes the module available; nothing emits `http.handlers.rate_limit` yet.
- **The WAF hook.** Deferred to S1, including whether enforcement belongs in Caddy at all.
- **A schema migration.** v52 already made the config cache version-independent by storing `ProxyConfig` rather than rendered Caddy JSON, so a Caddy version or module change needs no new migration.

## The spec-first flow collides with the tracey gate

`.github/workflows/tracey.yml` fails a pull request unless every rule in `docs/spec` carries an `r[impl ...]` reference, and tracey 1.3.0 has no spec-side marker for a rule that is planned but not yet built. So a rule cannot land ahead of its implementation, which is what the Workhorse spec-first flow produces while a card is still being specified.

The two rules this card adds are therefore accompanied by the declarations that implement them, rather than waiting for the rest of the build steps. That is a real constraint on how cards in this repo are sequenced, not a one-off: any card that adds a rule has to bring at least that rule's implementation with it.
