# J1: Custom error pages served from the container

## Status: blocked on Q1 for the branded half

The card as written ports a Caddyfile fragment from the Tamanu Linux config. Investigation
found that fragment does not work anywhere, and that the error page HTML exists in no artefact
Seedling can reach. The branded page needs the Tamanu definition to carry the pages, which is
possible in today's single-script format as string literals and pleasant only once Q1 lands a
definition shape that can hold sidecar files. Q1 is therefore a dependency on shape, not a hard
precondition. A generic Seedling page is deliverable independently of both.

## What the source fragment actually does

The fragment exists in three places, and is inert in all of them.

- `ops/ansible/roles/tamanu-single-install/templates/caddy/common.j2:33` (host Caddy, Linux).
  No `root` directive anywhere in the Linux Caddy config, and nothing in the ops repo creates
  `/resources`. The `file` matcher never matches, so `handle_errors` falls through to Caddy's
  default output.
- `tamanu/packages/web/Caddyfile.docker` (the frontend container's own Caddy, `root * /app`).
  The path would resolve, but the pages are not in the image.
- `ops/scripts/windows/Install-Caddy.ps1:78` (Windows, `root * $WebRoot`). The original, and
  the only one paired with a `root`. `$WebRoot` is caller-supplied and the pages reach no build
  output, so this is likely dead too.

## The pages ship nowhere

`tamanu/packages/web/resources/errors/` holds `502.html` (self-contained: inline CSS,
data-URI logos, 13.5 KB), symlinks `501`/`503`/`504` to it, and `526.html` (references sibling
`bg-image.png` at 460 KB and `logo.svg`). None of it is published:

- vite's `publicDir` is the default `public/`, which holds three unrelated files. No override
  in `vite.config.js`, no copy plugin.
- `scripts/docker-build.sh`'s `build_web()` is `npm run build` (= `vite build`) plus
  `precompress-assets.sh`. No copy of `resources/`.
- The published image confirms it: `/app` in
  `ghcr.io/beyondessential/tamanu-frontend:sha-22a5d40b62854c4bc0d32ed39df5d2ae35962432`
  contains exactly `dist/`'s contents, with no `resources/` or `errors/`.

526 is out of scope: it is an unused status picked so a manual step could activate a
maintenance page, and that step was never built. Dropping it also drops the sibling-asset
problem, since 502 is self-contained.

## Verified Caddy mechanics

Checked against stock Caddy 2.11.4, matching the pinned `seedling-caddy:2.11.4-2`.

- `errors.routes` is per-server. Error routes carry `host` matchers and `terminal: true`,
  structurally parallel to the existing `proxy_routes_for_vhost`, so per-vhost error pages fit
  the one-server-per-port layout without rework.
- Status selection is a CEL matcher: `{http.error.status_code} in [502]`.
- Fetching the page from an upstream at error time works. When the page source is also down it
  degrades to a plain 502, with no cascade and no hang.
- Two traps, both reproduced and both fixable:
  - Without `handle_response` + `copy_response <status>`, a 502 reaches the client as
    **200 OK**.
  - Asking the Tamanu frontend for a page that does not exist returns `index.html` with 200,
    because of its `try_files {path} {path}/ /index.html` SPA fallback. Existence-driven
    lookup is unsafe against that upstream; the status list has to be declared, not discovered.

### Module contract delta (`docker/caddy/required-modules.txt`)

- Config-embedded delivery: none. `http.handlers.static_response` is already listed.
- File delivery: `http.handlers.file_server`, `http.handlers.rewrite`,
  `http.handlers.subroute`.
- Fetch-at-error delivery: `http.handlers.rewrite`, `http.handlers.subroute`,
  `http.handlers.copy_response`. (`reverse_proxy` already listed.)

## Verified Seedling mechanics

- `/data` (`CADDY_DATA_VOLUME`) is mounted read-write into the Caddy container, and Seedling
  already resolves its host path via `volume_mountpoint` and reads files inside it
  (`oi/handler/tls.rs`, `reconcile/state.rs`). Seedling can put files where Caddy reads them
  with no new mount and no blue/green slot swap.
- `rt.exec` runs argv in a container but does not capture stdout (exit code only; output goes
  to the log sink). Extraction from an image must be a `cp` into a shared volume, not a `cat`.
- `rt.write(target, path, contents)` already writes app-supplied content into a volume from an
  action closure, which is the precedent for a declaration shape.
- Config applies live via `POST /config/`, so config-only changes need no restart.
- `/apps/create` takes `AppScriptParams { app, script: String }`: one script, no sidecar. Inline
  provenance today means HTML as a rhai string literal.

## Two axes, not one ladder

Provenance (where the bytes come from) is independent of delivery (where Caddy reads them),
except that "fetch live" is both at once.

- Provenance: inline in the definition / extracted from the app image at install / fetched live
  from an upstream / Seedling's own generic set.
- Delivery: embedded in the generated config as a `static_response` body / a file under the
  already-mounted `/data` via `file_server` / fetched from an upstream via `reverse_proxy` plus
  `copy_response`.

## What this card owes Q1

Q1 frames its subject as script text throughout: `/apps/create` takes "source text and nothing
else", and the outline speaks of fetching "a definition". If Q1 lands as "fetch one script from
a source", J1 stays stuck with string literals.

J1's requirement on Q1: **the definition transport must carry sidecar files, not only a
script.** A folder-shaped definition (a zipped release artefact, or pulled from git) satisfies
this and puts the error pages directly alongside the definition that declares them, which
removes the inline-string-literal downside and the need to reach into the app's image.

This is a constraint to feed into Q1's undecided mechanism question, where the listed
candidates are a release artefact, the definition carried in the app's container image, or a
relay through an existing connection. The first two can carry a folder.

Q1's other open question, whether it gates the migration, rests on a premise worth correcting
in the same breath: it cites shipping "with definitions still in `apps/`", and those are demos
that would never run in prod. The question is really whether a production definition stays in
today's single-script format, which is a different question with a different answer.

## If Q1 slips

Q1 offers its own escape hatch: "it can ship with definitions still in `apps/` and pick this up
after". That premise does not hold. The `apps/*.seed.rhai` definitions are demos, are not
expected to become the production Tamanu definition, and would never run in prod. There is
nothing "still in `apps/`" to ship with.

The real escape hatch is the format, not the location: a production definition in today's
single-script form, maintained wherever it is maintained and pushed through `/apps/create`.
That form can carry the error page HTML as a rhai string literal, so J1's branded page is
reachable without Q1 at all. It is merely unpleasant, being 13.5 KB of HTML inside a script.

So Q1 is a dependency on definition *shape*, not a precondition for this card. Waiting on it
buys a folder holding the pages as files instead of literals, which is a sequencing choice
rather than a blockage.

Separately, a Seedling-generic error page needs no provenance mechanism and no Tamanu change at
all, with delivery either config-embedded or from `/data`. It covers the everything-down case
already agreed as acceptable to serve generically.

## PRD correction

The J1 row sits under "Every item below is something a production host's edge does today", and
it does not. No host serves custom error pages, so nothing regresses on cutover and J1 blocks
no host, tier 2 or otherwise. The row's tier and rationale should be restated as a feature
rather than parity.

## Open questions

- [ ] Should a failed `/api/*` XHR receive HTML, or pass the raw error through? The SPA fetches
      the API expecting JSON, so an HTML body may break its error handling. Caddy can match on
      `Accept`, so this is a choice and not a constraint, but it changes what the routes match.
- [ ] Which statuses get a page: 502/503/504, or wider?
- [ ] Does Seedling ship a generic fallback for apps that declare nothing, and does an app page
      override it per status or wholesale?
- [ ] Does the error page declaration sit on the HTTP service (alongside `compress` and
      `rate_limit`, resolving service to route) or on the ingress? Caddy's `errors.routes` is
      per-server and matched by host, so delivery is vhost-shaped even if declaration is not.
