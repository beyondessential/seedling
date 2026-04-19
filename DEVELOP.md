# Development setup

Development on anything else than Linux is impractical.

You'll need the `jool` and `jool-tools` packages (Arch calls the first one `jool-dkms`) so that NAT64 is available.
There's no need to configure NAT64 yourself: Seedling does that.

You'll need podman >=5.0 installed, and for your OS to be on systemd.

Watchexec is recommended. `just` is used to run common tasks — see the `justfile` at the repo root.

## Building and running

Two terminal windows (or tmux panes):

```
just watch-build
```
```
just watch-run
```

`watch-run` restarts the daemon whenever the binary changes. It logs to `seedling.log` and writes TLS keys to `/tmp/seedling.keylog` (configure Wireshark to read from there when debugging the OI protocol). The data dir defaults to `/opt/seedling`; override with `just watch-run data_dir=~/seedling-data`. Verbosity defaults to `-v`; override with `just watch-run verbosity=-vvv`.

## Web UI

The web UI is a React/Vite SPA served by `seedling-web`. In production it is embedded directly into the binary via `rust-embed`. For development, run the Rust server and the Vite dev server side by side in two terminals:

```
just web
```
```
just frontend
```

Open **`http://localhost:5173`** (Vite's port, not the Rust server's). Vite's dev server is configured to proxy `/connect` and `/healthz` to the Rust server at `:8080`, so API calls work and HMR is fully functional.

`SKIP_FRONTEND_BUILD=1` is set automatically by `just web` and `just build` so that cargo does not run `npm run build` on every compile. You need a `frontend/dist/` to exist for the Rust binary to compile in this mode — run `just frontend-build` once if it doesn't, or run `just build-release` for a full embedded production build.

If you add npm dependencies, run `just frontend-install` first.

## Controlling

Use `just ctl <args>` to invoke `seedling-ctl`, e.g. `just ctl app list`. You'll need to follow the bootstrap guide in the README on first start to authenticate.

Keep `just events` running in a spare window to tail the live event feed.

## Principles

- Restart and reboot resilience: if Seedling stops, workloads should continue unimpeded (for the most part, some things might not be possible); when it starts again, it must take back control with the least possible disruption. This makes restarting/upgrading Seedling painless. When the server as a whole reboots, Seedling must restore all workloads to their desired state, so there's minimal downtime.
- Quiet and lightweight: Seedling is designed to use few resources, to leave as much as possible to the actual workloads. When there are no active workloads, it even stops infrastructure services to reduce its footprint further.
- Feels like Kubernetes: while Seedling is higher level than Kubernetes, someone familiar with K8s should feel reasonable comfortable: terminology is similar, behaviour is comparable. At the same time, Seedling is more opinionated, we don't want to emulate K8s 1:1.
- Comfy interfaces: CLI/API/Web UI/etc should be comfy, and not frustrate operators. Have consistent command design, don't have a million options, don't require arcane invocations. If we can make it easier for the user without surprising them, do so.
