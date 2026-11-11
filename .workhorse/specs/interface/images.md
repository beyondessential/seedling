---
id: IMGM
---

# Image management

## Registry Allowlist

<!-- from interface: Key Management / Registry Allowlist -->

> i[registry.list]
> `/registries/list` returns the current registry allowlist.

> i[registry.add]
> `/registries/add { registry }` adds a registry hostname to the allowlist.
> Adding a registry that is already present is a no-op.

> i[registry.remove]
> `/registries/remove { registry }` removes a registry hostname from the allowlist.
> All registered apps are re-evaluated after a registry is removed; any app whose images reference the removed registry will receive a `disallowed_registry` fault.

## Image Management

<!-- from interface: Key Management / Image Management -->

> i[image.list]
> `/images/list {}` returns the set of locally-present container images.
> Response `result` is an array of objects, each with fields:
>
> - `image_id`: string, the image's content-addressable digest (e.g. `"sha256:..."`).
> - `references`: array of strings; the tags and/or digest references that currently point at this image in local storage. May be empty for dangling images.
> - `size_bytes`: integer.
> - `created_at`: RFC 3339 timestamp, as reported by the container runtime.
> - `last_used_at`: RFC 3339 timestamp — the last time the runtime observed this image being used by a running container, or the time the image was first recorded locally if it has never been in use.
> - `in_use`: boolean; `true` when at least one running container currently uses this image.
> - `pinned_by`: array of app names that currently pin at least one reference resolving to this image.

> i[image.pull]
> `/images/pull { reference, app? }` requests that the named image reference be pulled into local storage through the runtime's standard pull machinery (with retries and back-off).
> `reference` must be a non-empty container image reference (e.g. `"ghcr.io/example/foo:1.2.3"`).
> If `app` is provided, the app must be registered; the pulled reference is additionally [pinned](rule:r/image.pin) to that app.
> Returns `{ "ok": true }` on accepted pull. Pull failures surface through the existing [image pull fault](rule:r/fault.image-pull) mechanism rather than the response.

> i[image.remove]
> `/images/remove { reference, force? }` removes the named image from local storage.
> If at least one running container currently uses the image, the request is rejected with `requirements_invalid` unless `force` is `true`.
> Pins targeting `reference` are cleared regardless of outcome.
> Returns `{ "ok": true }` on successful removal; `not_found` if no image resolves to `reference` in local storage.

> i[image.pin.list]
> `/images/pins/list { app? }` returns the set of image pins.
> When `app` is provided, only that app's pins are returned; otherwise pins for all apps are returned.
> Response `result` is an array of objects with fields `app` (string), `reference` (string), `pinned_at` (RFC 3339 timestamp), and `expires_at` (RFC 3339 timestamp or `null`). When `expires_at` is set, the reconciler will delete the pin once it passes (see [`image.pin.expiry`](rule:r/image.pin.expiry)).

> i[image.pin.clear]
> `/images/pins/clear { app, reference? }` clears pins for `app`.
> When `reference` is provided, only the pin matching that reference is cleared (no-op if absent); otherwise all pins for the app are cleared.
> Returns `{ "ok": true }`.

> i[image.discover]
> `/apps/images/discover { app, action_params?, lenient? }` runs the probe execution mode defined in [`image.discover`](rule:r/image.discover) across every handler declared by the named app (`install`, the implicit `start` handler, every explicit action, every shell, and every `on_change` handler), then returns what those handlers might pull.
>
> Params:
>
> - `action_params`: optional object map of handler name → object map of param name → string value. Values supplied here override stored values and defaults for the named handler's probe.
> - `lenient`: boolean, default `false`. When `true`, handlers with unresolved required parameters are reported as skipped and the probe continues; when `false`, the same condition is reported as an error and the other handlers still probe normally.
>
> Response shape:
>
> ```json
> {
>   "per_handler": [
>     {
>       "name": "migrate",
>       "kind": "action",
>       "images": ["ghcr.io/example/foo:1.2.3"],
>       "error": null,
>       "skipped_reason": null
>     },
>     { "name": "install", "kind": "install", "images": [], "error": null, "skipped_reason": null },
>     { "name": "upgrade", "kind": "action", "images": [], "error": null, "skipped_reason": "requires params: old_version" }
>   ],
>   "all_images": ["ghcr.io/example/foo:1.2.3"]
> }
> ```
>
> `kind` is one of `"install" | "start" | "action" | "shell" | "param_change"`. `error` is a human-readable message or `null`. `skipped_reason` is populated only in lenient mode when a handler was not probed. `all_images` is the deduplicated union of `images` across every handler with no `error` and no `skipped_reason`.
