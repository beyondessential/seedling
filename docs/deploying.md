# Deploying Seedling

Seedling ships as three binaries — the `seedling` daemon, the `seedling-web`
interface, and the `seedling-ctl` operator CLI. On Debian/Ubuntu the supported
path is the `seedling` package from the BES APT repository; the same binaries
are also published as standalone tarballs for other Linux hosts, and
`seedling-ctl` and `seedling-web` are built for macOS and Windows too.

The daemon is Linux-only: it loads the jool kernel module for NAT64, programs
nftables, drives systemd, and controls podman. `seedling-ctl` and `seedling-web`
have no such coupling and run anywhere.

## Install from the APT repository

Configure the repository once:

```bash
curl -fsSL https://tools.ops.tamanu.io/apt/bes-tools.gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/bes-tools.gpg
echo "deb [signed-by=/etc/apt/keyrings/bes-tools.gpg] https://tools.ops.tamanu.io/apt stable main" | sudo tee /etc/apt/sources.list.d/bes-tools.list
sudo tee /etc/apt/preferences.d/bes-tools <<EOF
Package: *
Pin: origin tools.ops.tamanu.io
Pin-Priority: 999
EOF
sudo apt-get update
sudo apt-get install seedling
```

The package depends on `podman` (5.x — podman 4 is too old and 6 is not yet
supported), `nftables` and `libsystemd0`, and recommends
`btrfs-progs` (for named-volume snapshots) and `jool-dkms` + `jool-tools` (for
NAT64). Recommends are installed by default; if you run without NAT64 or on a
non-btrfs data directory you can skip them with `--no-install-recommends`. It
also suggests `tailscale`, used to front the web interface (see below); install
it separately if you want that.

### What the package installs

- Binaries: `/usr/bin/seedling`, `/usr/bin/seedling-web`, `/usr/bin/seedling-ctl`.
- Units: `seedling.service` and `seedling-web.service` (both enabled and started
  on install), plus `seedling-web-tailscale-serve.service` (shipped **disabled**;
  enable it to expose the web interface over Tailscale).
- State: `/var/lib/seedling` (data directory: database, keys, authorized keys)
  and `/var/log/seedling` (audit log, rotated by `/etc/logrotate.d/seedling`).
- Config: `/etc/seedling/web.toml` (a conffile — your edits survive upgrades).

On first install the daemon starts immediately with sensible defaults
(`--data-dir /var/lib/seedling`). Workloads keep running while the daemon is
restarted, so upgrades (`apt-get upgrade`) are non-disruptive.

## Operator access (seedling-ctl)

`seedling-ctl` authenticates to the daemon with its own key. Authorise your
operator key once:

```bash
# Print your client key fingerprint (generates the key on first run).
seedling-ctl client fingerprint

# Authorise it with the daemon (needs write access to the data directory).
echo "<your-fingerprint> your-name" | sudo tee -a /var/lib/seedling/authorized_keys
sudo systemctl restart seedling.service   # the daemon imports new entries on start
```

On first connection `seedling-ctl` captures and asks you to confirm the daemon's
fingerprint (trust-on-first-use). You can read it ahead of time from
`/var/lib/seedling/oi.fingerprint`.

## The web interface (seedling-web)

`seedling-web.service` is **enabled and started on install**. Its daemon
credentials are already bootstrapped — the package pre-generated
`/var/lib/seedling/web.key`, authorised that key in
`/var/lib/seedling/authorized_keys`, and the unit pins the daemon from
`/var/lib/seedling/oi.fingerprint`. Its HTTP listener binds loopback only and
authenticates operators via Tailscale identity headers
(`--trust-tailscale-headers`), so out of the box it runs but the HTTP interface
is not reachable until you put a front-end in front of it. Its WebTransport
listener is bound separately on the tailnet (`--wt-interface lo,tailscale0`) —
see below.

### Tailscale (default)

Expose the loopback interface over your tailnet with `tailscale serve`, which
terminates HTTPS (the secure context WebTransport needs) and injects the
`Tailscale-User-*` identity headers seedling-web trusts. A unit for this ships
**disabled**; enable it once Tailscale is up on the host:

```bash
sudo systemctl enable --now seedling-web-tailscale-serve.service
```

That runs `tailscale serve --https=7895 --bg localhost:7894`, so the interface
is reachable at `https://<node>.<tailnet>.ts.net:7895/`. It uses **7895** (in
Seedling's 789x range), not 443 — 443 is reserved for app workloads (Caddy). If
Tailscale is not installed the unit is a no-op (a
`ConditionPathExists=/usr/bin/tailscale` gate skips it rather than failing), so
it is safe to leave enabled. To stop exposing the interface,
`systemctl disable --now seedling-web-tailscale-serve.service` (which runs
`tailscale serve --https=7895 off`).

Only loopback and the local `tailscale serve` reach the HTTP port, so the trusted
identity headers cannot be spoofed by tailnet peers.

WebTransport (used for the live session) is HTTP/3 over QUIC and **cannot** be
carried by `tailscale serve` (a TCP HTTP proxy), so the browser connects to it
directly at `<node>.<tailnet>.ts.net:7893`. The unit binds it there with
`--wt-interface lo,tailscale0`; it is gated by a per-session token and pins the
server certificate, so exposing it on the tailnet is safe without header trust.
`tailscale0` is only bound when tailscaled is already up, so **restart
`seedling-web` after bringing Tailscale online** (`sudo systemctl restart
seedling-web.service`) — otherwise the browser reaches the login page but the
session can't connect and falls back to the password prompt.

### Password login (alternative)

To use a password instead of Tailscale, set an Argon2id hash in
`/etc/seedling/web.toml` (generate one with the `argon2` CLI from the `argon2`
package):

```bash
printf '%s' 'your-password' | argon2 "$(head -c16 /dev/urandom | base64)" -id -e
```

Paste the resulting `$argon2id$...` string as `password_hash` under `[auth]`,
drop `--trust-tailscale-headers` with a drop-in, bind a reachable interface, and
front the HTTP port with a TLS-terminating reverse proxy (WebTransport requires a
secure context). Note the WebTransport port cannot go through that proxy either,
so bind it on the reachable interface too (`--wt-interface eth0`, or
`--wt-listen`); the browser connects to it directly on `:7893`:

```bash
sudo systemctl edit seedling-web.service
# [Service]
# ExecStart=
# ExecStart=/usr/bin/seedling-web --config /etc/seedling/web.toml \
#     --daemon-fingerprint-file /var/lib/seedling/oi.fingerprint \
#     --key-file /var/lib/seedling/web.key \
#     --interface eth0 --wt-interface eth0
```

## Standalone binaries (no APT)

Every release also publishes each binary as a `.tar.zst` under
`https://tools.ops.tamanu.io/`:

- `seedling`, `seedling-ctl`, `seedling-web` for Linux (x64 and arm64).
- `seedling-ctl` and `seedling-web` for macOS (Intel and Apple Silicon) and
  Windows (x64).

```
https://tools.ops.tamanu.io/<name>/<version>/<name>-<target>-<version>.tar.zst
https://tools.ops.tamanu.io/<name>/latest/<name>-<target>-<version>.tar.zst
```

where `<target>` is a Rust target triple (e.g. `x86_64-unknown-linux-gnu`,
`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`). Extract with
`zstd -d < file.tar.zst | tar -x`. These carry build-provenance attestations,
verifiable with `gh attestation verify <file> -R beyondessential/seedling`.

Running the daemon from a tarball means creating the state directories, the
systemd unit, and the log rotation yourself; the package is the supported way to
run the daemon.
