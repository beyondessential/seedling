# Operator Guide

This is a tour of what an operator can do with Seedling. It focuses on the web UI, with a brisker section at the end for `seedling-ctl`.

If you want to write or extend an app, see the [BSL scripting guide](./bsl-scripting.md). This document is the inverse: it assumes apps already exist and you want to run, observe, or modify them.

## Signing in

The login page takes a password. Once authenticated, the session lives for as long as the page does, with reconnection handled automatically — if the daemon goes away you'll see an "Offline" page that retries until the daemon comes back.

## The top bar

Every page has the same toolbar. From left to right:

- **Logo and hostname** — back to the apps dashboard.
- **Connected clients** — count of live web sessions, shells, and port forwards; clicking jumps to the apps page.
- **Per-section icons** — quick links to OI keys, registries, images, services, site ingresses, certificates, volumes, backups, and templates. Some show a badge (e.g. held volumes pending review).
- **Faults** — red chip with the active fault count. Click to see the [faults page](#faults).
- **Reconnecting** — a chip that appears whenever the daemon link is interrupted.
- **Safety mode** — the lock/shield/warning icon at the right; see the next section.

## Safety modes

Seedling has three safety modes that gate destructive actions in the UI:

- **Read-only** — view everything; no buttons that write are enabled.
- **Write** — routine mutations (start/stop, edit script, set params) are allowed; destructive actions stay locked.
- **Dangerous** — temporary elevation, with a visible countdown, that unlocks delete/uninstall/wipe operations.

Switching to Dangerous prompts for confirmation and reverts after the timer. Buttons that are currently locked show a tooltip explaining why.

## Apps

### Dashboard

The home page (`/`) lists every registered app with its status (`not_installed`, `installing`, `operating`, `stopped`) and any in-flight action. A "Partially running" chip appears when some resources of an otherwise-running app are stopped. Clicking a row opens its detail page; the **New** button starts a [fresh app](#creating-an-app).

Below the apps list, four sections appear when there's activity to show: **Web sessions** (other operators logged into the UI), **Active operators** (recent activity attributed to web users or CLI sessions, with last action and timestamp), **Shells** (open interactive sessions, stoppable with the X button), and **Port forwards** (active TCP/UDP forwards initiated from the CLI, also stoppable).

### Creating an app

From `/apps/new`, give the app a name (3–63 chars, ASCII alphanumeric/hyphens, must start with a letter) and paste or write a Rhai script in the editor. The **Review** button asks the daemon to plan the script and shows you what resources it will produce, including any validation errors. Confirm to register the app.

### App detail

The app page is the central hub. The header shows status, generation, and buttons for **Logs**, **Edit script**, **Stop**/**Uninstall**, and refresh. If an operation is in flight, an alert summarises its barrier (which lifecycle state it's waiting for, elapsed/deadline) and lets you cancel.

#### Faults panel

If the app has active faults, they're listed at the top — kind, resource, instance, description, timestamp. **Clear all** removes them; the gating tier varies by app status (clearing faults on a running app is more dangerous than on an uninstalled one).

#### Params

The script's `app.param(...)` declarations show up here as a table. Click a value to edit it inline; password-kinded params can be revealed; the X removes a value (revealing the default, if any). Use the row at the bottom to add a key the script doesn't statically declare — anything ending in `_volume` or `_filename` is reserved for the runtime and rejected.

#### External volumes

If the script declares any `app.external_volume(name)` slots, they appear here with their current mapping. Use **Map** to bind a slot to a site volume or another app's volume, **Edit** to change the target, **Unlink** to clear it. Read-only mappings are flagged with a chip.

#### Actions

Every `on_action`, `on_install`, scheduled action, and `on_shell` is listed with its kind and description. **Run** invokes the action; if it has params, a dialog collects them first. **Open shell** does the same for `on_shell` actions. Scheduled actions show their cron expression in a tooltip. Buttons disable themselves while another operation is running, while the script has errors, or in unsafe app states.

#### Schedules

A consolidated view of every scheduled action across the app, with its expression. Editing schedules is done by editing the script.

#### Resources

A card per resource. Deployments and jobs show their image, memory/CPU limits, mounts (volumes and bound services), bindings (TCP/UDP/HTTP), healthcheck state, and a per-instance table with logs/shell/snapshot affordances. Ingresses show their hostname, termination chip, and any redirect. Each card has scale up/down, restart, and stop/unstop controls where applicable.

#### TLS certificates

If the app has TLS-terminating ingresses, this section surfaces the per-hostname rollup (provider, current status, expiry, renewal). It's the same widget as the [Certificates page](#tls-certificates) but filtered to this app.

#### App images

Lists every container image the app references, with size and pin/in-use state. **Warm** pre-pulls and pins the images so the next deploy doesn't have to wait. **Remove** evicts an image (refused if a container is using it). **Clear pins** unpins all images for the app.

### Editing the script

`/apps/{name}/script` shows the current generation's script in a Rhai editor. **Review** runs `app.plan` and shows a structured diff of what will change, plus any validation errors. Saving applies the new generation. The editor is disabled while a planning or saving operation is in progress.

### Logs

`/apps/{name}/logs` streams logs from any of the app's resources. Pick a resource and (optionally) a specific instance from the dropdowns; choose tail length (50/100/200/500/all); toggle **Follow** to pause/resume live updates. Stderr lines are tinted; the buffer caps at 2000 lines.

### Shell

`/apps/{name}/shell/{name}` opens an xterm.js terminal wired into the daemon. Resize is debounced and forwarded; exit code is shown when the shell ends. Use this for `on_shell` actions and for the volume-mounted shells launched from the volumes page.

## Volumes

`/volumes` is the volumes management page. It has up to four sections.

### Site volumes

Volumes managed by the site itself, independent of any app. Three kinds: **managed** (a BTRFS subvolume Seedling owns), **bind** (a reference to an existing host path), and **snapshot** (a read-only BTRFS snapshot of another volume). The row actions are open shell, snapshot, **Promote** (snapshot only — copy into a fresh managed volume), and delete.

### App exports

Volumes that apps have declared with `volume.exported(...)`. Operators can open a shell into them or take a snapshot.

### External volume requests

The other side of [external volumes on the app page](#external-volumes): every `external_volume` slot declared by every app, with its current mapping. Map/edit/unlink from here when you'd rather work app-by-app from the volumes page.

### Held volumes

A volume goes into the "held" archive when an app is uninstalled or its declaration removed — the data is preserved instead of being deleted. **Restore** copies it back into a fresh site volume; **Delete** is permanent and dangerous-gated.

### Multi-volume shell

The **Open shell…** button at the top of the page opens a dialog where you tick checkboxes across site volumes, app volumes, and held volumes; each selected volume is mounted side-by-side under `/mnt/<name>` in a fresh shell. Useful for ad-hoc cross-volume work like inspection or copying.

## Services

`/services` is split between site-level service definitions and apps' external-service slots.

### Site services

A site service is a TCP/UDP/HTTP endpoint that any app can bind its `external_service` slot to. Create one with a name, port, and protocol; once created, edit or delete from the row. Deletes are refused while an app slot still points at the service.

### App external services

Each `app.external_service(name)` slot declared in any app's script. Map a slot to a site service or to another app's service, edit the target, or unlink it. Read-only is flagged where supported.

## Site ingresses

`/ingresses` lists site-level reverse-proxy entries (separate from `service.ingress(...)` declared in BSL scripts). Each entry has a hostname and TLS provider; click an entry to expand its attachments. **New** creates a manual ingress; entries discovered automatically (e.g. via Tailscale) appear here too and aren't deletable from the UI.

Each ingress has attachments — `(port, protocol, target)` mappings that tie the hostname to a specific app service or, for HTTP, a redirect URL. The dialog supports `tcp`, `udp`, `http`, and `http2` attachments, plus redirect codes 301/302/307/308 with optional path preservation. All ingress actions are dangerous-gated.

## TLS certificates

`/certificates` is the TLS control plane.

### Settings

Global ACME contact email and preferred CA profile (e.g. Let's Encrypt's `shortlived` for ~6-day certs). Setting an email enables auto-issuance for the first matching policy.

### DNS providers

Configured DNS-01 providers (Route 53 today; the kind list is open-ended). Credentials are write-only — the UI never displays them once stored. Edit replaces; delete is dangerous-gated.

### Policies

Per-hostname (or wildcard, including `*.example.com` shell-glob and a catch-all `*`) bindings to a DNS provider, with auto-renew toggle. Hostnames without a policy fall back to ACME-HTTP-01 on the public proxy.

### Certificates

Every stored certificate, ACME or manual or CSR-uploaded, with hostname, provider, issuance window, and status (active, expiring soon, expired). Per-cert actions are renew, retry, and delete. The detail expansion shows SANs, issuance history, and renewal log.

### Hostnames table

A unified per-hostname rollup of every TLS-terminating ingress in the system: who issues it, when it last renewed, retry blocks, etc. Same widget that appears on the app detail page when filtered by app.

## Backups

`/backups` manages backup strategies — combinations of (backup app, schedule, source volumes). Backup apps are normal apps that declare the `save-snapshot`, `list-snapshots`, and `restore-snapshot` actions; once registered, they show up in strategy creation.

Each strategy row has **Run** (manual trigger), **View snapshots**, edit, and delete. The snapshots dialog lists every snapshot the backup app reports, filterable by source volume; **Restore** spawns the app's restore-snapshot action and stores the result in a fresh site volume.

The page also surfaces the last run result, with status, duration, and any error message.

## Images

`/images` is the local container-image cache. The list shows reference, size, in-use status, and pinning. **Remove** evicts an image (refused if in use); **Sweep** removes every unpinned, unused image; pins protect images from sweeps and the autonomous garbage collector.

The Image Pins panel lists every active pin across every app, with a per-pin clear and a global clear-all.

## Registries

`/registries` is the container-registry allowlist. Anything that's not on this list will fail to pull and will file a `disallowed_registry` fault. Defaults are `docker.io` and `ghcr.io`; add and remove freely. Removal is dangerous-gated.

## Templates

`/templates` lists BSL script templates — saved scripts that operators can instantiate as new apps without writing them from scratch. **New** creates a template (name, optional description, script body); the detail page renders it read-only with a preview of declared resources, and **Edit** opens a CodeMirror editor. **Instantiate** clones the template into a new app under the name you supply.

## Faults

`/faults` shows every active fault across the system: kind, app, resource, instance, description, and when it was filed. Faults are real-time — they appear and disappear as the daemon files and clears them. The chip in the toolbar reflects the same count.

## OI Keys

`/keys` manages authorised CLI client keys. Each key is identified by a SHA-256 fingerprint and tagged with an operator label. Add a key with **New** (paste the fingerprint operators see when they first run `seedling-ctl client fingerprint`); remove to revoke.

## Infrastructure logs

`/infra/{component}/logs` streams logs from the bundled containers — currently the proxy (Caddy) and the resolver (CoreDNS). Same controls as app logs: tail length, follow toggle, refresh.

## seedling-ctl

The CLI is a thin shell over the same OI as the web UI, with a few things only it can do.

### Setup

On first run the CLI generates a client key under `$XDG_STATE_HOME/seedling/client.key` (mode 0600). When you connect to a new endpoint, you'll be prompted to confirm the server's fingerprint, which is then saved to `known_hosts` alongside the key. Subsequent connections to that endpoint are silent; if the server fingerprint changes, the CLI refuses to connect and prints a warning to remove the stale entry.

The operator on the daemon side adds your client fingerprint via the [OI Keys](#oi-keys) page (or `seedling-ctl user add` from another already-trusted client).

Global flags worth knowing:

- `--endpoint <addr>` — OI endpoint, default `[::1]:7891`.
- `--fingerprint <hex>` — pin a specific server fingerprint (skip first-run prompt).
- `--trust-any` — for development only; disables fingerprint verification.
- `-v`/`-vv`/`-vvv` or `SEEDLING_LOG=...` — verbose output.

`seedling-ctl client fingerprint` prints your own client key fingerprint without contacting any server.

### Common subcommands

Each subcommand has its own `--help`; below is just the shape:

- **`apps`** — register, list, show, create, update, remove, uninstall, install. Plus `apps action`/`shell`/`logs`/`scale`/`restart`/`stop-resource`/`unstop`/`unstop-resource`/`cancel-action`, `apps param set/unset`, `apps script`, `apps generations`, `apps plan`, `apps volumes list/attach/detach`. The CLI is the only place where you can dump a script with `apps script <name>` or browse history with `apps generations`.
- **`volumes`** — `site` (`create-managed`, `create-bind`, `list`, `delete`, `snapshot`, `promote`), `held` (`list`, `delete`, `restore`), and `shell` (the same multi-volume shell as the UI; pass volumes as `_site/NAME`, `APP/VOLUME`, or `held:ID`, optionally `--read-only`).
- **`services`** — `site` (`create`, `list`, `add-port`, `remove-port`, `delete`), `app list`, `exported list`, `external` (`map`, `unmap`, `remap`, `list`, `declared`).
- **`ingresses site`** — `list`, `show`, `create`, `update`, `delete`, `attach`, `attach-redirect`, `detach`, plus `discovery status` to see what providers like Tailscale have populated.
- **`tls`** — `dns-providers` (`list`, `set`, `delete`), `policies` (`list`, `set-acme-dns`, `clear`), `certs` (`list`, `upload-manual`, `delete`, `csr begin/get/upload-cert/cancel`, `issue-acme-dns`, `retry`), `config` (`get`, `set-contact`, `set-profile`), `hostnames`, `attempts`, `retry-blocks` (`list`, `set`, `clear`).
- **`backups`** — `apps` (`register`/`deregister`/`list`), `strategies` (`create`/`list`/`show`/`update`/`delete`), `run`, `snapshots list`, `restore`.
- **`images`** — `list`, `pull`, `remove`, `discover`, `pins list/clear`.
- **`registries`** — `list`, `add`, `remove`.
- **`templates`** — `list`, `show`, `create`, `update`, `remove`, `preview`, `instantiate`.
- **`shells`** — `list`, `stop` (matches the Shells panel on the apps page).
- **`forwards`** — `list`, `stop` (the server-side counterpart to `apps forward`; see below).
- **`user`** — `list`, `add`, `remove` (operator key management).
- **`faults`**, **`clear-faults <app>`**, **`status`** — top-level utilities.
- **`events`** — streams the daemon's event feed as JSON to stdout; pair with `jq`.
- **`proxy logs`** / **`dns logs`** — same log streamer as the infra logs page in the UI.

Default output is pretty-printed JSON; pipe through `jq` for filtering.

### Port forwarding

`seedling-ctl apps forward <app> <service> <port>` opens a local listener that tunnels through the daemon to the named service. Defaults: TCP, `[::1]:<auto>`. Override with `--proto udp` or `--local-port <n>`.

While the forward is running, throughput stats are written to stderr; Ctrl+C tears it down. From another shell, `seedling-ctl forwards list` shows every active forward and `seedling-ctl forwards stop <id>` kills one server-side. The web UI surfaces the same list under "Port forwards" on the apps page.

This is the operator's escape hatch for poking at services that aren't exposed via an ingress — databases, admin endpoints, debug ports.
