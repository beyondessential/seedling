# Debian packaging + APT integration

Package seedling as a `.deb`, modelled on how `bestool` packages itself, and
wire it into the BES APT repository (`third-party-builds`). Tracked as TAM-6946.

## Locked-in decisions

Settled with the user before writing this plan:

1. **Release/versioning: set up release-plz**, mirroring bestool. release-plz
   opens a release PR that bumps the workspace version and, on merge, tags
   `v{{version}}`; that tag triggers the deb build workflow. No crates.io
   publish (all crates are `publish = false`).
2. **Service enablement:** `seedling.service` and `seedling-web.service` are both
   enabled and started on install (both have working defaults; the only hard
   startup requirement is the pinned daemon fingerprint, which the bootstrap
   auto-supplies). `seedling-web` authenticates via Tailscale identity headers
   and binds loopback, so it is inert until fronted. A separate
   `seedling-web-tailscale-serve.service` ships **disabled**; the operator
   enables it once Tailscale is up to expose the web interface over the tailnet.
   Password login is the documented alternative.
3. **Single `seedling` deb** containing all three binaries (`seedling`,
   `seedling-web`, `seedling-ctl`), all three units, and support files.
4. **Both PRs:** the seedling packaging PR and a `third-party-builds` PR wiring
   seedling into the APT index + install/verify tests. Both tagged TAM-6946.
5. **Full auto-wire of the seedling-web bootstrap** on first install (see
   below). Small additions to all three binaries.

## Background (verified in the code)

- Binaries: `seedling` (daemon, `crates/daemon`), `seedling-web`
  (`crates/web`), `seedling-ctl` (`crates/ctl`). Libs: `seedling-core`,
  `seedling-protocol`.
- The daemon **must run as root**: `modprobe jool`, systemd over D-Bus,
  nftables (`nft` via the `nftables` crate), podman, writes `/var/log/seedling`.
- Runtime state lives under `--data-dir` (default `.`; package uses
  `/var/lib/seedling`): `seedling.db`, `seedling.db.key`, `oi.key` (daemon
  identity), `authorized_keys` (bootstrap import file), `tls-cert-endpoint.token`,
  Caddy working files. SQLite is statically bundled (no libsqlite dep).
- Audit log: `--audit-log`, default `/var/log/seedling/audit.log`. A logrotate
  stanza already exists at `etc/logrotate.d/seedling`.
- `seedling-web`: reads an optional TOML config (`--config`); authenticates via
  Tailscale headers (`--trust-tailscale-headers`) or, if `auth.password_hash` is
  set, password login (starting does **not** require either). Keeps a persistent
  client key (`--key-file`, default under `$XDG_STATE_HOME`); connects to the
  daemon OI at `[::1]:7891`; in release **requires** a pinned daemon fingerprint
  (`--daemon-fingerprint` or `--daemon-fingerprint-file`). Ports: web HTTP 7894,
  web WT 7893, OI 7891.
- Containerfiles (Caddy, volume-shell) and the web SPA are **embedded** in the
  binaries — nothing extra to package.
- Dynamic shared-lib dep: `libsystemd0` (via the `systemd` crate; CI installs
  `libsystemd-dev`). External programs: `podman` (>=5), `nft` (nftables),
  `jool` + `modprobe`, `btrfs` (optional, `--without-btrfs`), `ip`, `sysctl`.

## The seedling-web bootstrap (full auto-wire)

Two directions must be established; both keys are generated on first run and
neither exists at package-install time.

- **web → daemon (authorise web's key):** the daemon imports
  `{data_dir}/authorized_keys` (`<fingerprint> <label>` lines) on startup
  (`crates/core/src/oi/auth.rs`). Dropping web's fingerprint there authorises
  it with no running daemon and no interactive step.
- **daemon → web (web pins the daemon):** the daemon's identity is
  `{data_dir}/oi.key`; its SPKI fingerprint is what web pins.

Code additions (each small, each independently useful):

1. `seedling-ctl`: add a global `--key-file <path>` overriding the default
   client-key path. Lets `seedling-ctl --key-file … client fingerprint`
   generate a key at a chosen path and print its fingerprint offline (the
   `client fingerprint` subcommand already never connects).
2. `seedling` daemon: after computing its OI fingerprint in `oi::server::run`,
   write it to `{data_dir}/oi.fingerprint` (world-readable), so other local
   processes can pin it without scraping logs.
3. `seedling-web`: add `--daemon-fingerprint-file <path>`; read the pin from
   that file inside the connect-retry loop (so it tolerates the daemon not
   having written it yet). Mutually exclusive with `--daemon-fingerprint`.

postinst (first install), before starting the daemon:

- create `/var/lib/seedling` (0750 root), `/var/log/seedling` (0750 root),
  `/etc/seedling`;
- if `/var/lib/seedling/web.key` is absent, run
  `seedling-ctl --key-file /var/lib/seedling/web.key client fingerprint` to
  generate it and capture `FP_W`;
- if `FP_W` not already present, append `FP_W seedling-web` to
  `/var/lib/seedling/authorized_keys`;
- `seedling.service` on start generates `oi.key`, writes `oi.fingerprint`, and
  imports `authorized_keys` → web is authorised.

`seedling-web.service` ships enabled with
`--daemon-fingerprint-file /var/lib/seedling/oi.fingerprint`,
`--key-file /var/lib/seedling/web.key`, and `--trust-tailscale-headers` baked in;
it wires up to the daemon with no manual fingerprint copying. It binds loopback
and is reachable only via `seedling-web-tailscale-serve.service` (shipped
disabled), which runs `tailscale serve` to terminate HTTPS and inject the
identity headers. All services run as root (operator-trust threat model; avoids
key-ownership juggling).

## Repo A — seedling changes

### Packaging assets
- `services/seedling.service` — `Type=simple`, root,
  `ExecStart=/usr/bin/seedling --data-dir /var/lib/seedling --audit-log
  /var/log/seedling/audit.log`, `Restart=always`, `After/Wants=network-online`,
  `[Install] WantedBy=multi-user.target`, reasonable `Protect*` hardening that
  still permits modprobe/nftables/podman/dbus.
- `services/seedling-web.service` — `Type=simple`, root, enabled on install,
  `ExecStart=/usr/bin/seedling-web --config /etc/seedling/web.toml
  --daemon-fingerprint-file /var/lib/seedling/oi.fingerprint
  --key-file /var/lib/seedling/web.key --trust-tailscale-headers`.
- `services/seedling-web-tailscale-serve.service` — `Type=oneshot`
  (`RemainAfterExit`), `ConditionPathExists=/usr/bin/tailscale` (no-op if absent),
  `tailscale serve --https=443 --bg localhost:7894`; shipped disabled.
- `etc/seedling/web.toml` — sample/`conffile` with a commented `[auth]`
  `password_hash` placeholder (the alternative to Tailscale auth).
- Reuse `etc/logrotate.d/seedling` → `/etc/logrotate.d/seedling`.
- `[profile.dist]` in root `Cargo.toml` (release + `lto`, `codegen-units=1`,
  `strip="symbols"`) for lean deb binaries.

### Maintainer scripts (built in the workflow)
- `postinst`: create dirs; web-key + authorized_keys bootstrap (idempotent);
  `systemctl daemon-reload`; on first install `systemctl enable --now
  seedling.service`.
- `prerm`: on remove, `systemctl --no-reload disable --now seedling.service`
  and `seedling-web.service` if present.
- `postrm`: `daemon-reload`; on `purge`, remove generated state under
  `/var/lib/seedling` and `/var/log/seedling`.

### Release automation
- `release-plz.toml`: workspace tag template `{{package}}-v{{version}}`;
  `seedling` package overridden to `git_tag_name = "v{{version}}"` with a GitHub
  release; lib crates get `release = false` (no per-crate tags/releases). No
  crates.io.
- `.github/workflows/release-plz.yml`: mirror bestool's (push-to-main opens/
  updates the release PR; auto-merge; `release-hold` escape hatch) minus the
  crates.io-auth step.
- `.github/workflows/release.yml`: on `push` tag `v*`. Build matrix:

  | target | runner | builds | artifacts |
  |---|---|---|---|
  | `x86_64-unknown-linux-gnu` | ubuntu-24.04 | all 3 | deb + 3 tars |
  | `aarch64-unknown-linux-gnu` | ubuntu-24.04-arm | all 3 | deb + 3 tars |
  | `x86_64-apple-darwin` | macos-15 | ctl + web | 2 tars |
  | `aarch64-apple-darwin` | macos-15 | ctl + web | 2 tars |
  | `x86_64-pc-windows-msvc` | windows-2022 | ctl + web | 2 tars (.exe) |

  Rationale: the `seedling` daemon is Linux-only (systemd/nftables/jool/podman
  via `seedling-core`); `seedling-ctl` and `seedling-web` depend only on
  `seedling-protocol`, so they cross-compile with **no** libsystemd/Linux
  coupling. On non-Linux targets build **only** `-p seedling-ctl -p
  seedling-web` so core/daemon (and libsystemd) are never compiled.

  All targets: set up Node + build the frontend for `seedling-web` (do **not**
  set `SKIP_FRONTEND_BUILD`). Linux: install `libsystemd-dev`. Windows: statically
  link the CRT (`-Ctarget-feature=+crt-static`) and use GNU tar for caching, per
  bestool. Build with `--profile dist`.

  Linux only: stage the deb tree (three binaries, two units, logrotate, sample
  web.toml, copyright, dirs), write `control` (deps below) + maintainer scripts,
  `dpkg-deb --build --root-owner-group`. Attest provenance for every built
  binary. OIDC to `arn:aws:iam::143295493206:role/gha-tamanu-tools-upload`
  (ap-southeast-2); `aws s3 cp seedling-*.deb
  s3://bes-ops-tools/seedling/<version>/`; CloudFront invalidate `/seedling/*`.

### Standalone binary tarballs (non-Debian distribution)
In the same `release.yml`, additionally publish each of the three binaries as
its own `.tar.zst` for hosts that don't use the APT repo, mirroring bestool's
tarball scheme (but **no** minisign signing — seedling has no self-update path;
integrity is covered by build-provenance attestation):
- `tar -C target/<target>/dist -cf - <bin> | zstd -19` for whichever binaries
  the target built: `seedling` + `seedling-ctl` + `seedling-web` on Linux,
  `seedling-ctl` + `seedling-web` on macOS/Windows (Windows uses the `.exe`).
- Upload to `s3://bes-ops-tools/<name>/<version>/<name>-<target>-<version>.tar.zst`
  and to `.../<name>/latest/…` (per the tpb URL scheme, one prefix per binary
  name — `seedling`, `seedling-ctl`, `seedling-web`), then CloudFront-invalidate
  each prefix.
- Also attach every tarball (and the deb) to the GitHub release that
  release-plz created for the tag, via `gh release upload`.

Deb `control`:
```
Package: seedling
Version: <version>
Section: admin
Priority: optional
Architecture: <amd64|arm64>
Depends: libc6 (>= 2.39), libgcc-s1, libsystemd0, podman (>= 5.0), podman (<< 6), nftables, iproute2, kmod
Recommends: btrfs-progs, jool-dkms, jool-tools
Suggests: tailscale
Maintainer: BES Developers <support@bes.au>
Description: Lightweight high-level container app management
Homepage: https://github.com/beyondessential/seedling
```

### Spec + docs (per AGENTS.md: spec first, then implement, then tests)
- Add spec items in `docs/spec` for: daemon publishing its OI fingerprint to a
  file; web pinning via a fingerprint file; ctl configurable key path. Annotate
  impls/tests with tracey refs; `tracey query status` clean.
- New `docs/deploying.md` (or extend README): APT install, config of
  `web.toml`, enabling `seedling-web`, dependency notes (jool DKMS, podman).

## Repo B — third-party-builds changes
- `.github/workflows/apt-repo.yml`: add `seedling` to the non-glibc-variant
  download loop (`for dir in caddy kopia bestool algae` → add `seedling`); add
  `seedling` to the `apt-get install` + verify lists in `test-repo` and
  `test-repo-26-04`; optionally add the seedling release to the `workflow_run`
  trigger note (it lives in another repo, so scheduled/dispatch reindex covers
  it).
- `README.md`: add a "Seedling" Builds entry (first-party, built in its own
  repo, published to `s3://bes-ops-tools/seedling/…`).

## Verification
- `dpkg-deb --contents` / lintian-style sanity on the built deb locally if
  feasible; otherwise rely on the tpb `test-repo` jobs (install + `--version`).
- Bootstrap dry-run: on a scratch dir, `seedling-ctl --key-file … client
  fingerprint` prints a fingerprint; daemon writes `oi.fingerprint`; web with
  `--daemon-fingerprint-file` reads it.
- `just check` (clippy + fmt), `just test`, `tracey query status`.

## Out of scope / follow-ups
- Running `seedling-web` as a non-root user (deferred; operator-trust model).
- The `seedling-web`-as-managed-app direction (`docs/plans/seedling-web-as-app.md`)
  would eventually supersede the `seedling-web.service` unit.
