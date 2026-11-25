# Test cases: portable core and Linux platform crate

## Portability

- [ ] `cargo clippy -p seedling-core --all-targets` passes on macOS and on Windows in CI
- [x] `seedling-core`'s dependency tree, on a Windows target, holds none of nftables, rtnetlink, systemd, Podman's client or nix
- [x] `seedling-core` builds with all its targets for Windows and macOS when cross-compiled from Linux
- [x] Outside the daemon, `SystemFacts::default()` reports `AVAILABLE_MEMORY` undetermined (verifies spec: CONST#host-constants)
- [ ] Linux CI still builds and tests the whole workspace

## Behaviour unchanged on Linux

- [x] The existing test suites of core, the daemon and the CLI pass with no test removed to make them pass
- [x] Core's reconciler tests run against the stub backends, with no Linux adapter in core's dependency graph
- [x] The adapters' own tests move with them into `seedling-linux` and still run
- [x] A workload unit takes its tier slice and kill preference from its standing, and an infrastructure unit the infrastructure slice and -900 (verifies spec: LINUX#kill-order, LINUX#resource-control-groups)
- [x] An app's standings hold every Deployment level it declares and Normal (verifies spec: PRIO#resource-control-groups)
- [x] An exit code of 128 plus a signal number names that signal, and a real-time signal's stays a bare number
- [x] The node prefix is a `fd5e` /48 derived from the trimmed machine identifier, and an empty identifier is refused (verifies spec: INFRA#host-prefix-derivation)
- [ ] In stub mode, the daemon runs with no NAT64 translator set up on the host

## Snapshotting volume store (verifies spec: ACTU#snapshotting-check, LINUX#volume-storage)

- [ ] On a data directory not on BTRFS, the daemon exits with an error naming `--without-btrfs`, unless started with it
- [ ] With `--without-btrfs`, volumes are plain directories and snapshot and promotion requests are rejected with `requirements_invalid`
- [ ] On BTRFS, a snapshot site volume is a read-only snapshot and a promoted snapshot is writable

## Packet filter (verifies spec: FLT#host-faults, LINUX#packet-filter)

- [x] A failed packet-filter apply files `packet_filter_failed`, and the next successful apply clears it
- [x] A withheld apply neither files nor clears `packet_filter_failed`
- [ ] An apply replaces only the `inet seedling_net` table and leaves a rule in another table in place
