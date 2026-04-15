# Development setup

Development on anything else than Linux is impractical.

You'll need the `jool` and `jool-tools` packages (Arch calls the first one `jool-dkms`) so that NAT64 is available.
There's no need to configure NAT64 yourself: seedling does that.

You'll need podman >=5.0 installed, and for your OS to be on systemd.

Watchexec is recommended.

## Building and running

What I usually do is have two terminal windows (or tmux panes):

One to build on changes:

```
watchexec cargo build
```

One to restart the server on successful builds:

```
watchexec -IrW target/debug --ignore-nothing -E SSLKEYLOGFILE=/tmp/seedling.keylog 'sudo --preserve-env=SEEDLING_LOG --preserve-env=SSLKEYLOGFILE target/debug/seedling --data-dir /opt/seedling -v | tee seedling.log'
```

This starts the server with debug logging, you can remove the `-v` or add more e.g. `-vvv` to change that.
It also puts the logs in seedling.log so tools can query that.
The TLS keys are logged to /tmp/seedling.keylog: you can configure Wireshark to read from that to get useful information out of it when debugging the RPC "OI" protocol.
The state/data-dir is set to /opt/seedling to simulate an install without putting root-owned files in your home/source directory.

## Controlling

You can then use `target/debug/seedling-ctl` to interact with seedling.
You'll need to follow the bootstrap guide in the README on first start to authenticate to the instance, and then it will work without further issue.

Keeping `target/debug/seedling-ctl op events` running in another window is a good way to keep an eye on the server event feed while it's working.

## Principles

- Restart and reboot resilience: if seedling stops, workloads should continue unimpeded (for the most part, some things might not be possible); when it starts again, it must take back control with the least possible disruption. This makes restarting/upgrading seedling painless. When the server as a whole reboots, seedling must restore all workloads to their desired state, so there's minimal downtime.
- Quiet and lightweight: seedling is designed to use few resources, to leave as much as possible to the actual workloads. When there are no active workloads, it even stops infrastructure services to reduce its footprint further.
- Feels like Kubernetes: while seedling is higher level than Kubernetes, someone familiar with K8s should feel reasonable comfortable: terminology is similar, behaviour is comparable. At the same time, seedling is more opinionated, we don't want to emulate K8s 1:1.
- Comfy interfaces: CLI/API/Web UI/etc should be comfy, and not frustrate operators. Have consistent command design, don't have a million options, don't require arcane invocations. If we can make it easier for the user without surprising them, do so.
