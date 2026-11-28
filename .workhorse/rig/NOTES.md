# Seedling rig

A lease is the daemon and `seedling-web` at the commit, on 127.0.0.1, with the web UI at the base URL.

- The daemon runs on in-memory backend stubs, the mode the Playwright e2e fixture and the DAST stack use: no podman, systemd, nftables, Caddy or NAT64, so it needs no root. The operator interface, database, scheduler and event feed are real. Workloads never actually run.
- `seedling-web` runs with authentication off, so there is no login screen. It refuses that mode on any address but loopback.
- `build`: the frontend bundle, then a debug build of the daemon, web server and CLI (`--trust-any` exists only in debug builds). The target directory is removed once the binaries are copied out.
- `serve`: in a directory of the lease's own under the build, generates a client key, authorises it, starts the daemon, runs `fixtures.sh` through the CLI, then runs the web server in the foreground. `teardown` removes that directory. The lease's `info.ctl` is a CLI wrapper pointed at that daemon: `"$ctl" apps show hello-world`.

Needs `cargo` and `node`/`npm` on the PATH the manager runs with. Recipe steps run with a home of the rig's own, so `lib.sh` points rustup and cargo at the person's `~/.rustup` and `~/.cargo`, unless `RUSTUP_HOME` or `CARGO_HOME` is set, so a build shares the person's toolchain, crate cache and compiler wrapper.

There is no Postgres, so `rig reset` does nothing: release the lease and take another for a fresh daemon.
