---
status: complete
---

# Caddy image: add the rate-limit module

Prerequisite for B3 (per-route rate limiting): add the rate-limit module to the prebuilt Caddy image so the running binary understands configs that reference it, move the base to Caddy 2.11.4, and make the version-compatibility discipline the Containerfile describes in prose into something CI actually checks.

## Decisions taken

- **The WAF module is deferred.** BES builds coraza but has it enabled nowhere, because of application compatibility problems. Rather than carry a module the fleet cannot switch on, re-evaluate separately, possibly as an external WAF: Seedling controls the network path far better than the ad-hoc hosts do, so the enforcement point need not be in Caddy at all. This card therefore ships rate limiting only, and B8's hook is settled on that card rather than here.
- **Caddy base moves to 2.11.3 -> 2.11.4.** Lines up with what BES already builds, and 2.11.4 is what `caddy-l4@v0.1.2` requires.

- **The build stays in Seedling.** Seedling has to control its own Caddy version: it is compiled into the daemon and a bump drives a blue/green swap on every host, so the main thing consolidating into third-party-builds would buy is the thing Seedling cannot hand over. The cost, a second place to bump on a Caddy CVE, is real and is mitigated by asserting version parity with the BES build rather than by depending on it. Reasoning in Build strategy below.
- **Both adjacent defects are fixed on this card.** The double-shipped Caddy binary and the window where `main` references an unpublished tag. Both are in the two files this card already edits.
- **The capability check reads its required-module set from the Rust source**, so there is one place that says what the image must provide.
- **The tag becomes the Caddy version plus a build serial** (`2.11.4-2`), with a check that the serial cannot be forgotten when the build changes.

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

## Adjacent defects found, both fixed here

Neither is caused by this card. Both are made slightly worse by it, and both are in the files it already touches.

### The image ships two Caddy binaries

`docker/caddy/Containerfile` layers `COPY --from=builder /usr/bin/caddy` over `FROM docker.io/library/caddy:2.11.3`, whose own 48.3 MB binary stays in the lower layer and is still pulled. Measured, the published image is 41.5 MB compressed, roughly half of it a Caddy nobody runs. On the poor links the PRD describes, that is the single cheapest win available here.

The fix is a final stage that does not already contain a Caddy. Two things make that safe, and one makes it dangerous:

- **The binary is statically linked**, so the final stage does not need a matching libc. `scratch` is viable; `alpine` costs 3.84 MB and keeps a shell for the times someone needs to `podman exec` into the proxy.
- **A CA bundle has to be carried over explicitly.** ACME talks TLS to the issuer, and a `scratch` or bare-`alpine` stage has no roots.
- **`XDG_DATA_HOME=/data` is set by the official base image, and it is load-bearing.** It is what puts certmagic's storage at `/data/caddy`, which is exactly the path `cert_observation.rs` globs for `cert_valid` observations. Dropping the official base without carrying that env forward relocates every certificate to the process's home directory, and the failure is silent: Caddy still serves, certificates are still obtained, and Seedling simply stops seeing them. The PRD calls warm-cert observation the highest-leverage fix in the project, so this is the one line in the change that must not be missed.

`XDG_CONFIG_HOME=/config` should come across for the same reason, since the container mounts a tmpfs there. `/etc/caddy` should exist in the image, because the admin config is bind-mounted to `/etc/caddy/admin.json` into a read-only rootfs.

Estimated result: about 21.7 MB compressed against 41.5 MB today.

### `main` can reference an image tag that does not exist

`caddy-image.yml` triggers on push to `main`, and recent runs take 16 to 19 minutes (multi-arch, arm64 under qemu). `CADDY_IMAGE` lives under `crates/`, which is not in the workflow's `paths`, so it cannot trigger the build itself. Between merging a Containerfile change and the build finishing, a daemon built from `main` fails in `ensure_caddy_running` when `pull_image` cannot find the tag.

The fix is to publish the tag before `main` references it: build and push on pull requests that touch `docker/caddy/**`, so the image exists by the time the PR merges. Publishing an image for a PR that never merges is harmless, since the tag is the one that PR was claiming.

One wrinkle to handle rather than discover: a `pull_request` from a fork gets a read-only `GITHUB_TOKEN`, so the push would fail. The job should push only when the head repository is this repository, and build without pushing otherwise.

## Implementation shape

### The capability check

One required-module set, living in the Rust source next to the code that emits module ids, consumed by both halves:

- **A Rust test** builds a `ProxyConfig` exercising every feature `build_caddy_config` can emit, walks the emitted JSON for module ids, and asserts each one is in the required set. This derives the check from real emission rather than from a hand-kept list, so adding a handler without declaring it fails in the test suite.
- **A step in `caddy-image.yml`** runs `caddy list-modules` against the built image and asserts the required set is present, before the push. This is what catches a dropped `--with`, a plugin renaming its module id, and a base-image change that quietly moves `CADDY_VERSION`.

Composing the two gives the invariant worth having: everything the code can emit is in the image `CADDY_IMAGE` names.

The known limit is fixture coverage. A feature the fixture does not exercise contributes no module id, so the fixture has to be the everything-on config, and it is worth saying so where it lives.

The CI half needs the required set without building the workspace, which the image workflow does not otherwise do. Keeping the set in a small declarative form that Rust reads (rather than scraping Rust source text from a shell script) keeps one source of truth without putting a `cargo build` in the image workflow.

### The tag serial

`2.11.4-2`: Caddy version, then a serial bumped whenever the build changes without the Caddy version changing. The Containerfile stays the record of what is in the build.

The serial is forgettable by construction, so a pull-request check should assert that a diff touching `docker/caddy/Containerfile` also moves `TAG`. Same job as the fork-aware build above, and it fails on the PR rather than after merge.

## Open questions

None blocking. All four decisions above are settled; what remains is drafting.

## Trade-offs

- **Deferring the WAF costs a second image bump later.** Accepted. Measured, the BES plugin set costs 11.7 MB of binary, dominated by coraza carrying the OWASP CRS, for a capability enabled nowhere and which may end up enforced outside Caddy entirely. Tracked on S1.
- **Keeping the build in Seedling costs a second place to bump on a Caddy CVE.** Accepted, and worth naming rather than arguing away: it is the real cost of the split. Version parity with the BES build, asserted in the capability check, is what keeps the two from drifting silently.
- **Version parity is a check, not a dependency.** One Caddy version on a migrating host, two binaries with different plugin sets. Gets the migration-skew benefit of consolidation without ceding a version Seedling has to control.
- **A build serial is forgettable where a plugin-naming tag is not.** Accepted because the tag stops scaling once there is more than one plugin, and the forgetting is mechanically checkable where the unwieldiness is not fixable.
- **Fixing both adjacent defects here widens the card.** Accepted: both live in the two files this card already edits, and the missing-tag window gets worse as the build grows.

## Testing notes

- Every module id the emitted config can carry is declared in the required set, asserted from a fixture that exercises every feature `build_caddy_config` supports.
- The image `CADDY_IMAGE` names registers every module in the required set, asserted before publication rather than after.
- The built image reports the expected Caddy version, so a base-image change that moves `CADDY_VERSION` is caught rather than shipped.
- The image's Caddy version matches the BES build's, so the two drift only deliberately.
- An emitted config naming the rate-limit module is accepted by the pinned image.
- The image contains exactly one Caddy binary.
- Certificates land where `cert_observation.rs` looks for them after the base-image change, so `cert_valid` observations survive it. This is the regression the base change could cause silently, and it deserves a test that fails loudly rather than a manual check.
- A pull request touching `docker/caddy/Containerfile` without moving `TAG` fails.
- `main` never references a `seedling-caddy` tag that has not been published.
- Bumping `CADDY_IMAGE` drives a blue/green upgrade rather than a restart, and the cached config replays without a migration.
