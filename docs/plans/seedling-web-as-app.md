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
2. **Authorisation gate lives entirely inside the daemon.** The OI's
   `external_service_mappings` surface gains a new target kind that
   resolves to the daemon's own OI listener, but mappings of that kind
   are not creatable through any operator-facing path: `services/external/map`
   rejects `target_kind=oi` unconditionally, and the web UI / CLI don't
   offer it. The only path that creates such a row is the daemon's
   internal "instantiate a built-in template" hook, which inserts the
   mapping (and the matching authorized-key entry, and stages a client
   keypair) atomically with the app registration.
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
5. **There is no bespoke bootstrap CLI.** The operator-facing flow is just
   the standard surfaces: `templates/instantiate` to create the
   `seedling-web` app from the built-in template, then `apps/install/invoke`
   when ready, then `ingresses site attach` to give the web UI a hostname.
   Everything else — keypair generation, key authorisation, OI mapping
   creation, client-key delivery into the install action — is performed
   by the daemon as a side effect of instantiating a built-in template
   that declares `external_service("oi")`.
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
   name `seedling-web` is reserved: the only path that creates it is
   `templates/instantiate` against the built-in template under the same
   name (any other app name on that instantiate call is rejected, and
   `apps/create` rejects the name unconditionally). No postinst hook
   required.
9. **Key rotation for v1 = deregister + re-instantiate.** No standalone
   rotate-key action. The deregister path revokes the keypair as part of
   its cleanup; re-instantiating the built-in template generates a fresh
   one.

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
    └─ "oi" → fd**::oi:7891 (daemon-internal mapping, no operator path)

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
- Insertion via the OI: rejected unconditionally for `target_kind=oi`. No
  flag, no escape hatch on the operator-facing surface.
- Insertion via the daemon's internal `instantiate_builtin_template` path:
  permitted, and performed automatically when the AppDef declares an
  `external_service("oi")` slot. Idempotent: re-instantiation does not
  duplicate the row.
- Listing: `services external list` and the web UI surface oi-targeted
  mappings with a visible "OI listener" tag and an indication that the
  row is daemon-managed (i.e. operators can read but not edit it).
- Reconciler resolution: when the desired-state computation encounters a
  pod-side mount of an `external_service("oi")` slot whose mapping has
  `target_kind=oi`, the resolved endpoint is the OI listener's bridge
  address and the OI port. The pod's network namespace gets a route to
  that address through its own existing gateway.

Removal of an oi mapping is similarly daemon-internal: the deregister
hook for a built-in-template-derived app deletes its mapping as part of
teardown.

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

#### Side effects of instantiating a built-in template

The apps table grows a nullable `builtin_source TEXT` column naming the
built-in template the app was instantiated from (NULL for ordinary apps).
When `templates/instantiate` runs against a row with `is_builtin=1`, the
daemon performs the following side effects atomically with app
registration, in addition to the standard registration:

1. Generates an ed25519 client keypair (using the same code path as
   `ClientIdentity::load_or_generate`).
2. Authorises the public fingerprint in `authorized_keys`, with a label
   like `<app> (built-in)`. The row is tagged so deregister can remove
   it.
3. Stores the encrypted private key under the operation's secret-param
   storage, keyed by the synthetic install-param name `client-key`.
   When `apps/install/invoke` later runs, the daemon merges this key
   into the operator-supplied params before validation. The key never
   passes through the OI on the wire.
4. For each `external_service` slot the AppDef declares whose name is
   `"oi"`, inserts an `external_service_mappings` row with
   `target_kind=oi`. This is the only path that creates such rows.

On `apps/remove` of an app whose `builtin_source` is non-NULL, the
daemon's deregister hook performs the matching teardown: revoke the
authorized-key entry, delete the synthetic client-key secret, delete the
oi mapping(s).

Re-instantiation under the same name (after a previous deregister)
generates a new keypair and a new mapping; nothing leaks across
generations.

Script sketch (full body lives at the path above):

```rhai
app.description("Seedling web UI / WebTransport gateway");

let password_hash = app.param("password-hash")
    .kind("password")
    .required(false)
    .description("Argon2id-hashed operator password; required unless trust-tailscale-headers is true");

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
    // The daemon synthesised the `client-key` install param at template-
    // instantiation time and authorised the public half then. Write the
    // private half into the state volume and start the deployment.
    state.write("/web.key", param["client-key"]);
    rt.start(app).ready(60);
}, #{
    requirements: #{
        "client-key": #{ kind: "password", required: true,
            description: "PEM-encoded ed25519 client key (synthesised by the daemon at template-instantiation time; never operator-supplied)" },
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

### Operator flow

The operator-facing surface is the standard apps / templates / ingresses
machinery. No new CLI subcommands or web UI affordances are required for
v1 beyond what already exists; the plan adds zero operator-specific
ergonomics.

A typical bring-up sequence:

1. `seedling-ctl templates list` — sees `seedling-web (built-in)`.
2. `seedling-ctl templates instantiate --template seedling-web --app
   seedling-web` (constraint above forces this app name). Registers the
   app in `NotInstalled` state. Daemon performs the side effects above.
3. (If using password auth) `seedling-ctl apps params set seedling-web
   password-hash <argon2id-hash>`. The argon2id-hashing step is operator-
   side; we either ship a small `seedling-ctl util hash-password` helper
   (out of scope for this plan) or document the openssl/argon2 cli path.
4. `seedling-ctl apps install seedling-web`. The daemon attaches the
   synthesised `client-key` install param transparently before invoking
   the install action. Deployment comes up.
5. `seedling-ctl ingresses site attach <site-ingress> --port 443
   --protocol https --to seedling-web/web`. For example, attaching the
   discovered Tailscale site ingress (per the site-ingresses plan)
   exposes the web UI on the host's `.ts.net` hostname with TLS supplied
   by tailscaled.

Web UI users walk the same surfaces in their respective views.

Tear-down: `seedling-ctl apps remove seedling-web`. The daemon revokes
the keypair, removes the oi mapping, and runs the standard graceful
deregister sequence.

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

- **Operator browser → seedling-web**. The seedling-web BSL script
  declares an exported service rather than a hard-coded ingress, so the
  operator wires the front door themselves via a site ingress
  attachment (e.g. the discovered Tailscale site ingress, or a manual
  one with ACME-DNS). Standard site-ingresses machinery from the
  `site-ingresses` plan; nothing seedling-web-specific needed.
- **seedling-web → OI**. The `external_service("oi")` slot, resolved by
  the reconciler to the new bridge address. The mapping was inserted by
  the daemon at template-instantiation time; the operator never sees or
  edits it.

## Threat-model addendum

`docs/threat-model.md` gets a new note under "What we do not defend
against": an authenticated workload reached via `external_service("oi")`
is operator-equivalent, in the same sense as N1 (an authenticated
operator). The gate is "instantiation of a built-in template that
declares the slot"; built-in templates are owned by the daemon binary,
so the trust delegation flows from "the operator runs this version of
seedling" to "the workload it runs has OI access". The OI surface for
`services/external/map` rejects `target_kind=oi` unconditionally, so an
operator cannot grant OI access to an arbitrary app via that route.

The audit log already attributes every OI request to an actor;
seedling-web will continue to populate `actor.kind = "web"` for human-
driven requests proxied through it, and `actor.kind = "ctl"` synthesis
still applies to the seedling-web binary's own infrequent calls to
`/server/ping` etc.

## Spec changes

- `docs/spec/runtime.md`:
  - Add `r[service.external.mapping.oi]`: the OI target kind, the
    rejection of operator-driven creation, the daemon-internal-only
    insert path, and the resolution to the bridge listen address.
  - Add `r[template.builtin]`: built-in templates are owned by the
    daemon binary, refreshed at startup, immutable through the OI;
    instantiation triggers the side-effect set described above
    (keypair, authorisation, oi mapping, synthesised install param).
    Constrain `instantiate` so a built-in template must be instantiated
    under the template's own name.
  - Reserve the `seedling-web` app name (`r[app.name.reserved]`).
  - Clarify [generation.bumps](runtime.md#r--generation.bumps) so a
    script update whose body is byte-identical but whose re-evaluation
    produces a different AppDef counts as a successful update.
- `docs/spec/interface.md`:
  - Extend `services/external/map` request shape with the new
    `target_kind=oi` and the rejection rule.
  - Extend the templates section: `is_builtin` field on
    `templates/list` / `templates/show`, rejection rules on `update` /
    `remove`, and the `instantiate` constraint above. Cross-link to
    `r[template.builtin]`.
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
   migration, OI rejection of `target_kind=oi`, daemon-internal insert
   path, listing surface, reconciler resolution. The reserved BSL name
   "oi" gets carved out in the name validator. End of phase: a
   hand-rolled BSL script can declare `external_service("oi")`, but no
   external surface can give it a binding.
3. **BSL constants.** `SEEDLING_VERSION`, `SEEDLING_IMAGE`,
   `SEEDLING_OI_FINGERPRINT`. Trivial; gates phase 5 because the embedded
   script needs them.
4. **Built-in template machinery + `apps/update` constant-aware bump.**
   Add `is_builtin` to the templates table; reject operator-driven
   update / remove of built-in rows; constrain `instantiate` so a
   built-in template's app name matches the template name. Reserve the
   `seedling-web` app name. Add the `apps.builtin_source` column. Wire
   the side-effect set into the templates/instantiate handler:
   keypair generation, authorized-key insert, oi mapping insert,
   synthesised install-param storage. Wire the matching teardown into
   `apps/remove`. Confirm (and amend if needed) the runtime spec rule
   that `apps/update` bumps the generation when re-evaluation diverges
   even if the script content is byte-identical.
5. **Embedded seedling-web script.** Add the BSL script source at
   `crates/core/src/runtime/builtin_templates/seedling-web.rhai` and the
   startup upsert + reserved-name self-update path in `seedlingd`. End
   of phase: an operator can run `seedling-ctl templates instantiate`,
   `apps install`, `ingresses site attach` against the standard surfaces
   and end up with a working in-cluster web UI.
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
- **Synthesised install-param delivery shape.** The plan stores the
  daemon-generated client key as a synthetic install param the daemon
  merges in at `apps/install/invoke` time. Alternative shapes: a runtime-
  scoped param-binding analogous to operation-scoped volume bindings, or
  a `rt.*` builder that exposes the key directly to the install closure
  without it being a param at all. Choose during phase 4.
- **Operator UX for setting the password hash.** The operator needs to
  produce an Argon2id hash with the same parameters seedling-web uses.
  Options: ship a `seedling-ctl util hash-password` helper, document the
  external CLI path, or extend BSL param kinds with an
  argon2id-on-store variant. Out of scope for this plan but worth
  flagging.
- **Healthcheck approach.** Adding `--health-probe` to seedling-web vs
  dropping the healthcheck and relying on container-running. Either is
  fine; dropping is cheaper.
- **Whether `oi.address()` / `oi.port()` need to be expressible in BSL**
  vs assuming a fixed convention. The latter is simpler but couples the
  script to the daemon's address allocation. To revisit during phase 2.
- **Daemon → seedling-web visibility.** In-cluster, the daemon knows
  about seedling-web (it's a registered app). Consider whether
  `seedling-ctl status` should call out the seedling-web app
  specifically, or whether it reads as just another registered app.
- **Co-existence with a standalone seedling-web.** During the migration
  window, an operator may have both a systemd-managed seedling-web and
  an in-cluster one. Both contend for HTTP/WT ports. Document the
  migration step (stop the systemd unit before instantiating, or use a
  different hostname for the in-cluster instance).

## Critical files to touch

- `crates/core/src/oi/server.rs` — additional listen address from
  bridge-derived config.
- `crates/core/src/system/data_plane/...` — bridge ULA allocation for
  the OI listener (if not already a side-effect of the existing
  bridge plumbing).
- `crates/core/src/runtime/external_service_mappings.rs` — new target
  kind + migration; daemon-internal insert/delete helpers.
- `crates/core/src/oi/handler/services.rs` (or wherever external service
  mappings are handled) — reject `target_kind=oi` on the OI surface.
- `crates/core/src/runtime/registry/...` — reconciler resolution of the
  oi target kind to the listener address.
- `crates/core/src/defs/...` (BSL constants) — `SEEDLING_VERSION`,
  `SEEDLING_IMAGE`, `SEEDLING_OI_FINGERPRINT`.
- `crates/core/src/defs/app/...` — name validator carve-out for `"oi"`;
  reserved app-name list for `"seedling-web"`.
- `crates/core/src/runtime/templates.rs` — `is_builtin` column +
  migration; reject mutate/remove on built-in rows; constrain
  `instantiate` for built-in templates; instantiate side-effects
  (keypair, key authorisation, oi mapping, synthesised install param).
- `crates/core/src/runtime/apps.rs` — `builtin_source` column;
  deregister hook that runs the matching teardown for built-in-derived
  apps; bump the generation when re-evaluation diverges with
  byte-identical script content.
- `crates/core/src/runtime/builtin_templates/seedling-web.rhai` — the
  embedded BSL script.
- `crates/core/src/runtime/builtin_templates.rs` — startup upsert of the
  built-in template + self-update path on the reserved app name.
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
