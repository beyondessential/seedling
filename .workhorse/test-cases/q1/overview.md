# Test cases: definitions carry provenance and can be fetched

## Fetching

- [ ] A definition fetched from a standalone artefact registers, and `/apps/show` reports its reference and the manifest digest (verifies spec: i[definition.fetch], i[definition.provenance])
- [ ] A definition fetched from an image index entry registers, the index's image entries are ignored, and the recorded digest is the definition manifest's, not the index's (verifies spec: i[definition.fetch])
- [ ] A reference naming neither a definition manifest nor an index with a definition entry is refused with `fetch_failed`, and nothing is stored (verifies spec: i[definition.fetch])
- [ ] A fetch from a registry not on the allowlist is refused with `registry_not_allowed`, and no request reaches that registry (verifies spec: i[definition.fetch.access])
- [ ] A fetch while the registry is unreachable is refused with `fetch_failed`; the app keeps its previous definition, with no fault and no generation bump (verifies spec: i[app.update])
- [ ] A fetch from a private registry succeeds using the credentials the host's container engine uses, with no Seedling-side configuration (verifies spec: i[definition.fetch.access])
- [ ] A fetch from a registry the host has no credentials for, when it requires them, is refused with `fetch_failed` (verifies spec: i[definition.fetch.access])
- [ ] A fetched bundle whose `seedling.toml` requirement differs from its manifest annotation is refused with `bundle_invalid` (verifies spec: i[definition.fetch])
- [ ] `/registries/tags` lists a repository's tags, and is refused for a registry not on the allowlist (verifies spec: i[definition.tags])

## Selecting among index entries

- [ ] With entries annotated `>=0.12` and `>=0.13`, Seedling 0.13 selects the `>=0.13` entry (verifies spec: i[definition.fetch.select])
- [ ] With entries annotated `>=0.12` and `>=0.14`, Seedling 0.13 selects the `>=0.12` entry (verifies spec: i[definition.fetch.select])
- [ ] An unannotated entry is selected only when no annotated entry is a candidate (verifies spec: i[definition.fetch.select])
- [ ] No candidate refuses with `unsupported_seedling`; two candidates with the same minimum refuse with `fetch_failed` naming both (verifies spec: i[definition.fetch.select])
- [ ] A requirement with `||` alternatives is a candidate when any alternative matches, and its minimum is the lowest across the alternatives (verifies spec: l[bsl.bundle.seedling-versions])

## Bundles and metadata

- [ ] A bundle with a symlink, a path escaping the root, or a size over the cap is refused with `bundle_invalid`, whether pushed or fetched (verifies spec: i[definition.bundle.limits])
- [ ] A bundle with no `seedling.toml` evaluates `app.seed.rhai` (verifies spec: l[bsl.bundle.metadata])
- [ ] `script = ["lib.rhai", "app.seed.rhai"]` evaluates the two files concatenated in that order (verifies spec: l[bsl.bundle])
- [ ] An error in the second script file reports that file and its own line number (verifies spec: l[bsl.bundle.script-errors])
- [ ] An empty `script` list, a duplicate entry, a missing file, a non-UTF-8 script file, or an unknown field is refused with `bundle_invalid` naming it (verifies spec: i[definition.bundle.limits])
- [ ] A bundle requiring a newer Seedling that also has an unknown field is refused with `unsupported_seedling`, not `bundle_invalid` (verifies spec: i[definition.bundle.seedling-versions])
- [ ] The same files pushed as a folder and fetched as an artefact produce the same content hash and share one stored bundle (verifies spec: i[definition.content-hash], r[generation.script-storage])

## Bundle files in BSL

- [ ] `volume.write_dir` copies a folder containing a binary file byte for byte, and reapplies it on container restart (verifies spec: l[volume.write-dir])
- [ ] `volume.write_dir("/", …)` writes at the volume root (verifies spec: l[volume.write-dir])
- [ ] `volume.write` with a `File` writes it byte for byte (verifies spec: l[volume.write])
- [ ] `rt.write` with a `File` writes it byte for byte (verifies spec: l[rt.write])
- [ ] `app.file(path).text()` throws on invalid UTF-8 (verifies spec: l[file.text])
- [ ] `app.file` throws on an absolute path, a path escaping the root, and a missing file; `app.dir` throws when nothing is beneath the path (verifies spec: l[app.file], l[app.dir])
- [ ] Within an `on_change` handler after a definition update, `old.file` reads the previous bundle and `app.file` the new one (verifies spec: l[app.bundle.context])

## Evaluation failures are refused

- [ ] A fetched or pushed definition that fails to evaluate is refused with `script_error`; no fault is filed, no bundle stored, no generation bumped, and `/apps/show` and derived state are identical to before (verifies spec: i[app.update])
- [ ] An app with an active `script_error` from a failed param set has it cleared by a successful definition update (verifies spec: i[app.update])
- [ ] A stored definition that no longer evaluates after a Seedling upgrade files `script_error` on reload (verifies spec: i[app.persist])
- [ ] Reload of a stored combination runs no validators (verifies spec: i[app.persist], i[param.validation])

## Validation

- [ ] Setting `version` to a value the current definition's validator rejects is refused with `validation_failed` carrying the thrown reason; the value is not stored and the generation is unchanged (verifies spec: i[param.set], l[param.validate])
- [ ] A validator that errors by accident (e.g. calls an undefined function) rejects the value (verifies spec: l[param.validate])
- [ ] An unset param passes validation, so registering a definition whose `version` validator accepts only v2.12 succeeds with `version` unset (verifies spec: l[param.validate.unset])
- [ ] Unsetting a param is refused when another param's validator rejects the resulting values (verifies spec: i[param.unset], i[param.validation])
- [ ] A param set whose new values fail to evaluate but pass the last good evaluation's validators is stored and files `script_error` (verifies spec: i[param.validation], i[param.set])
- [ ] A param set whose new values fail to evaluate and fail the last good evaluation's validators is refused (verifies spec: i[param.validation])
- [ ] A validator declared conditionally on another param's value is found when the proposed values enable it (verifies spec: i[param.validation])
- [ ] A validator that defines a resource throws, and so rejects (verifies spec: l[param.validate.pure])
- [ ] `validate` called in an action closure, or twice on one param, throws (verifies spec: l[param.validate.constraints])

## Atomic definition and param update

- [ ] From def A at v2.11, updating to def B plus `version = v2.12`, where B's validator accepts only v2.12, succeeds in one generation; `version.on_change` runs under B with `old` reflecting A at v2.11 (verifies spec: i[app.update], l[param.on-change.old])
- [ ] The same B pushed without the param change is refused by B's validator, since the stored v2.11 fails it (verifies spec: i[app.update], i[param.validation])
- [ ] A validator on another param reading `version` sees the proposed v2.12, not the stored v2.11, during the atomic update (verifies spec: i[param.validation])
- [ ] A definition update with no param change schedules no `on_change` (verifies spec: i[app.update])
- [ ] Generation reconstruction at the atomic generation yields B with `version = v2.12`, and at the generation before yields A with v2.11 (verifies spec: r[generation.reconstruction])
- [ ] Replay of an interrupted `on_change` from an atomic update reconstructs B as target and A as source (verifies spec: r[operation.lifecycle.generations])

## Provenance and retrieval

- [ ] A pushed definition records `pushed_by` from the request's actor, and its content hash (verifies spec: i[definition.provenance])
- [ ] A push with an `origin` reports it as `reported_origin` (verifies spec: i[definition.source], i[definition.provenance])
- [ ] `Register` and `ScriptUpdate` generation history entries carry provenance; a `script_update` with a param change carries `param_name`, `previous_value`, and `new_value`, redacted for secrets (verifies spec: i[generation.history], r[generation.history])
- [ ] `AppUpdated` carries the new provenance and the param change (verifies spec: i[event.types])
- [ ] A past generation's bundle is retrievable with `/apps/bundle`, and `ctl apps export` writes a folder identical to what was pushed (verifies spec: i[app.bundle], i[ctl.definition.export])
- [ ] `ctl apps export` into a non-empty existing folder errors and writes nothing (verifies spec: i[ctl.definition.export])
- [ ] `/apps/script` returns the concatenated script and the file list (verifies spec: i[app.script])
- [ ] An app registered from a single script before the upgrade keeps working, and `/apps/script` still returns its text (verifies spec: i[app.script])
- [ ] Deregistering an app deletes bundles only it referenced (verifies spec: r[generation.deregister])

## Tag re-check

- [ ] A moved tag files `definition_source_moved` naming the reference and both digests; moving it back clears it, and so does replacing the definition (verifies spec: r[fault.definition-source-moved])
- [ ] A failed re-check leaves the fault exactly as it was, whether filed or not (verifies spec: r[fault.definition-source-moved])
- [ ] A filed fault survives a daemon restart followed by a failed re-check (verifies spec: r[fault.definition-source-moved], r[fault.lifecycle])
- [ ] Consecutive failed re-checks for one app back off to a cap and never stop; one later success files or clears correctly and resets the delay (verifies spec: r[definition.recheck.backoff])
- [ ] One app's failing registry does not delay another app's re-check (verifies spec: r[definition.recheck.backoff])
- [ ] A digest-pinned reference and a pushed definition are never re-checked and never file the fault (verifies spec: r[definition.recheck])
- [ ] A re-check against a registry removed from the allowlist makes no request and counts as failed (verifies spec: r[definition.recheck])
- [ ] A re-check downloads no bundle and changes nothing about the app (verifies spec: r[definition.recheck])

## Seedling version requirement

- [ ] Creating or updating with a definition the running Seedling doesn't satisfy is refused with `unsupported_seedling` (verifies spec: i[definition.bundle.seedling-versions])
- [ ] A stored definition whose requirement excludes the new Seedling version after an upgrade still evaluates and runs, and files `definition_unsupported` (verifies spec: r[fault.definition-unsupported])
- [ ] `definition_unsupported` clears when the definition is replaced by a supported one, and when Seedling restarts as a supported version (verifies spec: r[fault.definition-unsupported])

## Templates

- [ ] A template created from a reference records fetched provenance, and an app instantiated from it inherits that provenance (verifies spec: i[template.definition], i[template.instantiate])
- [ ] A template whose requirement the running Seedling doesn't satisfy is stored, reported `supported: false`, and refused at instantiation with `unsupported_seedling` (verifies spec: i[template.create], i[template.list], i[template.instantiate])
- [ ] A template bundle with sidecar files instantiates an app whose `app.file` reads them (verifies spec: i[template.instantiate])

## CLI

- [ ] Pushing a folder skips `.git` and `.jj`, and honours `.seedignore` (verifies spec: i[ctl.definition.folder])
- [ ] Pushing a folder containing a symlink errors naming it, and sends nothing (verifies spec: i[ctl.definition.folder])
- [ ] Pushing a GitHub folder URL at a branch records a push whose reported origin is the URL and the resolved commit (verifies spec: i[ctl.definition.source])
- [ ] `ctl apps update --ref … --set version=…` performs one atomic update (verifies spec: i[ctl.definition.param])
- [ ] `ctl registries tags` lists tags (verifies spec: i[ctl.registries.tags])

## Web

- [ ] The app detail page shows fetched provenance as reference and digest, and pushed provenance as hash, pusher, and reported origin marked as reported (verifies spec: w[routes.apps.definition])
- [ ] Update from registry lists the recorded repository's tags, accepts a typed reference, and applies definition plus one param in one request (verifies spec: w[routes.apps.definition.fetch])
- [ ] A validator rejection shows in the review step and disables Apply (verifies spec: w[routes.apps.definition.fetch])
- [ ] With `definition_source_moved` active, the page flags the moved tag and preselects it (verifies spec: w[routes.apps.definition.fetch])
- [ ] Editing the script of a single-script bundle with sidecars keeps the sidecars (verifies spec: w[routes.apps.definition.edit])
- [ ] A definition whose script spans several files opens read-only in the editor (verifies spec: w[routes.apps.definition.edit])
