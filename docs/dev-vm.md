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

The package also bootstraps the host identity that a `bestool` which reads
`/etc/bestool/seedling.key` uses to reach the daemon (releases that don't yet
use the operator's own key). To have it on a dev VM, mint and authorise it by
hand, pointing at the data directory the unit above uses:

```bash
fp=$(sudo seedling-ctl --key-file /etc/bestool/seedling.key client fingerprint)
echo "$fp bestool" | sudo tee -a /home/YOU/seedling-data/authorized_keys
sudo systemctl restart seedling
```

## Bringing up a demo app

The definitions under [`demos/`](../demos/) exercise Seedling on a dev VM;
none of them is a production definition. Each installs with no parameters
set, other than the one `params-tour` requires to show what a required
parameter looks like, and serves on a hostname under `.demo.internal`, which takes its
certificate from the proxy's internal CA without any TLS setup. Point that
hostname at the VM in your `/etc/hosts` to reach it.

`actions-and-gates` walks through gates, actions and shells:

```bash
seedling-ctl apps create actions-and-gates ./demos/actions-and-gates/
seedling-ctl apps install actions-and-gates
curl -k https://actions-and-gates.demo.internal/
```

The app stays `installing` until every gate it declares has been satisfied at
once. A gate whose run fails holds a `gate_failed` fault and is retried at a
growing delay, capped at an hour; `seedling-ctl apps gate run actions-and-gates <gate>`
runs it now, and `apps show actions-and-gates` lists each gate's state. A gate
whose key moves, through `apps params set` or a new definition, runs again on
its own:

```bash
seedling-ctl apps params set actions-and-gates format=3       # migrate steps the store up
seedling-ctl apps params set actions-and-gates format=1       # migrate refuses, gate_failed
seedling-ctl apps params set actions-and-gates greeting=Hello # render reruns, migrate does not
```

Params set through `apps params set` persist across definition updates.
Actions and shells take their params as `name=value`, and an action with a
`volume` param binds a site volume for that run alone:

```bash
seedling-ctl apps action actions-and-gates note text=hi copies=3
seedling-ctl volumes site create-managed scratch
seedling-ctl apps action actions-and-gates export destination=scratch
seedling-ctl apps shell actions-and-gates store user=root
```

If the app is wedged some other way, `apps uninstall` then `apps install`
resets the reconciler's view cleanly; clearing faults alone can leave it
oscillating (stopping and recreating a healthy container every few seconds).
Data volumes survive an uninstall, and an uninstall discards every gate's
record, so installing again runs each gate as for the first time.

## Disk pressure

Debug cargo target dirs run 6 to 12 GB each, and one per checkout fills the
default VM disk quickly; a full disk surfaces as a linker error
(`No space left on device`). Keep target dirs on the VM disk (not the
virtiofs mount) but prune superseded ones, and `podman image prune` catches
multi-GB dangling images left by app installs.
