---
id: TPL
---

# Templates

## Persistent History

<!-- from runtime: Persistent History -->

> r[template.persist]
> Stored templates (see [template.definition](rule:i/template.definition)) must be persisted durably and must survive runtime restarts.
> Template removal must be durable: a removed template must not reappear after restart.
> Templates are independent of app persistence: removing a template has no effect on apps previously instantiated from it.

## Templates

<!-- from interface: Templates -->

> i[template.definition]
> A template is a stored, named app definition [bundle](rule:l/bsl.bundle) that is held for reuse, together with the bundle's [provenance](rule:i/definition.provenance).
> Templates are not themselves evaluated by the reconciler: they do not have a generation, phase, resources, parameter values, faults, or any other runtime state.
> A template exists solely as a bundle that can be inspected (previewed) and copied wholesale into a new app via `/templates/instantiate`.
> Template requests supply a definition as for [definition.source](rule:i/definition.source), with the script text given as `body` in place of `script`.

> i[template.name]
> Template names follow the same rules as app names — see [bsl.name](rule:l/bsl.name) — and must not start with an underscore.
> Template names share no namespace with app names; a template and an app may have the same name.

> i[template.create]
> `/templates/create { name, ..., description? }` stores a new template.
> The definition is not evaluated at create time; syntactically invalid scripts are accepted and will surface as errors on `/templates/preview` or `/templates/instantiate`.
> A definition that cannot be fetched or whose bundle breaks the [bundle rules](rule:i/definition.bundle.limits) is refused as it would be for an app, and nothing is stored.
> A definition whose Seedling version requirement the running Seedling does not satisfy is stored.
> If a template with the given name already exists the request is rejected with `requirements_invalid`.
> On success a `TemplateCreated` event is emitted.

> i[template.list]
> `/templates/list {}` returns an array of objects with fields `name`, `description` (string or `null`), `created_at` (RFC 3339 timestamp), and `supported` (whether the running Seedling satisfies the template's declared Seedling version requirement).
> The array is ordered by name.

> i[template.show]
> `/templates/show { name }` returns a single object with fields `name`, `body` (the template's script files concatenated in order), `provenance`, `supported` (as for `/templates/list`), `description`, and `created_at`.
> Returns `not_found` if no such template exists.

> i[template.update]
> `/templates/update { name, ...?, description? }` updates an existing template.
> Only provided fields are changed: supplying a definition replaces the stored bundle and its provenance, omitting one leaves them unchanged, and omitting `description` leaves the stored description unchanged.
> To clear a description, pass `description: null`.
> A supplied definition is checked and stored as for [template.create](rule:i/template.create), and is not evaluated at update time.
> Returns `not_found` if no template with the given name exists.
> The template's `created_at` timestamp is not modified by update.
> Apps already instantiated from the template are unaffected; the template's bundle is copied at instantiation time and has no link to later edits.
> On success a `TemplateUpdated` event is emitted.

> i[template.remove]
> `/templates/remove { name }` deletes a template.
> Instantiated apps derived from the template are unaffected, since their bundle was copied at instantiation time.
> Returns `not_found` if no such template exists.
> On success a `TemplateRemoved` event is emitted.

> i[template.preview]
> `/templates/preview { name?, ... }` evaluates a template definition and returns a read-only summary of what it declares, without storing or instantiating anything.
> Exactly one of `name` (an existing template) or a definition must be supplied.
> The response contains:
>
> - `resources`: array of objects with fields `name`, `type`, and `def` — the same shapes defined in [app.describe](rule:i/app.describe). Instance state and faults are not included because a template has none.
> - `params`: array of objects with fields `name`, `kind`, `required`, `description`, `default_value`, and `secret`, matching the param schema defined in [app.describe](rule:i/app.describe).
> - `actions`: array of objects with fields `name`, `description`, `kind`, `params`, and `schedules`, matching the shape defined in [app.describe](rule:i/app.describe). For a template preview the timing fields of each schedule (`last_fired_at`, `next_fire_at`) are `null`, because the template is not associated with any registered app.
> - `script_error`: a string describing a script evaluation failure, or `null` when evaluation succeeded. When `script_error` is non-null the other fields reflect whatever partial state the engine produced before the error.

> i[template.instantiate]
> `/templates/instantiate { template, app }` creates a new app whose definition is a verbatim copy of the named template's bundle, with the template's provenance.
> The behaviour is equivalent to calling [app.register](rule:i/app.register) with the template's bundle and `app` as the name: the app starts in the `NotInstalled` state and an `AppRegistered` event is emitted.
> In addition a `TemplateInstantiated` event is emitted referencing both the template name and the new app name.
> After instantiation the app's definition is independent of the template: subsequent `/templates/remove` or any future template editing has no effect on the app.
> Returns `not_found` if the template does not exist; `requirements_invalid` if the app name is invalid or already in use; `unsupported_seedling` if the running Seedling does not satisfy the template's Seedling version requirement; `script_error` if the template's script fails to evaluate.
