# Dev VM notes

Gotchas for running the daemon and real apps in a Linux dev VM (Lima on
macOS, Ubuntu 25.04 aarch64).

## Container signals are denied on Ubuntu 25.04

Workloads hang rather than erroring: PostgreSQL connections stall, `gosu`
spins at 100% CPU, stops escalate to SIGKILL. Mechanism and `dmesg` signature
are in the "Known host issue" section of [deploying.md](deploying.md). The
dev VM remedy, both parts needed:

```bash
# podman itself is confined when launched from systemd units; unconfine it
sudo ln -s /etc/apparmor.d/podman /etc/apparmor.d/disable/
sudo apparmor_parser -R /etc/apparmor.d/podman

# crun mishandles the profile stacking either way; use runc
sudo tee /etc/containers/containers.conf.d/50-runc.conf <<EOF
[engine]
runtime = "runc"
EOF
```

Interactive `sudo podman run` works without this (a root shell's podman is
unconfined), so a workload that runs by hand but hangs under the daemon's
systemd units is this issue, not your code.

## Simulating a packaged install

Tooling that detects a Seedling host (e.g. `bestool tamanu <cmd>`) keys off
what the APT package installs: the `seedling.service` unit and
`/usr/bin/seedling-ctl`. A source-built VM has neither, so detection reports
no Seedling. To make a dev VM look like a real host:

```bash
sudo ln -s ~/target/debug/seedling /usr/bin/seedling
sudo ln -s ~/target/debug/seedling-ctl /usr/bin/seedling-ctl
sudo tee /etc/systemd/system/seedling.service <<EOF
[Unit]
Description=Seedling application orchestrator (dev VM)
After=network-online.target podman.socket
Wants=podman.socket

[Service]
ExecStart=/usr/bin/seedling --data-dir /home/YOU/seedling-data -v
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload && sudo systemctl start seedling
```

## Bringing up the postgres app

The app declares an external `data` volume, so install fails with
`external volume 'data' is not mapped` until one is attached:

```bash
seedling-ctl volumes site create-managed pgdata
seedling-ctl apps volumes attach postgres data _site/pgdata
seedling-ctl apps param set postgres version 18.3
seedling-ctl apps param set postgres password devpassword
seedling-ctl apps install postgres
```

Params set through `apps param set` persist across script updates; install
params passed positionally do not reach `app.param()` values.

If an install fails partway, `apps uninstall` then `apps install` resets the
reconciler's view cleanly; clearing faults alone can leave it oscillating
(stopping and recreating a healthy container every few seconds). Data
volumes survive an uninstall.

## Disk pressure

Debug cargo target dirs run 6 to 12 GB each, and one per checkout fills the
default VM disk quickly; a full disk surfaces as a linker error
(`No space left on device`). Keep target dirs on the VM disk (not the
virtiofs mount) but prune superseded ones, and `podman image prune` catches
multi-GB dangling images left by app installs.
