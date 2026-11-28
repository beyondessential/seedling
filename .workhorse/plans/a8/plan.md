# Workhorse rig and design library

## Notes

- The rig serves what the e2e fixture and `etc/ci/dast-stack.sh` already stand up: the daemon on in-memory backend stubs (no root, podman, nftables, Caddy or NAT64) and `seedling-web --dev-no-auth` against it. No Postgres: the daemon keeps its own SQLite under the lease's scratch directory.
- Debug build: `seedling-ctl --trust-any` exists only in debug builds, and the tester drives the daemon through a ctl wrapper the lease hands back. The frontend is a production Vite bundle either way.
- Per-build `target/`, deleted once the three binaries are copied out, to stay inside the card disk budget. Speed comes from the person's own cargo home and compiler wrapper (sccache on this machine), taken through `env.fromPerson`/`personHome`. A target directory shared across builds was rejected: cargo's mtime freshness lets a build at an older checkout reuse a newer commit's artefacts.
- `reset` only runs for Postgres recipes, so a lease is reset by releasing it and leasing again.
- Base mockups are snapshots: faithful captures of the shipped screens taken from a lease with Playwright, CSS serialised from the CSSOM (MUI/emotion inject rules without text), in the same header-comment format Workhorse's own capture produces.

## Checklist

- [ ] Rig recipe in `.workhorse/rig/`: `rig.json`, `deps`, `build`, `serve`, `lib.sh`, `NOTES.md`
- [ ] Fixtures: demo apps created and installed through the ctl at serve time
- [ ] Prove a lease at a committed recipe: web UI answers, apps listed
- [ ] `design-system.md`: palette (light and dark), type, spacing, radius, elevation, layout shell, status colours, safety mode, copy rules
- [ ] Component docs under `components/` for the recurring parts, each with an HTML preview
- [ ] Snapshots of the main screens under `snapshots/`, assets under `assets/`
- [ ] Spec style and citation checks pass
