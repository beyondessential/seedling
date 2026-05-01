# seedling-web as a seedling-managed app

## Context

`seedling-web` today is a separate process the operator runs alongside
`seedlingd` (typically as its own systemd unit). It connects to the daemon's
QUIC OI listener as a normal mTLS client — its key fingerprint is added to the
daemon's `authorized_keys` table — and proxies WebTransport from browsers into
the OI.

Running it instead as a seedling-managed app gives us:

- **One supervised entity.** The daemon already knows how to start, observe,
  restart, and roll a container deployment. Today's "is the seedling-web
  systemd unit alive?" question lives outside the runtime.
- **Version lock by construction.** The binary in the container is the same
  binary the package ships, so the SPA bundle (compiled into the binary) and
  the daemon's OI surface stay aligned across upgrades.
- **Operator surface uniformity.** Logs, faults, scheduling, ingress wiring,
  cert acquisition for the web UI's hostname — all flow through the same
  surfaces apps already use, instead of being a separate set of operator
  concerns.

The non-trivial parts are reaching the OI from inside a pod network and
bootstrapping the mTLS authorisation without a chicken-and-egg loop.

## Locked-in decisions

These were settled during the design conversation that opened this plan.
Anything in this section is treated as a constraint by the rest of the
document.

1. **`external_service` reserved name `"oi"`.** Apps signal "I want OI access"
   by declaring `app.external_service("oi")`. The 2-character name falls
   outside the normal `bsl.name` validator (3+ chars), so the slot is
   structurally unmistakable. The name is a hard reservation: any app may
   declare it; it has no effect unless the operator wires the matching mapping.
2. **Authorisation gate lives on the OI mapping, not in BSL.** The OI's
   `external_service_mappings` surface gains a new target kind that resolves
   to the daemon's own OI listener. Inserting a mapping with that kind is
   rejected unless the request body carries `dangerously_allow_oi_access:
   true`. Standard `seedling-ctl services external map` and the web UI never
   surface that flag; the only path that passes it is the dedicated bootstrap
   command.
3. **No scoped permission model for v1.** A workload that successfully wires
   up OI access has full operator-equivalent privilege. The threat-model
   addition is "this workload is now in the operator trust boundary", not
   "we have container-scoped permissions". Acceptable because the only
   blessed user is seedling-web itself, which is intentionally
   operator-equivalent.
4. **The BSL script lives in the daemon binary as a built-in template.**
   `seedlingd` `include_str!`s the script and registers it as a built-in
   entry in the templates table on first startup; the daemon refreshes the
   stored body from the embedded source on every subsequent startup, so a
   daemon upgrade automatically propagates new template content. Built-in
   templates cannot be removed or edited via OI. The template is visible
   in `templates/list` but does not occupy a row in `apps/list` until
   instantiated. Operators who don't run seedling-web inside seedling
   simply don't instantiate it.
5. **Bootstrap is a dedicated `seedling-ctl seedling-web` subcommand**, not
   the generic install path. It instantiates the built-in template into an
   app named `seedling-web`, generates the mTLS keypair, authorises the
   public half, creates the OI mapping with the dangerous flag, and
   invokes install with the private key as a secret param.
6. **Container image is the published seedling image at
   `ghcr.io/<repo>/seedling:<version>`.** ghcr.io is already in the default
   registry allowlist. The image carries the seedling-web binary (and any
   shared libs, an entrypoint suitable for invoking each seedling
   subprogram, and so on); the deployment runs `seedling-web` directly out
   of the image. Version lock comes from interpolating
   `SEEDLING_VERSION` (or `SEEDLING_IMAGE`) into the image ref, so a daemon
   upgrade re-evaluates the script with a new tag, bumps the generation,
   and rolls the deployment. Publishing the seedling image is being pursued
   independently for unrelated reasons; this plan assumes that work has
   landed.
7. **OI binds an additional address for in-cluster use.** A fresh ULA on the
   existing seedling-proxy bridge (the same bridge `r--tls.cert.serve` already
   uses) — not loopback, not a brand-new bridge. The reconciler resolves a
   bound `external_service("oi")` slot to that address.
8. **Auto-update via daemon-startup self-update on the reserved name.**
   When the daemon starts, after refreshing the built-in template's body,
   it checks for an app named `seedling-web`. If one exists, the daemon
   issues a self-`apps/update` with the embedded script content. The app
   name `seedling-web` is reserved: only the bootstrap command can create
   it, and only by instantiating the built-in template under the same
   name (instantiation under any other app name is rejected). No
   postinst hook required.
9. **Key rotation for v1 = re-bootstrap.** No standalone rotate-key action.
   Re-running `seedling-ctl seedling-web bootstrap` deregisters and
   re-installs with a fresh key.

## Architecture

```
host (apt-managed)
├─ /usr/bin/seedling, seedling-ctl                  binaries
├─ /lib/systemd/system/seedling.service
└─ /var/lib/seedling/                                data dir
    ├─ oi.key, …
    └─ db.sqlite

seedlingd (host process, root)
├─ OI QUIC listener
│   ├─ [::1]:7891             default loopback
│   └─ [fd**::oi]:7891        new ULA on the seedling-proxy bridge
└─ External-service resolver
    └─ "oi" → fd**::oi:7891 (with dangerously_allow_oi_access)

seedling-web app (managed deployment)
└─ container (ghcr.io/<repo>/seedling:<SEEDLING_VERSION>)
    ├─ mount  /var/lib/seedling-web  ← managed app volume "state"
    ├─ env    SEEDLING_DAEMON_FINGERPRINT=<fp>
    └─ command seedling-web
                    --daemon-addr  [<oi external_service address>]:7891
                    --daemon-fingerprint  $SEEDLING_DAEMON_FINGERPRINT
                    --key-file     /var/lib/seedling-web/web.key
                    --listen       [::]:7894
                    --listen       [::]:7893
```

The OI external_service slot resolves at reconcile time to the daemon's new
ULA address, exposed into the pod network through the same machinery that
already makes Caddy reachable to the cert-serving endpoint.

## Components

### OI listener: new bridge address

`crates/core/src/oi/server.rs` already accepts a list of addresses
(`r--transport.listen`). The daemon grows a config-derived address on the
seedling-proxy bridge and adds it to the listener set.

The address itself comes from the data-plane allocation logic that already
deals with bridge ULAs. The same key, fingerprint, and `authorized_keys`
table govern this listener — it's literally another listen address, not a
separate authentication domain.

The default allocation gives the daemon a stable address (e.g.
`fd**::oi`) so the BSL script can name it via the resolved external service
slot rather than re-deriving it.

### `external_service_mappings`: OI target kind

`crates/core/src/runtime/external_service_mappings.rs` today has target kinds
`app-service` and `site-service`. Add `oi`:

- DB schema: a new `target_kind` enum value. Existing rows are unaffected.
- Insertion validation: a row whose target_kind is `oi` is rejected unless
  the OI request body that created it included `dangerously_allow_oi_access:
  true`.
- Listing: `services external list` and the web UI surface oi-targeted
  mappings with a visible "OI listener (UNSAFE)" tag, so an operator
  reviewing mappings sees them.
- Reconciler resolution: when the desired-state computation encounters a
  pod-side mount of an `external_service("oi")` slot whose mapping has
  `target_kind=oi`, the resolved endpoint is the OI listener's bridge
  address and the OI port. The pod's network namespace gets a route to
  that address through its own existing gateway.

The standard `services external map` CLI / OI handler does not accept an
`oi` target kind — it returns an error pointing at the bootstrap command.
Only the bootstrap command's call path passes the dangerous flag.

### BSL constants

Three new constants exposed to BSL during script evaluation
(`crates/core/src/runtime/...`, alongside the existing `AVAILABLE_MEMORY`
etc):

- `SEEDLING_VERSION` — the daemon's version string. Used to interpolate
  the seedling image tag and useful beyond seedling-web (any app that
  wants to encode a daemon-version assumption).
- `SEEDLING_IMAGE` — the canonical full image ref the daemon was built
  to use, e.g. `ghcr.io/<repo>/seedling:0.5.2`. Strictly redundant with
  `SEEDLING_VERSION` plus a known registry path, but lets the script stay
  agnostic to where seedling images are published. The seedling-web
  script uses this directly.
- `SEEDLING_OI_FINGERPRINT` — the daemon's own SPKI fingerprint, equal to
  what `r--transport.server-identity` produces. Lets the script pin the
  daemon without an out-of-band parameter.

These are evaluated per script run, like the existing constants. They
flow through the standard generation-bump pathway: a daemon upgrade
changes `SEEDLING_VERSION` / `SEEDLING_IMAGE`, the AppDef changes,
generations bump, the deployment rolls.

### Built-in seedling-web template

The script lives in the source tree at
`crates/core/src/runtime/builtin_templates/seedling-web.rhai` and is
`include_str!`d into `seedlingd`. Not shipped as a separate file on disk.

The templates table grows an `is_builtin BOOLEAN NOT NULL DEFAULT 0`
column (new migration). On daemon startup:

1. The seedling-web template row is upserted: its body is replaced with
   the embedded source on every run, `is_builtin=1`, `description` set to
   a fixed string. Operators may not modify or remove this row via the
   OI: `templates/update` and `templates/remove` reject `is_builtin=1`
   rows with `requirements_invalid`.
2. The reserved app-name list grows an entry for `"seedling-web"`. The
   `apps/create` handler rejects this name unconditionally; the only
   creation path is through the built-in template via
   `templates/instantiate`. Additionally, `templates/instantiate { template:
   "seedling-web", app: <name> }` is constrained to require `app` ==
   `"seedling-web"` so the operator can't fork the blessed slot under a
   different name.
3. If a `seedling-web` app already exists, the daemon issues a self-call
   to `apps/update` with the embedded script body. This is the daemon-
   startup self-update path. Idempotent when the body matches.

Script sketch (full body lives at the path above):

```rhai
app.description("Seedling web UI / WebTransport gateway");

let password_hash = app.param("password-hash")
    .kind("password")
    .required(true)
    .description("Argon2id-hashed operator password (use `seedling-ctl seedling-web set-password` to set)");

let session_lifetime = app.param("session-lifetime-secs")
    .required(false)
    .default_value("86400")
    .description("Web session token lifetime in seconds");

let trust_tailscale = app.param("trust-tailscale-headers")
    .required(false)
    .default_value("false")
    .description("Trust Tailscale identity headers when fronted by Tailscale Serve");

let state = app.volume("state")
    .description("Persistent web client key + session state");

let oi = app.external_service("oi");

let svc = app.service("web")
    .description("Plain HTTP + WebTransport ports for the seedling web UI")
    .exported(#{ description: "Seedling web UI" });

app.deployment("web")
    .description("Seedling web UI / WebTransport gateway")
    .image(SEEDLING_IMAGE)
    .mount("/var/lib/seedling-web", state)
    .mount_service(oi)
    .env("SEEDLING_DAEMON_FINGERPRINT", SEEDLING_OI_FINGERPRINT)
    .env("SEEDLING_WEB_LOG", "info,seedling_web=debug")
    .command("seedling-web")
    .arg([
        "--daemon-addr", `[${oi.address()}]:${oi.port()}`,
        "--daemon-fingerprint", SEEDLING_OI_FINGERPRINT,
        "--key-file", "/var/lib/seedling-web/web.key",
        "--listen", "[::]:7894",
        "--listen", "[::]:7893",
    ])
    .tcp(7894, svc.port(80).http())
    .udp(7893, svc.port(443))
    .stop_signal("SIGTERM")
    .healthcheck(#{
        kind: "command",
        cmd: ["seedling-web", "--health-probe"],
        interval: 10, retries: 3, start_period: 10, on_failure: "replace",
    });

app.on_install(|rt, param| {
    // Seed the web client key from the install param and let the deployment
    // start. The bootstrap command authorised the public half before
    // invoking install, so the very first connect attempt will succeed.
    state.write("/web.key", param["client-key"]);
    rt.start(app).ready(60);
}, #{
    requirements: #{
        "client-key": #{ kind: "password", required: true,
            description: "PEM-encoded ed25519 client key (set by the bootstrap command)" },
    },
});
```

Notes on this sketch:

- `oi.address()` / `oi.port()` reflect the pre-existing
  `mount_service` resolution at deploy time. Today's API is
  `pod.mount(svc: ServicePort)`; we need the host-name and port to be
  available at script evaluation time so they can be interpolated into the
  command line. This may need a small BSL surface addition; alternative is
  a fixed convention (the OI bridge address is always `[<bridge-gateway>]`
  and the port is always 7891). To revisit during phase 2.
- `--health-probe` is a new no-op subcommand on seedling-web that exits 0
  if the binary can parse args; cheap to add and avoids needing curl in the
  image. Alternative: drop the healthcheck entirely and rely on the
  service-level "WT cert in `/connect` response" check.

### Bootstrap CLI: `seedling-ctl seedling-web`

A new top-level subcommand group (`crates/ctl/src/main.rs` +
`crates/ctl/src/seedling_web/mod.rs`):

```
seedling-ctl seedling-web bootstrap
    [--password-prompt]
    [--hostname <host>]      # for the site ingress; optional
seedling-ctl seedling-web set-password
```

`bootstrap` steps:

1. `templates/instantiate { template: "seedling-web", app: "seedling-web" }`
   — registers the AppDef from the daemon's built-in template.
2. Generate an ed25519 keypair using the same code path as
   `ClientIdentity::load_or_generate` (`crates/protocol/src/keys.rs`).
3. `keys/authorize { fingerprint, label: "seedling-web (in-app)" }` for the
   public half.
4. `services/external/map { app: "seedling-web", external_name: "oi",
   target_kind: "oi", dangerously_allow_oi_access: true }`.
5. (Optional, if `--hostname` given) create / update a manual site ingress
   for the hostname and attach it as a forward to `(seedling-web, web)`.
6. If `--password-prompt`, prompt for the operator password, hash it with
   the same Argon2id parameters seedling-web uses today, and set the
   `password-hash` param via `apps/params/set`.
7. `apps/install/invoke { app: "seedling-web", params: { client-key:
   <PEM> } }`.

The command is idempotent on re-run: each step short-circuits when its
target already exists, except step 2 (key generation) which is skipped
when the existing client key is still authorised.

`set-password` re-prompts and updates the `password-hash` param. Triggers
the `on_change` handler if the script declares one (otherwise a manual
restart is needed; mirror the postgres password-rotation pattern).

Key rotation in v1 is "re-run `bootstrap`"; no standalone subcommand.

### Auto-update: daemon-startup self-update on the reserved name

After upserting the built-in template at startup, `seedlingd` checks for
the existence of an app named `seedling-web`. If present, it issues a
self-call to `apps/update { app: "seedling-web", script: <embedded body>
}`. The actor on the call is `kind: "system"`, `id: "builtin-template",
display: "seedlingd self-update"` so the audit log distinguishes
self-updates from operator-driven changes.

When the daemon upgrades, the embedded script's constants
(`SEEDLING_VERSION` / `SEEDLING_IMAGE`) interpolate to new values; the
re-evaluated AppDef differs from the prior generation; the runtime bumps
the generation and the deployment rolls. The pathway is the standard
generation / reconcile machinery — the only daemon-side special-case is
the existence check on `"seedling-web"`.

A subtle point: today, `apps/update` re-evaluates the script and bumps
the generation per `r--generation.bumps`. The current spec text reads
"successful script update", which is silent on whether a byte-identical
script with diverging re-evaluation should still bump. This needs
clarification — see Open questions.

### Container image

The image is the published seedling image at
`ghcr.io/<repo>/seedling:<SEEDLING_VERSION>`, owned by the seedling repo's
release pipeline. It is shared with at least one other plan that needs the
same image; this plan does not specify the image's contents beyond
"contains the seedling-web binary, callable as `seedling-web`".

Image-related concerns that this plan therefore does not address:

- Base distro / libc choice: a property of the image, not of seedling-web.
- Multi-arch publishing: the image must publish at least the architectures
  seedling itself supports.
- Registry credentials: ghcr.io public images need none; private images
  are out of scope.

### TLS / WT cert

seedling-web's WebTransport endpoint uses a self-signed ECDSA-P256 cert
that rotates every <14 days (`w--wt.cert`). Today the cert is kept
in-memory; the rotation thread regenerates as needed. In-cluster:

- Cert stays in-memory inside the container.
- A container restart loses the cert; the next browser connection calls
  `POST /connect` and re-pins via `cert_hashes`. Same path as today's
  systemd-restart-of-seedling-web flow.
- No new persistence needed.

### Network reach for the web UI itself

Two directions of traffic:

- **Operator browser → seedling-web**. Needs to reach the container's
  HTTP + WT ports. Wired via standard BSL `service.ingress(...)` and a
  hostname provided by the operator (manual site ingress + attachment, or
  baked into the script via params).
- **seedling-web → OI**. The `external_service("oi")` slot, resolved by
  the reconciler to the new bridge address.

The browser-facing side is plain seedling app territory and doesn't need
new mechanisms. The bootstrap command's optional `--hostname` flag is a
convenience that wires the site ingress at install time.

## Threat-model addendum

`docs/threat-model.md` gets a new note under "What we do not defend
against": an authenticated workload reached via `external_service("oi")`
with `dangerously_allow_oi_access` is operator-equivalent, in the same
sense as N1 (an authenticated operator). The bootstrap command is the
single intended path; the gate is the explicit dangerous flag, which the
default surfaces don't expose.

The audit log already attributes every OI request to an actor; seedling-web
will continue to populate `actor.kind = "web"` for human-driven requests
proxied through it, and `actor.kind = "ctl"` synthesis still applies to
the seedling-web binary's own infrequent calls to `/server/ping` etc.

## Spec changes

- `docs/spec/runtime.md` — add `r[service.external.mapping.oi]`: the
  semantics of the OI target kind, the dangerous-flag gate, the resolution
  to the bridge listen address. Also clarify
  [generation.bumps](runtime.md#r--generation.bumps) so a script update
  whose body is byte-identical but whose re-evaluation produces a
  different AppDef counts as a successful update.
- `docs/spec/interface.md` — extend `services/external/map` request shape
  with the new target kind and the gating rule. Add the
  built-in-template concept to the templates section: `is_builtin` flag
  in `templates/list` / `templates/show`, rejection rules on `update` /
  `remove`, and the `instantiate` constraint that built-in templates
  must be instantiated under the template's own name. Reserve the
  `seedling-web` app name. Cross-link the threat-model line about
  operator-equivalent authority.
- `docs/spec/web.md` — no changes to the program's behaviour; the spec
  describes the binary, which is the same binary running in or out of a
  container.
- `docs/threat-model.md` — addendum noted above.

## Phasing

1. **OI listener: bridge address.** Plumb a new ULA on the existing
   seedling-proxy bridge into the listener address set. No BSL or
   external-service changes yet — verifiable by `seedling-ctl --addr
   [fd**::oi]:7891 server ping` from the daemon's own pod-network namespace
   (or just from the host).
2. **`external_service_mappings`: OI target kind.** New target_kind, new
   migration, gating rule, listing surface, reconciler resolution. The
   reserved BSL name "oi" gets carved out in the name validator. End of
   phase: a hand-rolled BSL script can declare `external_service("oi")`,
   the operator can map it via a raw OI request, and a container in that
   pod can `nc` the OI port.
3. **BSL constants.** `SEEDLING_VERSION`, `SEEDLING_IMAGE`,
   `SEEDLING_OI_FINGERPRINT`. Trivial; gates phase 5 because the embedded
   script needs them.
4. **Built-in template machinery + `apps/update` constant-aware bump.**
   Add `is_builtin` to the templates table; reject operator-driven
   update / remove of built-in rows; constrain `instantiate` so a
   built-in template's app name matches the template name. Reserve the
   `seedling-web` app name. Confirm (and amend if needed) the runtime
   spec rule that `apps/update` bumps the generation when re-evaluation
   diverges from the current AppDef even if the script content is
   byte-identical. Add the BSL script source at
   `crates/core/src/runtime/builtin_templates/seedling-web.rhai` and the
   startup upsert + self-update path in `seedlingd`.
5. **Bootstrap subcommand.** Stitches steps 2–4 together end-to-end. End
   of phase: `seedling-ctl seedling-web bootstrap` brings up a working
   in-cluster web UI.
6. **Packaging.** `.deb` (and a parallel rpm if/when relevant) places
   binaries under `/usr/bin/` and the systemd unit under
   `/lib/systemd/system/`. No postinst hook required (the daemon's
   startup self-update handles in-place upgrades). Migrate the existing
   dev path that runs seedling-web standalone to use the in-cluster
   path on packaged installs; standalone stays available for
   development.

## Open questions

- **`apps/update` semantics for byte-identical scripts.** The auto-update
  flow assumes that re-evaluating an unchanged script with new ambient
  constants produces a new generation when the AppDef diverges. The
  runtime spec is silent on this case — needs clarification or a small
  amendment in phase 4.
- **Healthcheck approach.** Adding `--health-probe` to seedling-web vs
  dropping the healthcheck and relying on container-running. Either is
  fine; dropping is cheaper.
- **Whether `oi.address()` / `oi.port()` need to be expressible in BSL**
  vs assuming a fixed convention. The latter is simpler but couples the
  script to the daemon's address allocation. To revisit during phase 2.
- **What should `seedling-web set-password` look like.** Today the password
  hash is in a TOML config; as an app it's a (secret) param. The
  set-password command becomes `apps/params/set` underneath. Decide whether
  to keep a dedicated subcommand or expose it through the standard
  parameter surface.
- **Daemon → seedling-web visibility.** Today's standalone deployment has
  the daemon agnostic to seedling-web's existence. In-cluster, the daemon
  knows about it (it's a registered app). Consider whether
  `seedling-ctl status` should call out the seedling-web app specifically,
  or whether it reads as just another registered app.
- **Co-existence with a standalone seedling-web.** During the migration
  window, an operator may have both a systemd-managed seedling-web and an
  in-cluster one. Both contend for HTTP/WT ports. Document the migration
  step (stop the systemd unit before bootstrap, or use a different
  hostname for the in-cluster instance).

## Critical files to touch

- `crates/core/src/oi/server.rs` — additional listen address from
  bridge-derived config.
- `crates/core/src/system/data_plane/...` — bridge ULA allocation for
  the OI listener (if not already a side-effect of the existing
  bridge plumbing).
- `crates/core/src/runtime/external_service_mappings.rs` — new target
  kind + migration.
- `crates/core/src/oi/handler/services.rs` (or wherever external service
  mappings are handled) — request-shape extension, dangerous-flag gate.
- `crates/core/src/runtime/registry/...` — reconciler resolution of the
  oi target kind to the listener address.
- `crates/core/src/defs/...` (BSL constants) — `SEEDLING_VERSION`,
  `SEEDLING_IMAGE`, `SEEDLING_OI_FINGERPRINT`.
- `crates/core/src/defs/app/...` — name validator carve-out for `"oi"`;
  reserved app-name list for `"seedling-web"`.
- `crates/core/src/runtime/templates.rs` — `is_builtin` column +
  migration; reject mutate/remove on built-in rows; constrain
  `instantiate` for built-in templates.
- `crates/core/src/runtime/builtin_templates/seedling-web.rhai` — the
  embedded BSL script.
- `crates/core/src/runtime/builtin_templates.rs` — startup upsert of the
  built-in template + self-update path on the reserved app name.
- `crates/core/src/runtime/apps.rs` (or wherever `apps/update` lives) —
  bump the generation when re-evaluation diverges, even with identical
  script content.
- `crates/ctl/src/main.rs` + `crates/ctl/src/seedling_web/mod.rs` —
  bootstrap and set-password subcommands.
- `crates/web/src/main.rs` — `--health-probe` (if we keep the healthcheck).
- `docs/spec/runtime.md`, `docs/spec/interface.md`,
  `docs/threat-model.md` — spec deltas.
- Packaging (deb / rpm specs, eventually) — install layout under
  `/usr/bin`, `/lib/systemd/system`. No postinst hook.

## What stays unchanged

- `seedling-web`'s actual program: same binary, same CLI flags, same SPA
  bundle. The host-process deployment mode keeps working unchanged for
  development.
- `crates/web/src/*`: zero source changes for v1, modulo the optional
  `--health-probe` subcommand.
- The OI's authentication and authorization model: a key in
  `authorized_keys` is operator-equivalent, just as it is today. The only
  new wrinkle is "an additional listen address" and "a way for a workload
  to reach that address through an `external_service` slot".
- Frontend bundling: the SPA stays embedded in the seedling-web binary.
  Bind-mounting the binary into the container brings the SPA along for free.
