---
status: draft
---

# Caddy image: add the rate-limit and WAF modules

Prerequisite for B3 (per-route rate limiting) and B8 (WAF hook): add the rate-limit and WAF modules to the prebuilt Caddy image so the running binary understands configs that reference them, then bump the pinned tag with the version-compatibility discipline the Containerfile already requires.

## What the code already settles

Read before interviewing, so these are findings rather than assumptions.

- **The image is pinned in three places that must move together.** `CADDY_IMAGE` at `crates/core/src/system/caddy/startup.rs:22`, the `FROM` lines plus the `xcaddy build` line in `docker/caddy/Containerfile`, and `env.TAG` in `.github/workflows/caddy-image.yml`. Current tag `2.11.3-l4.0.1.1`, one plugin, `caddy-l4@v0.1.1`.
- **Seedling emits Caddy JSON, not a Caddyfile.** `build_caddy_config` in `crates/core/src/system/caddy/config.rs` returns a `serde_json::Value`. So `import waf*` has no direct equivalent here: snippets and `import` are Caddyfile-only. B8 has to be re-expressed as a handler slot in the emitted JSON.
- **A missing module is not a soft failure.** Caddy rejects a JSON config that names an unregistered module at load time. Combined with `r[infra.proxy.upgrade.rollback]`, a config referencing `rate_limit` against an image without the module means the replacement container rejects the config, the upgrade aborts, and the old container stays authoritative. The proxy does not break, but the new config never lands and the failure repeats every tick.
- **Migration v52 is not touched by this card.** v52 was a one-time wipe that changed what the cache stores: it now holds the Seedling-internal `ProxyConfig`, rebuilt into Caddy JSON at replay time by current code. That is exactly what `r[infra.proxy.upgrade.cache]` requires, and it makes the cache version-independent by construction. A Caddy version or module change needs no new migration. The card's open note on this resolves to "no".
- **Seedling does DNS-01 itself, in Rust.** `crates/core/src/runtime/tls/dns.rs` and `.../dns/route53.rs`. The `caddy-dns/route53` plugin in the BES build is therefore not something Seedling needs, even though the PRD's certificate work leans on Route53.

## The BES third-party-builds option

`beyondessential/third-party-builds` `.github/workflows/caddy.yml` builds Caddy 2.11.4 with:

- `github.com/caddy-dns/route53@v1.6.2`
- `github.com/mholt/caddy-ratelimit@v0.1.0`
- `github.com/corazawaf/coraza-caddy/v2@v2.5.0`
- a `--replace` pinning certmagic to the merge commit of certmagic#380, for listening on passed file descriptors

Two gaps against Seedling's needs:

1. **It has no `caddy-l4`.** Seedling's whole reason for a custom image.
2. **It publishes binaries and debs to S3, not a container image.** Seedling pulls an OCI image at runtime and never builds one.

The certmagic replace exists for socket activation (`caddy.socket`, `default_bind fd/4`), which is how the ad-hoc hosts run Caddy. Seedling's containers bind ports directly, so that patch is probably surplus here, but it is a real difference in the binary either way.

The WAF pieces on the ad-hoc side are config, not build: `caddy-files/config/waf` is `order coraza_waf first` and `caddy-files/snippets/waf` defines a `(waf)` snippet running `coraza_waf` with the OWASP CRS and `SecRuleEngine On`. That is the thing `import waf*` reaches for.

## Open questions

- [ ] **Build strategy.** Own image with two more plugins, reuse the BES build, or own image plus a drift test against BES. See Implementation options.
- [ ] **Which WAF module, and does B8's default state need it present at all?**
- [ ] **Caddy base version.** Hold at 2.11.3, or move to 2.11.4 to line up with the BES build.
- [ ] **What "confirm the version-compatibility discipline" means concretely.** The Containerfile header states it as prose. Does this card make it mechanical.
- [ ] **Shape of the capability test.** What it asserts, and where it runs.

## Implementation options

### A. Own image, more plugins

Add `--with github.com/mholt/caddy-ratelimit@vX` and the chosen WAF module to the existing `xcaddy build` line, bump the tag in all three places. Smallest diff, no cross-repo coordination, and the tag scheme (`2.11.3-l4.0.1.1`) needs extending to name more plugins or to stop naming them.

### B. Reuse the BES build

Requires adding `caddy-l4` to third-party-builds' caddy, and either publishing an image from there or having Seedling's Containerfile pull the built binary from S3. Puts one Caddy binary across the fleet, at the cost of coupling Seedling's proxy to another repo's release cadence, and of a build whose certmagic replace Seedling does not need.

### C. Own image, plus a parity test

As A, but with a test in this repo asserting the image provides the module set Seedling requires, so an upstream change that drops or renames something is caught in CI rather than at a customer's cutover. The card's own suggestion.

## Testing notes

- The image provides every module Seedling's emitted JSON can reference. Worth asserting against a required-module list rather than a snapshot, so adding a module to the image does not fail the test.
- An emitted config that names the rate-limit module is accepted by the pinned image.
- Bumping `CADDY_IMAGE` drives a blue/green upgrade rather than a restart, and no new migration is required for the cached config to replay.
