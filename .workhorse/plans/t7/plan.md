# Split seedling-core into a portable core and a Linux platform crate

## Why

T6 (`definitions check` in `seedling-ctl`) needs the evaluator on every platform `seedling-ctl` ships for: Linux, macOS and Windows.
Making `seedling-core` portable unblocks it. A runtime for another platform is a long-run possibility, not a goal of this card.

## Tech notes

### Where the line falls

Core keeps every decision; `seedling-linux` holds only the adapters that touch the host.

- Stays in core: `defs/`, `runtime/`, `oi/`, the reconciler (`system/reconcile*`), `system/translate/`, Caddy and CoreDNS configuration (they run as infrastructure containers through the container-runtime trait), `stub.rs` and `unavailable.rs`.
- Moves to `seedling-linux`: the Podman, systemd and journal clients, the nftables data plane, rtnetlink address and route reading, Jool, the btrfs volume store, confinement, host probes, and host facts read from `/proc`.
- Core gains traits for the facilities not yet behind one: log reading, the volume store, routes and addresses, the NAT64 translator, host probes and host facts. They join the existing `ContainerRuntime`, `ProcessManager`, `NetworkProxy` and `DataPlane`.

## Explicitly rejected

- Moving all of `system/` into `seedling-linux`, reconciler included. Another platform would have to rewrite the reconciler.
- Lifting only the evaluator into a new crate, leaving core Linux-only.
