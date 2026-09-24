# Definitions carry provenance and can be fetched from their source

Split from the card's working doc, which keeps the full reasoning. This plan holds the implementation choices and trade-offs; behaviour lives in `docs/spec/` (language, interface, runtime, web).

## Transport: OCI artefact

A definition bundle is published as an OCI artefact, typically in the same registry as the app's images.

- The host already reaches that registry to upgrade, so fetching adds no new connectivity. That matters on poor links.
- It carries a folder, which bundles need.
- The digest is content-addressed provenance; the tag is the human-facing version.
- It sits behind the existing registry allowlist, so operators control where definitions come from the same way as images.
- It stays separate from the images, which fits the definition and the app version being independent knobs.

Both placements are accepted, standalone (its own repository) or as an entry in the app image's index. The index entry follows the precedent of buildx attestation manifests: runtimes select by platform and skip it. Resolution is the same code path either way: check a single manifest's artifact type, or pick definition entries out of an index.

Artefact shape: one manifest, artifactType `application/vnd.bes.seedling.definition.v1`, one layer of media type `application/vnd.bes.seedling.definition.v1.tar+gzip` holding the bundle as a gzipped tar, root at the bundle root. The Seedling version requirement is mirrored as annotation `vnd.bes.seedling.versions` on the manifest and on the index descriptor, so selection among several definition entries needs only the index. These three names were picked at split time and are worth a second look before anything publishes against them; OCI recommends reverse-domain annotation keys.

Rejected transports:

- **Definition inside the app image**: evaluation would first need a large image pulled, an app with several images would have to nominate one, and it couples the definition to an image release.
- **Release asset**: a new endpoint for the host to reach, and forge-specific.
- **Canopy relay**: depends on a live bestool offer, and the relay is deliberately outbound-only.

## Fetch path: Seedling resolves directly

Seedling fetches with an in-process OCI client rather than through Podman.

Image pulls today go through Podman's libpod REST API over `/run/podman/podman.sock` (`crates/core/src/system/podman.rs`), and Podman resolves registry, manifest, digest and auth. Seedling has no OCI client and no registry credentials. Podman's pull is image- and platform-shaped: selecting a manifest by artifact type, resolving a tag without pulling, and listing tags are all outside it, and the tag re-check and the web tag picker need exactly those. Going through Podman would still need a direct registry client for those, splitting the path.

Auth: the client reads the same credential file Podman reads (containers-auth). That keeps "no new credential store": Seedling's access is whatever the host already has for image pulls. Resolve the path the way Podman does rather than hard-coding one.

Allowlist: checked by Seedling before any request, so a definition fetch is a hard gate. Image pulls only fault (`disallowed_registry`); `runtime::registries::is_registry_allowed` exists but has no non-test caller, and this gives it one. The asymmetry is deliberate: a refused fetch answers an operator's request, where an image pull happens mid-reconciliation with no request to fail. Registry host extraction today is `defs::container::image_registry` (text before the first `/`); use a proper reference parser for definitions and consider sharing it.

Pick the crate with `cargo add` and check it handles artifact manifests, index descriptors with annotations, tag listing, and bearer-token auth flows against ghcr.io and docker.io.

## Wire format for pushed bundles

`bundle` on the wire is a JSON object map of path to base64 contents. It can't express symlinks or special files at all, so the "regular files only" rule mostly bites on fetched tars. Check the bundle size cap against any per-request size limit on the control stream, and raise or special-case that limit if needed.

Content hash: computed over sorted `(path, bytes)` pairs, not over archive bytes, so a pushed folder and the same folder fetched as a tar hash the same and share storage. Document the exact canonical form in code.

## Bundle metadata

`seedling.toml` at the bundle root, optional, two optional fields: `seedling` (version requirement) and `script` (array of script files, default `["app.seed.rhai"]`). This reverses the working doc's "no manifest" decision, taken at split time when the Seedling version requirement came in: a requirement must be readable without evaluating the script, and a file keeps room for later metadata.

Parse order matters: read `seedling` first and check it, then strictly validate the rest. Otherwise an older Seedling reports a newer bundle's new field as malformed instead of as unsupported.

Version requirement syntax: semver comparator sets joined by `||` (npm-style). The `semver` crate's `VersionReq` handles one comma-joined set, so split on `||` and treat it as any-of. "Minimum" for index selection is the lowest version satisfying any set.

Script concatenation: evaluation errors must map back to file and line. Keep an offset table from concatenated line to `(file, line)` and rewrite Rhai error positions through it.

## Evaluation and validation ordering

`/apps/update` changes from "evaluation failure files `script_error` and succeeds" to "refused, nothing observable". Order of work in the handler:

1. Resolve the definition (fetch or decode), check the version requirement, then bundle limits. No writes.
2. Evaluate with the proposed params (current map plus the one change).
3. Run validators of every set param against the proposed map.
4. Commit bundle, provenance, param change and generation bump in one transaction, then make it observable, then dispatch `on_change`.

Every early return before step 4 must leave memory and the database untouched (failure-modes rule on observable state). `script_error` stays for reload and for param set/unset whose re-evaluation fails.

Param set/unset: evaluate with the proposed values; if that fails, take validators from the most recent successful evaluation, which means the registry must keep the last good AppDef's validators around, not only the AppDef.

Validators are Rhai closures captured at top level; run them with no `rt` in scope and with resource-definition methods throwing. Reuse whatever the engine does to make `old` read-only.

## Generation storage

New migration block at the bottom of `crates/core/src/runtime/db.rs` (never edit an existing one):

- Bundle storage keyed by content hash, replacing or extending the script-body table. Existing script bodies become one-file bundles (`app.seed.rhai`), with the content hash recomputed in the bundle's canonical form.
- Provenance columns (or a table) per `Register`/`ScriptUpdate` generation. Existing rows get `pushed` provenance with no actor, since none was recorded.
- Optional param change on `ScriptUpdate` rows.
- Templates: store bundle hash and provenance; migrate existing bodies the same way.

Reconstruction (`r[generation.reconstruction]`) now reads param changes from `ScriptUpdate` rows too; check every reader of the param-history query.

Use `ON CONFLICT ... DO UPDATE` for any row another writer touches.

## Tag re-check

Coarse cadence, six-hourly to daily, as a fixed constant (TLS renewal and the Tailscale poll are the precedent; `TailscaleConfig.poll_interval` shows how to carry the field for a later flag). Jitter in the shape of backup scheduling's `random_delay_secs` (0..interval/10).

Back-off on failure follows `r[actuate.image.retry]` using `runtime::retry::RetryGate`, keyed per app, cap well above the baseline, no terminal state. Runs as its own spawned task like the Tailscale poller and TLS renewal, not a ticker on the 5s reconcile loop.

**Trap.** `sync_faults` is the usual condition-fault primitive and `TailscaleProvider::sync_unreachable_fault` looks like the template, but converging each pass would clear a filed `definition_source_moved` on a tick where resolution failed. Build the converge set only from successful resolutions, per app, and skip the sync for apps whose check failed. This is the rule that an iteration withholding an apply draws no conclusion from it. Scope the sync `FaultScope::AppKind(app, "definition_source_moved")` per app so one app's failure can't touch another's fault.

Selection depends on the running Seedling version, so after a Seedling upgrade a tag that hasn't moved may select a different index entry. That files the fault, which is the right outcome: a better-suited definition is available.

`definition_unsupported`: evaluated at startup and on every definition replacement; converge with `sync_faults` per app at those points (the condition only changes then).

## Consistency extensions made at split time

- `rt.write` accepts a `File` as well as a string, matching `Volume.write`.
- `/apps/plan` accepts a proposed bundle or reference as well as script text, and reports validator rejections, so the web review step can show a rejection before apply.
- Templates carry provenance and instantiated apps inherit it; templates store unsupported bundles and report `supported`, while instantiation refuses them.
- The web script editor edits single-script-file definitions only, carrying sidecars over; multi-file scripts are read-only there.

## Follow-ups

- A template catalogue that Seedling could pull from: card C5. Builds on template provenance and the version requirement for filtering.
- PRD correction: the migration-gating open question carries the premise that definitions are "still in `apps/`". The `apps/` definitions are demos; the real reason this card doesn't gate migration is that a production definition can stay a single pushed script.

## Docs to update when implementing

- `docs/bsl-scripting.md`: bundles, `seedling.toml`, `app.file`/`app.dir`, `write_dir`, validators.
- `docs/deploying.md`: publishing a definition artefact (e.g. with `oras push`), the annotation, and index placement.
- `docs/failure-modes.md` if the re-check trap turns out to be a pattern worth listing.

## Build checklist

Ordered so each stage compiles and tests on its own.

- [x] Bundle model (`crates/core/src/defs/bundle.rs`): paths, limits, `seedling.toml` metadata, version requirements, content hash and canonical encoding, script concatenation with an offset table, error position rewriting, tar.gz and base64-map decoding
- [x] Provenance type and its JSON shape
- [x] BSL: `File`, `Directory`, `app.file`, `app.dir`, `file.text`, `volume.write` with a `File`, `volume.write_dir`, `rt.write` with a `File`; volume writes carry bytes
- [x] BSL: `param.validate`, run against proposed values after evaluation; resource definitions throw inside a validator
- [x] Evaluation entry point takes a bundle; every caller (`AppEntry`, reload, replay, lifecycle, shells, images, registries, reconcile, templates) switches to it
- [x] Migration: `definition_bundles`, bundle hash, provenance and param change on generations, template bundles; backfill existing scripts as one-file bundles
- [x] Generation storage: bundle store/load, provenance, param change on `ScriptUpdate`, reconstruction and history readers, GC including templates
- [x] OCI fetch (`crates/core/src/runtime/definition/fetch.rs`): reference parsing, allowlist gate, containers-auth credentials, manifest or index resolution, selection, layer pull, annotation check; tag listing; digest-only resolution for re-checks
- [x] Handlers: definition source parsing, `/apps/create`, `/apps/update` (refusal semantics, validators, atomic param change, `on_change`), param set/unset validation, `/apps/show` provenance, `/apps/script` files, `/apps/bundle`, `/apps/generations`, `/apps/plan`, `/registries/tags`, `AppUpdated` fields, error codes
- [x] Templates carry bundles and provenance; `supported`; instantiation refuses unsupported
- [x] Faults: `definition_unsupported` at startup and on replacement; tag re-check task with per-app back-off filing `definition_source_moved`
- [x] CLI: definition sources (file, folder with `.seedignore`, GitHub URL, `--ref`), `--set`/`--unset`, `apps export`, `registries tags`
- [x] Web: provenance on the app page, update from registry with tag picker and plan review, editor keeps sidecars and is read-only for multi-file scripts
- [x] Docs: `docs/bsl-scripting.md`, `docs/deploying.md`
- [x] Tests against the test-cases file

## Implementation notes

- Bundles cap at 2 MiB of file contents. The daemon and ctl limit a request to 4 MiB, and the web proxy's request-line limit was raised from 1 MiB to match, so a bundle pushed from the web editor reaches the daemon.
- The size limit applies at intake only (pushed, fetched). A stored bundle always reloads, including a script stored before bundles existed that is larger than the limit.
- Content hashes are `sha256:`-prefixed hex over the canonical encoding in `runtime/definition/bundle.rs`, which is also the storage format. Migration v58 rehashes every stored script into it and records `pushed` provenance with no actor.
- A lone script is returned by `/apps/script` exactly as written; files are joined with a newline only between files that lack one, so the line table stays exact.
- Validators are captured during an evaluation run with validation on, and called with the same engine and AST. For a param set or unset whose proposed values fail to evaluate, the fallback is the registry's running `App`, whose `bundle` and `stored` are the last successful evaluation's.
- The re-check ticks every five minutes; a healthy app is due every six hours plus up to a tenth of that, a failing one on a 15-minute-to-24-hour back-off. A result is dropped if the app's definition was replaced while the registry was being asked.
- `definition_source_moved` is keyed by the newly selected digest, so a tag that moves again replaces the fault instead of leaving one describing a digest it no longer names.

