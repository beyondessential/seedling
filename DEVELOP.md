# Development setup

Development on anything else than Linux is impractical.

You'll need the `jool` and `jool-tools` packages (Arch calls the first one `jool-dkms`) so that NAT64 is available.
There's no need to configure NAT64 yourself: Seedling does that.

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
watchexec -IrW target/debug --ignore-nothing -E SSLKEYLOGFILE=/tmp/seedling.keylog 'sudo --preserve-env=SEEDLING_LOG --preserve-env=SSLKEYLOGFILE target/debug/seedling --data-dir /opt/seedling -v 2>&1 | tee -a seedling.log'
```

This starts the server with debug logging, you can remove the `-v` or add more e.g. `-vvv` to change that.
It also puts the logs in seedling.log so tools can query that.
The TLS keys are logged to /tmp/seedling.keylog: you can configure Wireshark to read from that to get useful information out of it when debugging the RPC "OI" protocol.
The state/data-dir is set to /opt/seedling to simulate an install without putting root-owned files in your home/source directory.

## Web UI

The web UI is a React/Vite SPA served by `seedling-web`. In production it is embedded directly into the binary via `rust-embed`. For development, run the Rust server and the Vite dev server side by side in two terminals:

```
just web
```
```
just frontend
```

`just web` starts `seedling-web` with `--dev-no-auth` (no password needed) and `--vite-port 5173`, which proxies all SPA requests to Vite. Open the URL printed by the Rust server (e.g. `http://localhost:8080`); the page will be served through the proxy so Vite's HMR websocket won't reach the browser directly. If you need HMR, open Vite's own URL (`http://localhost:5173`) instead and configure its proxy to forward `/connect` and `/healthz` to the Rust port.

`SKIP_FRONTEND_BUILD=1` is set automatically by `just web` and `just build` so that cargo does not run `npm run build` on every compile. Run `just frontend-build` or `just build-release` when you need a fresh embedded bundle.

If you add npm dependencies, run `just frontend-install` first.

## Controlling

You can then use `target/debug/seedling-ctl` to interact with Seedling.
You'll need to follow the bootstrap guide in the README on first start to authenticate to the instance, and then it will work without further issue.

Keeping `target/debug/seedling-ctl op events` running in another window is a good way to keep an eye on the server event feed while it's working.

## Principles

- Restart and reboot resilience: if Seedling stops, workloads should continue unimpeded (for the most part, some things might not be possible); when it starts again, it must take back control with the least possible disruption. This makes restarting/upgrading Seedling painless. When the server as a whole reboots, Seedling must restore all workloads to their desired state, so there's minimal downtime.
- Quiet and lightweight: Seedling is designed to use few resources, to leave as much as possible to the actual workloads. When there are no active workloads, it even stops infrastructure services to reduce its footprint further.
- Feels like Kubernetes: while Seedling is higher level than Kubernetes, someone familiar with K8s should feel reasonable comfortable: terminology is similar, behaviour is comparable. At the same time, Seedling is more opinionated, we don't want to emulate K8s 1:1.
- Comfy interfaces: CLI/API/Web UI/etc should be comfy, and not frustrate operators. Have consistent command design, don't have a million options, don't require arcane invocations. If we can make it easier for the user without surprising them, do so.
