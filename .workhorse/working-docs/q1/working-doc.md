---
status: draft
---

# Definitions carry provenance and can be fetched from their source

An app's definition lives with the app it describes, carries provenance for where it came from and at which version, and Seedling can fetch a newer one so the definition and app version move together.

## Context

What exists today, as read from `docs/spec/`:

- `/apps/create { app, script }` and `/apps/update { app, script }` take BSL source text and nothing else (`i[app.register]`, `i[app.update]`). ctl reads a single file and posts its contents.
- A failed update files `script_error`, keeps the previous AppDef running, and leaves every derived piece of state untouched; a partially-evaluated definition is never observable (`i[app.update]`). Whatever fetching looks like must keep this.
- Generations are `(script, parameter values)` pairs. Script bodies are stored by content hash and shared across generations (`r[generation.script-storage]`); `/apps/script { app, generation? }` returns the body at any generation (`i[app.script]`). Nothing records where a body came from.
- Templates (`i[template.definition]`) are stored script bodies copied wholesale into a new app. Also single-text, also no provenance.
- The host already reaches a container registry for every upgrade: the demo Tamanu definitions derive three image refs from the `version` param, and `version.on_change` runs the upgrade closure. The PRD's Canopy section relies on that ("one param set is a whole upgrade").

Constraints gathered so far:

- **Carries files, not only text.** From J1 (custom error pages, card comment): Tamanu's error page HTML exists in no artefact Seedling can reach, so the definition is the natural carrier. A single-script format forces 13.5 KB of HTML into a string literal, and it won't be the only asset. The mechanism should carry a folder.
- **Poor links.** A migrating host is often on a poor connection; anything that requires the host to reach a new external service needs care.
- **Does not gate the migration.** The `apps/` definitions are demos and never run in prod, so there is nothing "still in `apps/`" to ship with. A production definition can stay single-script and be pushed through `/apps/create` from wherever it is maintained, which is why this need not block migration. (The PRD's open question carries the wrong premise; worth correcting there too.)

## Behaviour

### Definition version and app version are independent, and checked

The definition's source ref and the app's version are two separate knobs. Neither moves the other automatically. The definition declares which app versions it supports, and Seedling refuses a combination the definition does not support. That keeps the "move together" property as a guard, not a coupling: setting `version` to a release the current definition doesn't cover is caught, where today it silently produces an app configured for the wrong regime.

Rejected: making the source ref *be* the version (drops the `version` param, repointing the source is the upgrade), and having a nominated param trigger a fetch (ties fetching to one param's semantics).

Interaction with today's upgrade path worth keeping in view: `on_change` runs under the definition current at the time of the param set, with `old` being the previous generation (`l[param.on-change.old]`). So the definition that performs an upgrade from N to N+1 is whichever one is current when `version` is set, which means it has to be the new one.

### Supported versions are declared through generic param validation

A param can carry a validator. Seedling runs it before accepting any change that would produce a new combination of definition and param values, and rejects the request if it fails: nothing is persisted, no generation is bumped. This is how a definition says which app versions it supports, and there is no Seedling-level notion of "the app version" or of version ordering; the script decides what it accepts.

This differs from how a script failure behaves today. A script that fails to evaluate on update or param set files `script_error` and the request still succeeds (`i[app.update]`, `i[param.set]`). A validator failure is a refusal: the request fails and the previous state stands.

A validator gets the full proposed param map, not only its own value, so it can check constraints that span params. It is pure: no `rt`, no side effects.

Validators see only set values: an unset param always passes, and `.required()` stays the thing that covers presence at install. So registration (`/apps/create`, template instantiate) is unchanged and takes no params. A definition that insists on a supported `version` accepts being registered without one, and its validator bites when `version` is set. Reloading at restart runs no validators. The stored combination was validated when it was written, and there's no request to refuse.

Rejected: a nominated version param with a declared range that Seedling understands. It would let Seedling suggest a fitting definition for a target version, but commits Seedling to a version scheme.

### Crossing a regime change is one atomic update

One request carries a new definition and any param values to change alongside it. Validation applies to the resulting combination, so def B (supports only v2.12) plus `version = v2.12` is accepted from an app on def A at v2.11. It produces a single generation. Params whose values changed fire `on_change` under the new definition, with `old` being the previous generation, i.e. the old definition at the old values. That puts the upgrade closure in the definition that knows the new regime, and gives it a read of the old one.

A definition update that changes no params still fires no `on_change`, as today.

Rejected: requiring each release's definition to accept its predecessor's version so the two steps can be separate. It asks every definition to carry its predecessor's regime.

### A definition that fails to evaluate is refused

If a new definition fails to evaluate, whether fetched or pushed, and with or without params alongside it, the request fails and nothing is stored. No bundle, no param values, no generation bump, no fault. The previous definition keeps running and nothing observable changes. Validators come from the evaluated definition, so a definition that doesn't evaluate can't be validated, and "couldn't check" must not turn into "accepted".

This changes `i[app.update]`. Today an evaluation failure there files `script_error` and the request succeeds. `script_error` still has a role for evaluation failures that aren't a request the operator can be refused: reloading at restart, for instance, where a Seedling upgrade might have changed BSL underneath a stored definition.

A fetch that fails (registry unreachable, ref not found, not a definition artefact, blocked by the allowlist) fails the request the same way.

Param sets and unsets keep their existing behaviour. One whose re-evaluation fails is stored, and `script_error` is filed. That lets an operator set several interdependent params one at a time. Validators still run on a param set. It's only an evaluation failure that's tolerated, not a validation failure.

Validators come from evaluation, so a param set whose new values don't evaluate takes its validators from the definition's last successful evaluation and runs them against the proposed params. If they pass, the value is stored and `script_error` is filed. If they fail, the set is refused.

### Folder-shaped definitions

In scope for this card. A definition is a bundle: an entry script plus sidecar files. BSL gains a way to read files from the bundle. J1's error pages are the first consumer.

BSL gets both per-file access and a directory copy. The directory copy is something like `volume.write_dir("/errors", app.dir("errors"))`: a folder of assets lands in a volume in one static declaration and reapplies the way static `Volume.write` does. Since a directory may hold binary assets, the copy is byte-exact.

Per-file access (`app.file(path)`) returns a file value. `Volume.write` accepts it and writes it byte-exact, and `.text()` gives a string where one is needed (throwing on invalid UTF-8).

The entry script is `app.seed.rhai` at the bundle root, and there's no manifest. That name is an interface contract that app repos publish against, so it goes in the spec.

Bundle limits, applied to pushed and fetched bundles alike:

- A total size cap. Seedling refuses anything over it. The figure lives in docs and code, not in spec prose.
- Regular files only: no symlinks, devices or other special files. Paths are normalised and can't escape the bundle root.

When ctl pushes a folder, it skips VCS metadata (`.git`, `.jj` and similar) and honours an ignore file if one is present.

For development, ctl can take a GitHub URL including a folder path (at a branch, tag or commit) as the thing to push. ctl resolves it to a commit, downloads the folder, and pushes it as a bundle, so the host needs no GitHub access. It's a push as far as the daemon is concerned. The provenance records it as pushed, plus the URL and commit ctl reported as the origin, labelled as reported by the client, since the daemon can't verify them.

A lone script is a one-file bundle: its entry script and nothing else. `{ app, script }` keeps working as shorthand, existing apps need no migration, and templates can hold bundles.

### Pushed and fetched definitions both exist

A definition reaches Seedling in one of two ways:

- **Fetched** from an OCI reference. The provenance is that reference plus the digest it resolved to.
- **Pushed** from a local folder: ctl bundles it and uploads it. This covers development, air-gapped hosts and hotfixes. The provenance records that it was pushed, its content hash, and who pushed it.

### Provenance is visible and bundles are retrievable

- `/apps/show` carries the current definition's provenance. For a fetched definition that's the ref and digest; for a pushed one, the content hash and who pushed it.
- Every `Register` and `ScriptUpdate` generation history entry carries the provenance of the definition it installed, next to the existing content hash (`r[generation.history]`).
- The whole bundle is retrievable at any generation, and ctl can export it to a folder. `/apps/script`'s single-text response stays for one-file bundles. Bundles are stored by content hash and shared across generations, the same way script bodies are today (`r[generation.script-storage]`), and deleted with the app.

### Fetching names a full reference

At the OI and CLI level every fetch names a complete OCI reference. Nothing is remembered as a "source" that later fetches resolve against. The recorded provenance ref is just a record.

The web UI can offer an easier path. It derives the repository from the app's current provenance ref, lists that repository's tags, and lets the operator pick one. It then builds the full ref and submits that. Tag listing is therefore an OI capability, and per the repo rule it also has a CLI command.

### Seedling notices when a recorded tag moves

Seedling never fetches or applies anything on its own. It does periodically re-resolve the tag in each fetched app's provenance ref and surfaces it when that tag now points at a different digest than the one the app is running. It keeps running the digest it has.

Pushed definitions and refs that name a digest have nothing to re-resolve.

A move is surfaced as a condition fault, `definition_source_moved`, keyed to the app. It is filed while the recorded tag resolves to a digest other than the running one, and cleared when the tag resolves back to the running digest or the app's definition is replaced. A failed check neither files nor clears it: on a poor link "couldn't reach the registry" must not read as "unchanged", and it must not read as "moved" either. It reaches Canopy through the existing `health/faults` check with no new wiring.

## Implementation options

### Transport: OCI artefact (chosen)

A definition bundle is published as an OCI artefact in a registry, typically the same one as the app's images. Why:

- The host already has to reach that registry to upgrade, so this adds no connectivity. That matters on poor links.
- It carries a folder, which folder-shaped definitions need.
- The digest is content-addressed provenance for free. The tag is the human-facing version.
- It sits behind the existing registry allowlist (`i[registry.add]`), so the operator controls where definitions come from the same way they control where images come from.
- It stays separate from the images, which fits the decision that the definition and the app version are independent.

Two placements, both plausible:

- **Standalone artefact**: its own repository, e.g. `…/tamanu-central-seed:v2.12.0`.
- **Composited into the image index**: an extra manifest inside the app image's multi-platform index at the same tag, marked with a definition artifact type and no runnable platform. buildx attestation manifests set the precedent: container runtimes select by platform and skip them. So one reference names both the images and the definition that goes with them, and the independence holds anyway, because an operator can still point the definition at a different ref.

Seedling accepts both. There's no real difference between them: resolution always selects a manifest with the definition artifact type, either by checking a single manifest's type or by picking the definition entry out of an index.

Authentication and the allowlist work exactly as they do for image pulls. The registry allowlist governs definitions too, and there's no new credential store. Image pulls go through the container engine today and Seedling adds no auth of its own, so if Seedling fetches definitions itself rather than through the engine, it has to end up with the same access. That's a tech design question.

Rejected:

- **Definition inside the app image**: evaluation would first need a large image pulled, an app with several images would have to nominate one, and it couples the definition to an image release.
- **Release asset**: a new endpoint for the host to reach, and forge-specific.
- **Canopy relay**: depends on a live bestool offer, and the relay is deliberately outbound-only.

### Fetch path: Seedling resolves directly (chosen)

Seedling fetches with an in-process OCI client rather than going through the container engine.

Today every pull goes through Podman's libpod REST API over `/run/podman/podman.sock`, and Podman resolves the registry, manifest, digest and auth itself. Seedling has no OCI client and holds no registry credentials. The reason not to reuse that path is shape: Podman's pull is image- and platform-shaped. Selecting a manifest by artifact type out of an index, resolving a tag to a digest without pulling it, and listing a repository's tags all sit outside what that API offers, and the moved-tag check and the web UI's tag picker need exactly those three. Routing the fetch through Podman would leave those needing a direct registry call anyway, splitting the path for no gain.

Auth: the client reads the same credential file Podman reads, so "no new credential store" holds and Seedling's access is whatever the host already has for image pulls. This is a shared read of an existing file, not a second store to populate.

Allowlist: Seedling checks the allowlist itself before issuing the request, so a definition fetch is gated hard. This is stronger than image pulls, where `is_registry_allowed` exists but has no non-test caller and the allowlist only drives the `disallowed_registry` fault rather than blocking anything. Definition fetch gives that function its first real caller. The asymmetry is deliberate: a refused fetch answers a request an operator made and can be told about, where an image pull happens mid-reconciliation with no request to fail.

### Tag re-check cadence and back-off

Baseline cadence is coarse, in the six-hour to daily range rather than hourly. Tags move rarely, migrating hosts are often on poor links, and noticing a move some hours late costs nothing because Seedling never acts on it: the fault is the whole output.

The interval is a fixed constant, matching TLS renewal and the Tailscale poll rather than GC's operator flag. `TailscaleConfig.poll_interval` is the precedent for carrying the field now and wiring a flag later if a deployment ever needs one.

The schedule carries jitter, in the shape of backup scheduling's `random_delay_secs` (a random 0..interval/10 delay), so a fleet of hosts doesn't converge on the same instant against the same registry. That is the only other jittered timing in the codebase.

Back-off on failure follows `r[actuate.image.retry]` and the existing `RetryGate`: capped exponential on consecutive failures, no terminal give-up state, success resets the count. Keyed per app, so one unreachable app's registry doesn't pace another's. Because the baseline is already coarse, back-off's job here is narrow: stop a host on a dead link burning a round trip every cycle, so the cap sits well above the baseline interval rather than near it.

It runs as its own spawned task, like the Tailscale poller and TLS renewal, not as a ticker riding the 5s reconcile loop. It is a slow, self-contained external poll with its own back-off and its own fault. The `ScheduleTicker`/`BackupTicker` pattern exists for work that has to interleave with reconciliation, which this doesn't.

**Implementation trap worth naming.** `sync_faults` is the usual condition-fault primitive, and Tailscale's `sync_unreachable_fault` looks like the template, but converging blindly each pass is wrong here. A failed check must leave `definition_source_moved` exactly as it was, filed or not, and a converge run on a tick where the resolution failed would clear a filed fault. The converge set can only be built from a successful resolution; a failed one skips the sync entirely. This is the general rule in `runtime.md`, that an iteration withholding an apply must draw no conclusion from having done so, and the failure-modes rule that wholesale-applied state must not be applied when a contributor is missing from it.

## Testing notes

- A definition fetched from a standalone artefact and one fetched from an image index entry both register, and both record ref and digest.
- A ref that resolves to neither a definition artefact nor an index with a definition entry is refused, and nothing is stored.
- A fetch from a registry that isn't on the allowlist is refused.
- A fetch while the registry is unreachable is refused, and the app keeps running on its previous definition with no fault and no generation bump.
- A fetched or pushed definition that fails to evaluate is refused. There's no `script_error`, no stored bundle, no generation bump, and `/apps/show` and derived state are identical to before the request.
- An atomic update from def A at v2.11 to def B plus `version = v2.12`, where B's validator accepts only v2.12, succeeds in one generation. `version.on_change` runs under B, with `old` reflecting A at v2.11.
- The same B pushed without the param change is refused by B's validator (the current v2.11 fails it).
- Setting `version` to a value the current definition's validator rejects is refused; the value isn't stored.
- A param set whose new values fail to evaluate but pass the last good evaluation's validators is stored, and files `script_error`.
- An unset param passes validation, so registering a definition whose `version` validator rejects every value except v2.12 succeeds with `version` unset.
- A validator reading another param sees the proposed value of that param in an atomic update, not the stored one.
- Restart reload of a stored combination runs no validators. A stored definition that no longer evaluates after a Seedling upgrade files `script_error`.
- A moved tag files `definition_source_moved`. Moving it back clears the fault, and so does replacing the definition. A failed re-check leaves the fault exactly as it was, whether filed or not.
- A re-check that fails does not clear a `definition_source_moved` fault that was already filed, and does not file one that wasn't: the fault survives an intervening failed check across a daemon restart, where the in-memory failure count starts at zero but the fault is in the database.
- Consecutive failed re-checks for one app back off up to a cap and never disable the check: after any run of failures, one later success re-resolves the tag and files or clears the fault correctly.
- Fetching resolves a definition from a registry that is on the allowlist but for which the host has no stored credentials, using the same credential source image pulls use, and a private registry the credentials don't cover is refused the fetch (not silently pulled).
- A digest-pinned ref and a pushed definition never file `definition_source_moved`.
- `volume.write_dir` copies a folder containing a binary file byte-exact, and reapplies on container restart.
- `app.file(path).text()` throws on invalid UTF-8. `Volume.write` with a file value writes it byte-exact.
- A bundle containing a symlink, a path escaping the root, or exceeding the size cap is refused, whether pushed or fetched.
- ctl push of a folder skips `.git`/`.jj` and honours the ignore file.
- ctl push from a GitHub URL records a push with the reported URL and commit.
- An existing app registered from a single script keeps working after the upgrade, and `/apps/script` still returns its text.
- A past generation's full bundle is retrievable and exports to a folder identical to what was pushed.
- Generation history `Register` and `ScriptUpdate` entries carry provenance.

## Open questions

- [x] What "move together" means for the `version` param: independent, and checked
- [x] Is a folder-shaped definition in scope: yes, this card
- [x] Which transport: OCI artefact
- [x] Does push stay: yes, alongside fetch
- [x] Standalone artefact or composited into the image index: both, resolved by artifact type
- [x] Registry credentials: the same as image pulls today; the allowlist applies, no new credential store
- [x] What an operator names when fetching: a full ref (the web UI may derive one from a tag picker)
- [x] Does Seedling look at the source on its own: re-resolves recorded tags and surfaces a move, never applies
- [x] How a moved tag is surfaced: condition fault `definition_source_moved`; a failed check neither files nor clears
- [x] How a definition declares the app versions it supports: generic param validation
- [x] What a validator sees: the full proposed param map, and it's pure
- [x] How an upgrade crosses non-overlapping definitions: one atomic definition-plus-params update
- [x] A definition that fails to evaluate: the request is refused and nothing is stored (changes `i[app.update]`)
- [x] Param set whose re-evaluation fails: unchanged, stored with `script_error`
- [x] How BSL uses sidecar files: per-file access plus a directory copy into a volume
- [x] What per-file access returns: a file value, byte-exact into `Volume.write`, `.text()` for strings
- [x] Single scripts and templates: a lone script is a one-file bundle; templates can hold bundles
- [x] Which validators apply to a param set whose new values don't evaluate: the last good evaluation's
- [x] Where provenance is surfaced: `/apps/show`, generation history, and whole-bundle retrieval at any generation
- [x] How a bundle names its entry script: a fixed name at the root, no manifest
- [x] Validators at registration and reload: unset always passes, create is unchanged, reload runs none
- [x] Entry script name: `app.seed.rhai`
- [x] Bundle limits: a size cap, regular files only, ctl skips VCS metadata and honours an ignore file
- [x] Pushing from a GitHub URL plus folder path: ctl downloads and pushes; provenance is a push with a client-reported origin
- [x] How often tags are re-checked, and its back-off on a poor link: coarse (6-hourly to daily) fixed constant with jitter; per-app capped-exponential back-off on failure, no give-up; standalone spawned task; a failed check never files or clears the fault
- [x] Fetch path vs container-engine auth: Seedling resolves directly with an in-process OCI client reading Podman's existing credential file; the allowlist is a hard gate for definition fetches
