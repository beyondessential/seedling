# Seedling Threat Model

This document describes what seedling tries to defend against, what it does
not, and the mechanisms it currently uses. It is descriptive, not normative:
the authoritative requirements live under `docs/spec/`. Where a mitigation is
mentioned, the relevant spec rule is cross-referenced.

## Audience

- Operators evaluating whether seedling fits their deployment model.
- Reviewers assessing whether a change preserves the boundaries described
  here.
- Future contributors deciding which class of threat a new feature belongs
  to.

## Trust model

Seedling sits between three principals on a single Linux host:

1. **The host.** The Linux kernel, systemd, podman, and any other daemon root
   could compromise. Seedling trusts the host completely; if it is
   compromised, seedling's protections do not apply.
2. **The seedling daemon (`seedling`).** Runs as root. Owns the OI listener,
   the database, the secret-key file, and is the only process that issues
   `podman` commands. Trusted by operators to execute their declared
   intent and nothing more.
3. **Operators.** Humans (or service accounts) that authenticate to the OI.
   Their authority over seedling is total: see "What we do not defend
   against" below.

Below these, two further principals exist but are not trusted:

4. **App definition authors.** Whoever wrote the BSL script (`*.seed.rhai`)
   the operator registered. May be the operator themselves, a vendor, a
   contractor, or a peer. The script is sandboxed by the rhai engine and
   only sees the builder methods seedling exposes — see
   `docs/spec/runtime.md#r--engine.limits`.
5. **Workloads.** Containers that seedling starts on the host. Always
   untrusted, even when the script was written by the operator.

## What we defend against

The following classes of threat are in scope. Seedling's mitigations target
them, and a regression here is a security bug.

### T1. Unauthenticated access to the OI

Anyone who can reach the OI listener on the network must not be able to
issue commands without authenticating. Both the daemon's QUIC listener and
the web frontend's HTTP listener enforce authentication on every request.

### T2. Eavesdropping or tampering on OI transports

A network attacker between an operator and the daemon must not be able to
read or modify OI traffic.

### T3. Workloads escaping containment via a seedling bug

Workloads run as untrusted code. They must not be able to acquire
capabilities, mounts, host network access, or device access that the BSL
script did not explicitly request from seedling. Examples in scope:

- A workload coercing seedling into adding a host bind mount that the BSL
  did not declare.
- A workload tricking seedling into running a follow-up container with
  attacker-controlled args because of how a parameter is interpolated.
- A workload exfiltrating another app's secrets through a seedling-mediated
  pathway (action params, environment, logs the daemon serves).

### T4. Cross-workload influence not mediated by an operator

Two workloads on the same host that share no declared volume, service, or
external mapping must not be able to influence each other through seedling.
The operator can deliberately wire two apps together; that is consent.
Without that consent, seedling must not provide a back-channel.

### T5. Untrusted app definitions affecting the host

A BSL script must not be able to escape its sandbox to read or write the
host filesystem, fork host processes, or open arbitrary sockets. Its only
side effects are the builder calls seedling exposes.

This boundary is partial: a script *can* request a container with a host
bind mount (`app.external_volume(...)`) or with privileged capabilities
(`container.cap_add(...)`). Those requests appear on the operator's
generation diff and must be explicitly applied by an operator who trusts
the script. The boundary is between "the script alone can do harm" (out of
scope) and "the script can do harm without the operator seeing it" (in
scope).

### T6. Disclosure of secret parameters at rest or through the OI

Parameter values marked secret must be encrypted at rest, must be omitted
from operator-facing describe responses, and must remain encrypted in the
generation history. The runtime decrypts them only to pass them to the
container the script targets — see `docs/spec/runtime.md#r--secret.storage`,
`#r--secret.history`, and `#r--secret.redaction`.

### T7. Accidental destruction of operator data

Not strictly a security threat in the confidentiality/integrity sense, but
treated as one: deleting a managed volume routes through the held-volume
mechanism so a fat-fingered command does not vapourise data —
`docs/spec/runtime.md#r--actuate.volume.hold`. The same applies to
generation history retention: rolling back to an older generation is a
first-class operation, not a forensic exercise.

### T8. Privileged-action discoverability

Every OI request must be attributable to an actor and recorded in the
audit log so a post-incident review can answer "who did this and when".
See `docs/spec/runtime.md#r--audit.log` and
`docs/spec/interface.md#i--wire.actor`.

### T9. Disclosure of TLS key material

The runtime stores three classes of TLS key material on behalf of the
operator: ACME account keys, DNS-provider credentials, and the private
keys for runtime-managed certificates (manual uploads, CSR-derived
certs, and certs the runtime obtained itself via ACME). All three must
be encrypted at rest, omitted from any operator-facing list/describe
response, and reachable to the proxy only through the cert-serving
endpoint defined in
`docs/spec/runtime.md#r--tls.cert.serve`. In particular:

- Private keys must never appear in the proxy's persistent
  configuration or its restart-replay cache.
- DNS-provider credentials must not be passed to the proxy as
  environment variables, file mounts, or anywhere the proxy could log
  them; the runtime drives ACME-DNS itself precisely so credentials
  never leave the daemon.
- The cert-serving endpoint must restrict access by binding to a
  non-routable bridge address and requiring a per-installation token.

## What we do not defend against

These threats are explicitly out of scope. A request to "harden against X"
where X is one of the following is a request to expand the threat model,
not a bug.

### N1. An authenticated operator compromising the host

Authenticated OI access is equivalent to root on the host. An operator
can:

- Create a bind site volume pointing at any absolute host path
  (`/etc`, `/root`, `/`) and open a volume shell over it.
- Register an app whose BSL declares a container with `cap_add("SYS_ADMIN")`
  and a bind mount of `/`, then install it.
- Open a normal app shell into any installed container and run arbitrary
  commands as the workload's user.
- Use `seedling-ctl` to push a script with a malicious install action.

Operator authorisation is the trust boundary. Seedling provides
*audit-after-the-fact* (every action attributable to an actor in the audit
log) but not *prevention*. There is no privilege separation among
operators; an operator who has authenticated has full access.

The web UI's safety modes (read / write / dangerous, with elevation
timeouts) are an *ergonomic* tool to reduce accidents — not a security
boundary. A determined operator can switch modes at will.

### N2. Container escape via a kernel, podman, or runc bug

If a workload exploits a CVE in the underlying container runtime to escape
into the host, seedling cannot detect or prevent it. Mitigations like
`--cap-drop=ALL` and `--security-opt no-new-privileges` raise the bar but
do not eliminate the kernel attack surface. Patch the host.

### N3. Compromise of the host kernel, systemd, or other daemons

Seedling trusts the host. A rootkit, a malicious systemd unit installed
out-of-band, or a compromised package manager can subvert seedling at
will, including by reading the secret-key file.

### N4. Supply-chain compromise of container images

The registry allowlist (`docs/spec/language.md#l--container.image.registry-allowlist`)
restricts *which registries* a script may pull from, not *what* those
registries serve. Seedling does not verify image signatures, does not pin
content digests by default beyond what the script asks for, and does not
sandbox the contents of a pulled image beyond the standard container
hardening. Compromise at the registry, the image-publishing toolchain, or
the upstream base image is out of scope.

### N5. Physical access

Disk encryption and hardware security are the operator's responsibility.
Seedling's secret-key file lives in the data directory with the database
and is encrypted only by the host filesystem's permissions
(`docs/spec/runtime.md#r--secret.key`). Anyone with raw read access to the
disk can read every secret parameter ever stored.

### N6. Side channels

Spectre, Meltdown, RowHammer, timing attacks against TLS, and similar
hardware/microarchitectural attacks are out of scope. Seedling does not
attempt to make workloads constant-time or to enforce cache partitioning.

### N7. Denial of service against the OI by an authorised operator

The OI has a global stream-concurrency limit
(`docs/spec/interface.md#i--stream.concurrency-limit`) to protect against
*accidental* overload, but an authorised operator who deliberately tries to
DoS the daemon will succeed. There is no per-key rate limiting today.

### N8. Network security of forwarded service ports

Port forwards (`/forwards/start`) tunnel TCP/UDP from the operator to a
named service inside the workload network. Seedling forwards bytes. It is
the workload's responsibility to authenticate or encrypt that traffic if
it needs to.

### N9b. ACME-CA and DNS-provider availability or honesty

Once the operator binds a hostname to a public CA (Let's Encrypt by
default) or a DNS provider (Route 53), seedling depends on those third
parties for issuance and renewal. If the CA mis-issues a certificate
for a hostname operators control, or if the DNS provider serves a TXT
record for an attacker, seedling has no independent path to detect
that. This is a property of the public ACME / DNS ecosystem, not of
seedling. Mitigations: prefer DNS-01 (which requires control of the
zone) over HTTP-01 where possible, monitor CT logs out-of-band, and
rotate DNS-provider credentials promptly if compromise is suspected.

### N9c. Outbound network reach from the daemon

The daemon makes outbound HTTPS requests to the configured ACME
directory and to the DNS-provider API endpoints (Route 53 today). A
firewall blocking those endpoints will stall ACME-DNS issuance and
renewal; the runtime files faults but cannot work around the lack of
egress. Operators are responsible for permitting the necessary egress
or supplying an alternative directory via
[`tls.cert.issue-acme-dns`](docs/spec/interface.md#i--tls.cert.issue-acme-dns).

### N9. Backup integrity once exfiltrated

A registered backup app is responsible for its own at-rest encryption,
remote-store authentication, and integrity validation. Seedling delivers
the volume bytes; the backup app decides what to do with them. The kopia
example app uses Kopia's built-in encryption and authenticated metadata,
but that is a property of the app, not of seedling.

### N10. A rogue BSL author running code on the host directly

The script engine sandbox is only meaningful while it holds. If the
operator runs `bash` on the script file before registering it, all bets
are off. The threat is "the registered BSL alone can compromise the host";
it is not "the file the operator received from a third party is safe to
open".

## Mitigations in place

### Authentication and transport

- **mTLS with raw public keys (RFC 7250).** Every OI client connection
  presents a SPKI; the daemon's authorised-key table gates connections at
  the TLS layer. `docs/spec/interface.md#i--transport.client-auth`.
- **Bootstrap via `authorized_keys` file.** New keys can be added by an
  operator with write access to `$data_dir/authorized_keys` without
  needing a prior authenticated connection.
- **TLS fingerprint probe.** The CLI presents a single-use ephemeral key
  on first contact to capture the server's SPKI fingerprint, which the
  user confirms before any real authentication
  (`docs/spec/interface.md#i--transport.fingerprint-probe`). This avoids
  TOFU on the cert chain.
- **Web auth.** Argon2id-hashed password, optional Tailscale identity
  headers (only when the operator has explicitly enabled trust), and an
  explicit dev bypass that is rejected at startup if the bind address is
  not loopback (`docs/spec/web.md#w--auth.password`,
  `#w--auth.tailscale`, `#w--auth.dev`).
- **WebTransport handshake tokens.** Short-lived single-use tokens bridge
  `POST /connect` and the WebTransport handshake; they cannot be replayed
  (`docs/spec/web.md#w--wt.token`).
- **Pinned WebTransport certs with rotation.** 14-day max validity,
  rotation with a 24-hour overlap window, fingerprints surfaced in
  `/connect` responses for `serverCertificateHashes`
  (`docs/spec/web.md#w--wt.cert`, `#w--wt.cert.rotation`).

### Container hardening

Every container seedling launches gets a hardened argv by default — see
`crates/core/src/system/translate/container.rs` and the matching tests:

- `--cap-drop=ALL` then opt-in `--cap-add` per BSL declaration
  (`docs/spec/language.md#l--container.cap-add`).
- `--security-opt no-new-privileges`.
- `--read-only` rootfs by default; opt-in writable rootfs via BSL.
- `--pids-limit` (default 1024, configurable) and `nofile=65536:65536`.
- `--cap-add=NET_BIND_SERVICE` is granted only to the network-edge
  containers seedling itself runs (Caddy, the resolver), not to workloads.
- `--log-driver=none` so container output flows through systemd once,
  preventing the journald double-write pathway from being exploited as a
  log-injection sink.

### BSL sandbox

The rhai engine is configured with `engine.limits` (and the
`engine.limits.*` sub-rules) to bound expression cost, recursion depth,
operations per evaluation, and string/array/map sizes. There is
no host filesystem, network, or process API exposed to BSL — only the
builders enumerated in `docs/spec/language.md`.

### Image provenance gates

- **Registry allowlist** at install time. Default `docker.io` and
  `ghcr.io`; operator-configurable. A registered app whose script
  references an image outside the allowlist receives a
  `disallowed_registry` fault and is not installed
  (`docs/spec/language.md#l--container.image.registry-allowlist`).
- **Image pin tracking** records which apps depend on which images so the
  garbage collector does not delete an image still in use, and so a
  per-app probe can surface the digests actually in flight
  (`docs/spec/runtime.md#r--image.pin`).

### Network isolation

- Each pod runs on its own podman network with a deterministic
  `seedling-{display}` name and a per-pod IPv6 prefix derived from the
  pod's identity
  (`docs/spec/runtime.md#r--infra.pod.network`). Cross-pod traffic
  goes through declared services or external service mappings, never via
  shared bridges.
- A central NAT64 + DNS resolver presents IPv4 hosts to IPv6-only pods,
  with curated A/AAAA records under the seedling-managed zones; pods do
  not see the host's `/etc/resolv.conf`
  (`docs/spec/runtime.md#r--infra.resolver`,
  `docs/spec/runtime.md#r--infra.nat64.mode`).

### Volume safety

- **Held volumes** for accidental deletion of managed site volumes and
  app volumes whose name was removed from the script
  (`docs/spec/runtime.md#r--actuate.volume.hold`).
- **Read-only volume shells** when the web UI is in read mode, so
  inspection cannot mutate
  (`docs/spec/web.md#w--volumes.shell-ui.read-only`).
- **Snapshot site volumes are inherently RO at the filesystem level**
  (BTRFS) and the runtime checks that property when serving them
  (`docs/spec/runtime.md#r--volume.site.snapshot`).
- **Bind site volumes require an absolute host path** — there is no
  string interpolation that could resolve to one
  (`docs/spec/runtime.md#r--volume.site.lifecycle`). The operator-as-root
  assumption (N1) accepts that they can still point this anywhere.

### Secret parameter handling

- **Encryption at rest** under a runtime-managed key file with the same
  permissions as the database
  (`docs/spec/runtime.md#r--secret.key`, `#r--secret.storage`).
- **Redaction in describe and history responses**
  (`docs/spec/runtime.md#r--secret.redaction`).
- **Automatic migration** when a parameter transitions from non-secret to
  secret, so the historical plaintext does not linger
  (`docs/spec/runtime.md#r--secret.migration`).

### TLS certificate handling

The runtime owns the full lifecycle of operator-managed certificates
to keep key material out of the proxy's reach until the moment of a
TLS handshake.

- **Encryption at rest** for ACME account keys, DNS-provider
  credentials, manual private keys, and CSR-flow private keys, all
  under the same secret-key file used for secret parameters
  (`docs/spec/runtime.md#r--tls.acme.account.persist`,
  `#r--tls.dns-provider.lifecycle`, `#r--tls.strategy.manual`,
  `#r--tls.csr.flow`).
- **Daemon-driven ACME-DNS** so DNS-provider credentials never
  reach the proxy or its persistent configuration. The runtime runs
  the ACME client itself, publishes the DNS-01 TXT record via the
  `DnsProvider` trait, and only then hands the resulting certificate
  to the proxy via the cert-serving endpoint
  (`docs/spec/runtime.md#r--tls.strategy.acme-dns`).
- **No private keys in the proxy config** — the rendered Caddy JSON
  references the cert-serving endpoint URL and never contains cert
  bytes or keys, so the blue/green replay cache is safe to keep
  in plaintext (`docs/spec/runtime.md#r--tls.cert.serve`).
- **Cert-serving endpoint scope.** The endpoint binds to the
  seedling-proxy bridge gateway address (a ULA in `fd00::/8` reachable
  only from Caddy and host processes) and requires a per-installation
  token in the URL path. The token is stored as a 0600-permission file
  alongside the database secret key. Defence-in-depth against a
  non-root host process attempting to enumerate hostnames and harvest
  private keys.
- **Redaction in the OI.** The `tls.cert.list`, `tls.policy.list`, and
  `tls.dns-provider.list` responses describe certificates and
  providers without exposing private key material, account keys, or
  provider credentials (`docs/spec/interface.md#i--tls.cert.list`,
  `#i--tls.policy.list`, `#i--tls.dns-provider.list`). The CSR-flow
  contract additionally requires that a server-generated private key
  never leaves the host
  (`docs/spec/runtime.md#r--tls.csr.flow`).
- **Autonomous renewal** of ACME-DNS certificates by a background
  task, before expiry, so a stalled operator does not turn into a
  served-cert outage. Manual and CSR-derived certs are flagged via
  the `cert_expiring_soon` fault (`docs/spec/runtime.md#r--tls.fault.expiring`).

### Audit and observability

- **Audit log** records every OI request, the actor, and the resulting
  effect, separately from the autonomous operations log
  (`docs/spec/runtime.md#r--audit.log`).
- **Wire-level actor binding** ensures the audit log can resolve the
  human (or system account) behind every action
  (`docs/spec/interface.md#i--wire.actor`).
- **Generation history** retains both old and new values for every param
  change, enabling rollback and forensic diffing — secret values stay
  encrypted (`docs/spec/runtime.md#r--secret.history`).
- **Fault surface** turns persistent error conditions
  (`disallowed_registry`, `script_error`, `health_check_failed`,
  `operation_failed`, `operation_cancelled`, etc.) into operator-visible
  state, not just log noise (`docs/spec/runtime.md#r--fault.surfacing`).

### UI ergonomics that reduce blast radius

These are not security boundaries but they reduce the chance of an
operator confusing themselves into a destructive action:

- **Three-tier safety mode** in the web UI (read / write / dangerous) with
  a 10-minute elevation timeout. Read mode disables every write-tier
  control by default; volume shells are still available but auto-RO.
- **Confirmation dialogs** for destructive volume actions, distinguishing
  "moves to held" from "permanently deletes"
  (`docs/spec/web.md#w--routes.volumes.delete-confirm`).
- **Plan-then-apply** for parameter changes
  (`docs/spec/interface.md#i--plan.dry-run`) so an operator can preview
  what an update will trigger before committing.

## Reviewing changes against this model

When reviewing a feature, walk it against the boundaries:

1. Does it expose a new way for a workload to influence seedling's
   behaviour beyond what the BSL declared? (T3)
2. Does it bridge two apps that are not deliberately wired together by
   the operator? (T4)
3. Does it persist new operator-facing data that should be redacted in
   the OI surface? (T6, T9)
4. Does it short-circuit the audit log? (T8)
5. Does it create a destructive action without a confirmation path or a
   recoverable hold? (T7)
6. Does it move TLS key material into a place the proxy or its config
   cache can read? (T9)

If a change relaxes a boundary on purpose (e.g. exposing a new capability
to BSL), call it out and update this document so the new posture is the
recorded one.
