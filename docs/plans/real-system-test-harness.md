# Real-system integration test harness

This is a requirements sketch for the tier of tests that exercise real
podman / systemd / btrfs / nftables / Jool / Caddy / resolver, as opposed
to the mock-based reconciler and OI-harness tiers already planned
(families 1–3). Implementation is deferred; this document fixes what the
harness must do so the shape is clear when we do pick it up.

## Why this tier needs to exist

The mock-based tiers prove *logic*: that the reconciler issues the right
lifecycle calls for a given world, that the OI handler returns the right
JSON for a given request, etc. They do not prove that the code which
actually drives the underlying subsystem is correct — i.e. that
`PodmanRuntime` speaks to podman correctly, that transient-unit specs
are accepted by real systemd, that the nftables ruleset the code emits
is syntactically valid and routes packets as intended, that the Caddy
config triggers real certificate acquisition, etc.

A non-trivial number of spec rules exist precisely to describe that
real-subsystem interaction, and they cannot be meaningfully verified
without actually talking to the subsystem.

## Rules this tier is responsible for

Non-exhaustive list, grouped by subsystem. Each item is a spec rule that
the mock tiers cannot verify on their own.

- **podman / container runtime**
  - `actuate.container.hardening`
  - `actuate.container.journal-metadata`
  - `actuate.deployment.start / stop`
  - `actuate.deployment.anon-volume.start`
  - `container.image.registry-allowlist` (end-to-end, including pull)
  - `fault.container-start` (real container repeatedly failing)
- **systemd**
  - `actuate.infra.journal-metadata`
  - `infra.proxy.startup / upgrade / upgrade.cache`
  - `infra.resolver.startup / upgrade / config / address`
  - `infra.pod.network / mount`
- **btrfs / volume store**
  - `actuate.volume.btrfs`
  - `actuate.volume.start / stop`
  - `actuate.volume.hold / hold.confirm / hold.events`
  - `actuate.volume.storage / tmpfs`
  - `startup.btrfs`
  - `volume.site / volume.site.lifecycle{,.events}`
  - `volume.site.snapshot{,.events}`
  - `volume.site.promote{,.events}`
  - `volume.external.mapping.events`
- **nftables / dataplane**
  - `infra.dataplane.mount-dnat`
  - `infra.dataplane.service-dnat`
  - `autonomous.network` (repair of dropped rules)
- **NAT64 / Jool**
  - `infra.nat64.detection`
  - `infra.nat64.dns64`
  - `infra.nat64.forwarding`
  - `infra.nat64.ipv6-egress`
  - `infra.nat64.mode`
  - `infra.nat64.translator{,.lifecycle}`
- **Caddy / ingress**
  - `fault.cert-acquisition` (with a controllable ACME endpoint, e.g. Pebble)
  - `autonomous.ingress` (rule drift recovery against a real Caddy)
- **Observation**
  - `observe.deployment / volume / ingress / facts / persist`
  - `observe.volume.backend-mismatch`
- **Reconciliation convergence**
  - `reconciliation.convergence / liveness / loop` (as emergent properties
    of the full stack, not the reconciler logic in isolation)
- **Update strategies**
  - `update.rolling / .over-provision / .reboot-resume / .restart-resume`
  - `update.replace`
- **Shell sessions (end-to-end)**
  - `operation.shell / operation.shell.resources`

Rough count: 40–50 rules are in this set. About a quarter of the total
requirements surface.

## What a single test needs to be able to do

The harness is whatever shape lets a single test express the following,
repeatably and in isolation:

1. **Bring up a clean world.** A pristine OS-level state — no leftover
   containers, no stale nftables rules, no surviving unit files — even
   if the previous test crashed partway through.
2. **Install a seedling daemon build** at a known version into that
   world, configured to point at real subsystems (podman socket,
   systemd, btrfs mount, Jool module if applicable).
3. **Provide controllable dependencies.**
   - A container registry the test can push synthetic images to (local
     registry + tiny test images, not pulls from the internet).
   - An ACME endpoint (Pebble) for certificate tests.
   - A pair of interfaces / namespaces for NAT64 tests where one side
     is IPv6-only.
4. **Drive the daemon** — register apps, set params, invoke actions,
   inspect state — via the OI, exactly as operators do. The test must
   not reach around the daemon to manipulate the world directly
   (because that would test something other than what the daemon does).
5. **Observe real effects.** Query podman, systemd, nft, btrfs
   directly to assert that the world reached the state the spec
   requires, not merely that the daemon thinks it did.
6. **Inspect logs / faults / history.** Open the daemon's journal,
   read the audit log, read the faults table — the same surfaces an
   operator would see — and assert on them.
7. **Tear down completely.** Remove containers, unmount btrfs, flush
   nftables, rm the data dir. A test's failure must not affect the
   next test's world.

## Properties the harness must have

### Isolation

- Each test case must run against a world it owns. Parallelism within
  the harness is nice to have, not required; if tests must serialise,
  that's acceptable.
- A crashed or killed test must not leak state that a subsequent test
  could observe. Practically this means VM-level or network-namespace-
  level isolation, not "`rm -rf` at the end of the test body".
- Tests must not observe or depend on the host running them. No
  `/etc/machine-id`, no host resolver, no host clock-sync quirks bleeding
  through.

### Reproducibility

- The kernel, distro, systemd version, podman version, btrfs-progs,
  nftables, Jool, Caddy must be pinned. A test's pass/fail must not
  silently depend on the host's package mix.
- Test image payloads (the container images the test pushes / the
  daemon pulls) must be built from source as part of the harness or
  committed as fixtures. No pulls from a public registry at test time.
- Timestamps, UUIDs, and other sources of non-determinism must either
  be fixable or be explicitly tolerated by the assertions.

### Local runnability

- A developer must be able to run a single test, or the whole suite,
  on their own machine. Whatever sandbox the harness uses (VM,
  network-namespace, container-in-container) must be installable via a
  documented, reproducible sequence — not "ask the maintainer to
  onboard you".
- The dev loop must be usable on Linux. macOS support is a bonus but
  not a requirement; it is acceptable for Mac users to run the suite
  against a remote Linux host or VM.
- Iteration time for a single test should be seconds, not minutes,
  after the first run. Image rebuilds and VM provisioning should be
  cached aggressively.

### CI runnability (GitHub Actions)

- Must run unattended on `ubuntu-latest` (or whatever the closest
  supported variant is that actually has KVM, btrfs-progs, etc.). No
  self-hosted runners for the default suite.
- Must not require GitHub secrets. A real-system test that needs an
  ACME endpoint spins up Pebble; it does not call Let's Encrypt.
- Must fail the job hard when something leaks or a test flakes. No
  swallowing of non-zero exits, no "retry until it passes".
- Must produce artifacts on failure: daemon journal, audit log,
  container logs, nft ruleset snapshot, btrfs listing. The PR reviewer
  should not need to reproduce locally to see what went wrong.
- Runtime budget: the full real-system suite may run nightly or on
  release PRs rather than on every push. A faster "smoke" subset
  (maybe 5–10 tests covering the happy paths of the biggest rules)
  should run on every PR.

### Host-impact-free

- A test run must not leave behind containers, network interfaces,
  nftables rules, mounts, systemd units, or any other trace observable
  from the host outside whatever sandbox the harness uses. If the
  harness is VM-based this is free; if it is namespace-based the
  harness must explicitly clean up.
- The test suite must coexist with other things the developer is doing
  on the same machine. A developer running `podman ps` outside the
  harness must not see test containers; iptables rules the developer
  set must not be wiped.

### Observable from the outside

- The harness should expose a structured log of what it's doing so a
  reviewer reading CI output can tell which test phase was in progress
  when something failed.
- Daemon logs, subsystem logs (journalctl, nft list ruleset, podman
  ps / inspect, btrfs subvolume list) should be retrievable for any
  failing test, automatically in CI and on demand locally.

## Non-requirements

To keep scope honest, some things the harness explicitly does **not**
need to do:

- **Cluster / multi-node tests.** Seedling is single-node; the harness
  can assume one daemon, one host.
- **Network-quality simulation.** We are not trying to prove the
  system is correct under 30% packet loss; we are trying to prove it
  is correct when the subsystems behave normally.
- **Long-running soak / performance tests.** Those are a different
  kind of test with different requirements (stable baselines, trend
  tracking). Not in scope here.
- **Upgrade / migration tests across seedling versions.** Useful, but
  a separate axis; not part of the first cut.
- **Arbitrary platforms.** Linux x86_64 only. If we add aarch64 later
  we'll reconsider.

## Adjacent questions to resolve when we return to this

These are flagged so they do not come as surprises when implementation
starts:

- **Rootless vs root.** Some subsystems (btrfs, nftables, Jool) need
  root or specific caps. Rootless podman is a partial option. The
  harness likely ends up needing a root-owned sandbox, which pushes
  toward VM-based isolation rather than namespace-based.
  - Answer here: we 100% need to test with rootful podman, as this is
    what we're targetting in production.
- **Kernel features on GHA.** `ubuntu-latest` has KVM-with-nested-virt
  available but not always enabled; some runners have btrfs-progs, some
  don't; Jool is not in-tree. We will need to either install missing
  pieces per-run (cache aggressively) or commit to a pre-built VM image.
- **Test-image provenance.** The container images the daemon pulls
  during tests need to come from somewhere deterministic. Either ship a
  local registry seeded from fixtures in the repo, or bind-mount a
  pre-populated podman storage. The first is cleaner; the second is
  faster.
- **Interaction with the spec process.** Rules that currently say
  "the runtime must X" without saying "observable by Y" may need to
  have a verification surface added to the spec — otherwise a test
  can only assert on implementation details. This is the same tension
  we hit when writing unit tests; it's sharper here because the cost
  of "cheat and peek at internals" is higher when the whole point is
  realism.
- **Overlap with mock tiers.** A rule like `actuate.deployment.start`
  may have both a mock-level test (reconciler issued the start call)
  and a real-system test (the container actually came up). Both are
  valuable; tracey should be able to accept that both annotations
  exist. Confirm that's how the tool counts coverage before relying on
  it.

## Decision log

- *2026-04-23*: Sketched requirements above; deferred implementation
  until after families 1–3 land. Rationale: families 1–3 unlock most
  of the coverage numbers cheaply; family 4 unlocks the
  highest-confidence coverage but is an order of magnitude more
  infrastructure to build.
