---
id: TPL
---

# Templates

Applies to the templates a [host](../overview.md#host) stores for creating [apps](../overview.md#app), and to the [Operator Interface](../overview.md#operator-interface) methods that manage them.

## Terminology

### Template

A named [definition bundle](../overview.md#definition-bundle), with its [provenance](definitions.md#provenance-reporting), stored on a host for creating apps from.

### Supported template

A template whose bundle declares a [Seedling version requirement](../language/bundle.md#seedling-version-requirement) that the running Seedling satisfies, or declares none.

## Templates and apps

The [runtime](../overview.md#runtime) MUST NOT reconcile a template.
It MUST NOT record a [generation](../overview.md#generation), parameter values or [faults](../overview.md#fault) for a template.

An app instantiated from a template MUST hold its own copy of the template's bundle.
Updating or removing a template MUST NOT change any app instantiated from it.

## Template names

A template name MUST follow the rules for app names in [names](../language/overview.md#names).
The runtime MUST refuse a request naming a template by an invalid name with `requirements_invalid`.

Template names and app names are separate namespaces.
A template MAY have the same name as an app.

## Template definitions

A request that supplies a template definition MUST carry it as a request supplying an [app definition](definitions.md#definition-sources) does, with the script text in a member named `body` in place of `script`.

The runtime MUST resolve a template definition as it resolves an app definition, fetching a `reference` and decoding a `bundle`.
The runtime MUST refuse a definition that cannot be fetched or decoded with the error that [app definitions](definitions.md#definition-sources) give for that refusal, and store nothing.

The runtime MUST NOT refuse a resolved definition because the running Seedling does not satisfy its Seedling version requirement.
For a definition whose template would be a [supported template](#supported-template), the runtime MUST apply every [bundle rule](definitions.md#bundle-rules).
For any other definition, it MUST apply the bundle rules on size, file count, file types and paths, and on the readability of the version requirement, and MUST NOT apply the rules on the other metadata fields and the script files.

> [!NOTE]
> A bundle written for a later Seedling may use metadata fields the running Seedling does not know, so only a Seedling that supports it can judge its metadata.

The runtime MUST NOT evaluate a template definition when storing it.
A definition whose script fails to evaluate is stored, and the failure surfaces on [preview](#preview) and [instantiation](#instantiation).

## Persistence

The runtime MUST store templates durably, so that a stored template survives a restart of the runtime.
The runtime MUST store a removal durably, so that a removed template does not reappear after a restart.

## Creation

`/templates/create` stores a new template.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the template's name |
| `body`, `bundle`, `reference`, `origin` | as in [Template definitions](#template-definitions) | one of `body`, `bundle` and `reference` | the template's definition |
| `description` | string or null | no | a description for operators |

The result is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the template's name |
| `created_at` | string | when the template was stored, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |

If a template with the given name already exists, the runtime MUST refuse the request with `requirements_invalid` and store nothing.
On success, the runtime MUST emit a `TemplateCreated` event.

## Listing

`/templates/list` takes no parameters.
Its result is an array with one object per stored template, ordered by name in ascending byte order.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the template's name |
| `description` | string or null | the template's description |
| `created_at` | string | when the template was stored, as an RFC 3339 timestamp |
| `supported` | boolean | whether the template is a [supported template](#supported-template) |

## Inspection

`/templates/show` takes `name`, a template name, and returns one object.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the template's name |
| `body` | string | the template's script files concatenated as the [bundle rules](../language/bundle.md#script-formation) define |
| `provenance` | object | the template definition's [provenance](definitions.md#provenance-reporting) |
| `supported` | boolean | whether the template is a supported template |
| `description` | string or null | the template's description |
| `created_at` | string | when the template was stored, as an RFC 3339 timestamp |

If the running Seedling cannot read the template's script files, `body` MUST be the empty string.
If no template has the given name, the runtime MUST refuse the request with `not_found`.

## Update

`/templates/update` changes a stored template.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the template to change |
| `body`, `bundle`, `reference`, `origin` | as in [Template definitions](#template-definitions) | no | a replacement definition |
| `description` | string or null | no | a replacement description |

A supplied definition MUST replace the stored bundle and its provenance.
The runtime MUST check a supplied definition as for [creation](#creation), and MUST NOT evaluate it.
If no definition is supplied, the stored bundle and provenance MUST be left unchanged.
The runtime MUST refuse an update that supplies `origin` without a definition, with `requirements_invalid`.

A `description` that is a string MUST replace the stored description, and a `description` of `null` MUST clear it.
If `description` is absent, the stored description MUST be left unchanged.

An update MUST NOT change the template's `created_at`.

The result is `{ "updated": true }`.
If no template has the given name, the runtime MUST refuse the request with `not_found`.
If the runtime refuses the request, it MUST leave the template unchanged.
On success, the runtime MUST emit a `TemplateUpdated` event.

## Removal

`/templates/remove` takes `name`, a template name, and deletes that template.
The result is `{ "removed": true }`.
If no template has the given name, the runtime MUST refuse the request with `not_found`.
On success, the runtime MUST emit a `TemplateRemoved` event.

## Preview

`/templates/preview` evaluates a template definition and describes the [resources](../overview.md#resource), parameters and actions it declares.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | no | a stored template to preview |
| `body`, `bundle`, `reference`, `origin` | as in [Template definitions](#template-definitions) | no | a definition to preview |

A request MUST supply exactly one of `name` and a definition.
The runtime MUST refuse a request supplying both, or neither, with `requirements_invalid`.
If `name` names no stored template, the runtime MUST refuse the request with `not_found`.
The runtime MUST refuse a supplied definition that cannot be fetched or decoded as for [creation](#creation).

The runtime MUST evaluate the definition with no parameter values set.
It MUST NOT store or register anything.

The result is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `description` | string or null | the description the definition declares for its app |
| `resources` | array | one object per declared resource |
| `params` | array | one object per declared parameter |
| `actions` | array | one object per declared action, shell and install action |
| `script_error` | string or null | why evaluation failed, or `null` if it succeeded |

A `resources` entry MUST carry `name`, `type` and `def` as an entry of the `resources` member of [app description](apps.md#description) does.
A deployment's entry MUST also carry `scale`, an object whose `low` and `high` are the declared scale bounds, and `priority`, the deployment's priority.
An exported volume's entry MUST also carry `export`, an object whose `exported` is `true` and whose `description`, present only if the volume declares one, is the export description.

A `params` entry MUST carry `name`, `kind`, `required`, `description`, `default_value` and `secret`, as the parameter schema of app description does.

An `actions` entry MUST carry `name`, `description`, `kind`, `params` and `schedules` as an entry of the `actions` member of app description does.
In each schedule, `last_fired_at` and `next_fire_at` MUST be `null`.

The runtime MUST report in `script_error` any failure to evaluate the definition, including a bundle whose metadata or script files the running Seedling cannot read.
When `script_error` is not `null`, the other members MUST describe the declarations the evaluation made before it failed.

## Instantiation

`/templates/instantiate` creates an app from a template.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `template` | string | yes | the template to instantiate |
| `app` | string | yes | the name of the new app |

The runtime MUST register the app as [app registration](apps.md#registration) does, with a copy of the template's bundle as its definition and the template's provenance as the definition's provenance.

If no template has the name `template`, the runtime MUST refuse the request with `not_found`.
If `app` is not a valid app name, is reserved, or names a registered app, the runtime MUST refuse the request with `requirements_invalid`.
If the template is not a supported template, the runtime MUST refuse the request with `unsupported_seedling`.
If the template's bundle breaks a bundle rule, the runtime MUST refuse the request with `bundle_invalid`.

> [!NOTE]
> A template stored while unsupported was not held to the metadata and script-file rules, so it can break them once a later Seedling supports it.

The result is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the new app's name |
| `generation` | integer | the new app's generation |

On success, the runtime MUST emit the `AppRegistered` event that registration emits, followed by a `TemplateInstantiated` event.

## Events

The runtime MUST emit these events on the [event feed](events.md#event-types), each carrying the members every event carries and the members below.

| `type` | members |
| --- | --- |
| `TemplateCreated` | `name` |
| `TemplateUpdated` | `name` |
| `TemplateRemoved` | `name` |
| `TemplateInstantiated` | `template`, `app` |
