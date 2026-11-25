# Test cases: portable core and Linux platform crate

## Portability

- [ ] `cargo clippy -p seedling-core --all-targets` passes on macOS and on Windows in CI
- [ ] `seedling-core`'s dependency tree, on a Windows target, holds none of nftables, rtnetlink, systemd, Podman's client or nix
- [ ] Linux CI still builds and tests the whole workspace

## Behaviour unchanged on Linux

- [ ] The existing test suites of core, the daemon and the CLI pass with no test removed to make them pass
- [ ] Core's reconciler tests run against the stub backends, with no Linux adapter in core's dependency graph
- [ ] The adapters' own tests move with them into `seedling-linux` and still run

## Snapshotting volume store (verifies spec: ACTU#snapshotting-check, LINUX#volume-storage)

- [ ] On a data directory not on BTRFS, the daemon exits with an error naming `--without-btrfs`, unless started with it
- [ ] With `--without-btrfs`, volumes are plain directories and snapshot and promotion requests are rejected with `requirements_invalid`
- [ ] On BTRFS, a snapshot site volume is a read-only snapshot and a promoted snapshot is writable

## Packet filter (verifies spec: FLT#host-faults, LINUX#packet-filter)

- [ ] A failed packet-filter apply files `packet_filter_failed`, and the next successful apply clears it
- [ ] A withheld apply neither files nor clears `packet_filter_failed`
- [ ] An apply replaces only the `inet seedling_net` table and leaves a rule in another table in place
