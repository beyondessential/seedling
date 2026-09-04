---
status: draft
---

# Caddy image: add the rate-limit module

Prerequisite for B3 (per-route rate limiting): add the rate-limit module to the prebuilt Caddy image so the running binary understands configs that reference it, move the base to Caddy 2.11.4, and make the version-compatibility discipline the Containerfile describes in prose into something CI actually checks.

## Decisions taken

- **The WAF module is deferred.** BES builds coraza but has it enabled nowhere, because of application compatibility problems. Rather than carry a module the fleet cannot switch on, re-evaluate separately, possibly as an external WAF: Seedling controls the network path far better than the ad-hoc hosts do, so the enforcement point need not be in Caddy at all. This card therefore ships rate limiting only, and B8's hook is settled on that card rather than here.
- **Caddy base moves to 2.11.3 -> 2.11.4.** Lines up with what BES already builds, and 2.11.4 is what `caddy-l4@v0.1.2` requires.

## What the code already settles

- **The image is pinned in three places that must move together.** `CADDY_IMAGE` at `crates/core/src/system/caddy/startup.rs:22`, the two `FROM` lines plus the `xcaddy build` line in `docker/caddy/Containerfile`, and `env.TAG` in `.github/workflows/caddy-image.yml`. Current tag `2.11.3-l4.0.1.1`, one plugin, `caddy-l4@v0.1.1`.
- **Seedling emits Caddy JSON, not a Caddyfile.** `build_caddy_config` in `crates/core/src/system/caddy/config.rs` returns a `serde_json::Value`. So `import waf*` has no direct equivalent here: snippets and `import` are Caddyfile-only constructs. Whatever B8 becomes, it is a handler entry in the emitted document, not an import.
- **A missing module is not a soft failure.** Caddy rejects a JSON config that names an unregistered module at load time. Combined with `r[infra.proxy.upgrade.rollback]`, a config referencing `http.handlers.rate_limit` against an image without the module means the replacement container rejects the config, the upgrade aborts, and the old container stays authoritative. The proxy keeps serving, but the new config never lands and the failure repeats every reconciler tick. This is why the image has to lead B3 rather than follow it.
- **Migration v52 is not touched by this card.** v52 was a one-time wipe that changed what the cache stores: it now holds the Seedling-internal `ProxyConfig`, rebuilt into Caddy JSON at replay time by current code. That is exactly what `r[infra.proxy.upgrade.cache]` requires, and it makes the cache version-independent by construction. A Caddy version or module change needs no new migration. The card's open note on this resolves to "no".
- **Seedling does DNS-01 itself, in Rust.** `crates/core/src/runtime/tls/dns.rs` and `.../dns/route53.rs`. The `caddy-dns/route53` plugin in the BES build is not something Seedling needs, even though the PRD's certificate work leans on Route53.
- **Caddy pinning is implicit and invisible.** `docker/caddy/Containerfile` runs `xcaddy build` with no version argument. That is still pinned, because the official builder image sets `ENV CADDY_VERSION=v2.11.x` and xcaddy reads it. So the `FROM docker.io/library/caddy:2.11.3-builder` tag is what fixes the Caddy version. It works, but nothing in this repo says so, and nothing fails if it stops being true.

## Version compatibility, checked

| Component | Version | Minimum Caddy | Against 2.11.4 |
| --- | --- | --- | --- |
| `mholt/caddy-l4` | v0.1.1 (current) | 2.11.3 | compatible |
| `mholt/caddy-l4` | v0.1.2 (latest) | 2.11.4 | exact match |
| `mholt/caddy-ratelimit` | v0.1.0 (only tag) | 2.8.0 | compatible |

`docker.io/library/caddy:2.11.4` and `:2.11.4-builder` both exist. Moving to 2.11.4 lets caddy-l4 go to v0.1.2 in the same change, and `caddy-ratelimit` has exactly one tag so there is no version judgement to make there.

The emitted module id for rate limiting is `http.handlers.rate_limit`.

## Build strategy: the investigation

Both repos are ours, so this is not a question of whether Seedling can depend on third-party-builds. It is a question of **where the Seedling Caddy build should live**, and the arguments are about release coupling and what Seedling controls, not about trust. An earlier pass of this doc leaned on the repo's stability disclaimer and on the cost of a cross-repo PR. Both are noise: the disclaimer is aimed at outside consumers, and a cross-repo PR is a card and a tab.

### What third-party-builds actually produces

`.github/workflows/caddy.yml` builds Caddy 2.11.4 with `caddy-dns/route53@v1.6.2`, `mholt/caddy-ratelimit@v0.1.0` and `corazawaf/coraza-caddy/v2@v2.5.0`, plus a `--replace` pinning certmagic to the merge commit of certmagic#380. Output is per-target binaries and Debian packages on `tools.ops.tamanu.io` and an APT repo, with build attestations. No OCI image, and no `caddy-l4`.

### Measured

| Binary | Contents | Size |
| --- | --- | --- |
| stock `caddy:2.11.3` | Caddy only | 48.3 MB |
| current `seedling-caddy` | Caddy + l4 | 49.1 MB |
| third-party-builds 2.11.4 | Caddy + route53 + ratelimit + coraza | 60.0 MB |

`caddy-l4` costs 0.8 MB. The three plugins in the BES build cost 11.7 MB together, dominated by coraza carrying the OWASP CRS. The PRD notes migrating hosts are often on a poor link, so that is a real if modest pull cost for a module enabled nowhere.

### The case for moving the build into third-party-builds

- **One place to bump on a Caddy CVE.** Today a Caddy security release needs someone to remember Seedling builds its own. That is the sort of thing that gets missed precisely when it matters.
- **Release engineering already exists there.** third-party-builds attests its artefacts with `actions/attest-build-provenance`. `caddy-image.yml` attests nothing and pushes straight to GHCR. Consolidating hardens one build instead of leaving two half-hardened.
- **Version skew during migration is an operational hazard.** The PRD's migration puts the ad-hoc Caddy and the Seedling Caddy on the same host at the same time. Two Caddy versions behaving differently between the ad-hoc vhost and the Seedling vhost is a bad thing to debug mid-cutover.
- **If ops ever wants L4 proxying**, the split stops making sense at all.

### The case against

- **Seedling structurally cannot cede its Caddy version, which is most of the prize.** `CADDY_IMAGE` is a compile-time constant, so a Caddy bump is a Seedling release, and it drives a blue/green container swap on every host. `caddy-l4` pins a minimum Caddy (v0.1.2 requires exactly 2.11.4), so Seedling has to gate on the plugin compatibility window whoever builds. third-party-builds tracks what ops needs, including downgrading to 2.10 in March 2026 "because they broke it". Consuming `latest` means inheriting ops' version decisions on a fleet-wide proxy swap; pinning a version URL means doing Seedling's own bumps anyway. Either way the CVE-bump argument above only saves *deciding* the version and *maintaining the recipe*, not the Seedling-side release. The recipe is four lines, and xcaddy comes free in the official builder image.
- **The two builds want opposite things, and consolidating means everyone carries everything.** Seedling needs l4 and not fd-passing; the ad-hoc hosts need fd-passing and not l4. Measured, that is 0.8 MB one way and 11.7 MB the other. A shared binary either taxes both, or grows build variants, at which point it is two builds again sharing a file.
- **A container image is a different artefact to a deb.** third-party-builds is a binaries-and-packages pipeline with an S3 and APT publishing model. Adding an OCI target is not hard, but it is new machinery there rather than reuse of existing machinery.

### What is genuinely worth taking from third-party-builds

Not the binary. Two other things:

- **Attestation.** `caddy-image.yml` should attest what it publishes, the way third-party-builds already does.
- **Version parity as a deliberate choice.** Keeping Seedling's Caddy version equal to the BES build's gets the migration-skew benefit without the coupling: one Caddy version on the host, two binaries with different plugin sets. That is a check, not a dependency.

### The certmagic replace, honestly

An earlier pass overstated this. certmagic#380 is about listening on passed file descriptors and cannot affect the on-disk storage layout that `cert_observation.rs` globs, so the specific patch is inert for Seedling. What remains is thinner: a shared build means Seedling ships whatever replaces the ad-hoc hosts need, on a library whose on-disk output Seedling parses. Worth knowing, not worth deciding on.

### Recommendation

Keep the build in Seedling, and close the two gaps that make the split look worse than it is: attest the image, and assert version parity with the BES build in the capability check. Revisit consolidation if ops ever needs L4, which is the point where the two builds genuinely converge.

## Adjacent defects found

Neither is caused by this card, both are made slightly worse by it, and both are cheap to fix while the files are open.

- **The image ships two Caddy binaries.** `docker/caddy/Containerfile` layers `COPY --from=builder /usr/bin/caddy` over `FROM docker.io/library/caddy:2.11.3`, whose own 48.3 MB caddy stays in the lower layer and is still pulled. That is 34.7 MB compressed of Caddy binary per pull, about half of it dead. A final stage that does not already contain a Caddy would halve it, which matters on the poor links the PRD describes.
- **Merging a Containerfile change leaves `main` pointing at an image tag that does not exist yet.** `caddy-image.yml` triggers on push to `main`, and recent runs take 16 to 19 minutes (multi-arch, arm64 under qemu). `CADDY_IMAGE` lives under `crates/`, which is not in the workflow's `paths`, so it cannot trigger the build itself. In that window a daemon built from `main` fails in `ensure_caddy_running` when `pull_image` cannot find the tag. Same-repo does not avoid the ordering constraint that a cross-repo build would impose, it just hides it.

## Open questions

- [ ] **Confirm the recommendation: keep the build in Seedling.** The measured deltas and the version-control argument are in Build strategy above.
- [ ] **Scope of the two adjacent defects.** Fix the double-shipped binary and the missing-tag window here, or split them out. Both touch the same two files this card already edits.
- [ ] **Shape of the capability check.** Where it runs, and whether the required-module list is derived from the code or hand-maintained. See Testing notes.
- [ ] **Tag scheme.** `2.11.3-l4.0.1.1` names its one plugin. With two plugins that becomes unwieldy, and it gets worse with each one. Options: keep extending it (`2.11.4-l4.0.1.2-rl0.1.0`), drop to the Caddy version plus a build serial (`2.11.4-2`), or use the Caddy version plus a content hash of the build line.

## Trade-offs

- **Deferring the WAF costs a second image bump later.** Accepted. Measured, coraza and the rest of the BES plugin set cost 11.7 MB of binary, for a capability enabled nowhere and which may end up enforced outside Caddy entirely. Tracked on S1.
- **Keeping the build in Seedling costs a second place to bump on a Caddy CVE.** Accepted, but it is the real cost of the split and worth naming rather than arguing away. Version parity with the BES build, asserted in the capability check, is what keeps the two from drifting silently.
- **Version parity is a check, not a dependency.** One Caddy version on a migrating host, two binaries with different plugin sets. Gets the migration-skew benefit of consolidation without ceding a version Seedling has to control.

## Testing notes

- The image `CADDY_IMAGE` names registers every module Seedling's emitted JSON can reference, asserted against a required set rather than a snapshot so adding a module does not fail the check.
- Every module id `build_caddy_config` can emit appears in that required set, so the two halves cannot drift apart in the repo.
- The built image reports the expected Caddy version, so a base-image change that moves `CADDY_VERSION` is caught rather than shipped.
- The image's Caddy version matches the BES build's, so the two drift only deliberately.
- An emitted config naming the rate-limit module is accepted by the pinned image.
- Bumping `CADDY_IMAGE` drives a blue/green upgrade rather than a restart, and the cached config replays without a migration.
- The image contains exactly one Caddy binary.
- `main` never references a `seedling-caddy` tag that has not been published.
