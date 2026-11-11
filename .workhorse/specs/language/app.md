---
id: APP
---

# The app global

## App global

<!-- from language: App global -->

> l[app.var]
> `app` is a global variable available to every BSL script at the top level (and below).

> l[app.type]
> The `app` global variable is of type `App`.

> l[app.constructor]
> The `App` type is not constructible within a BSL.

> l[app.methods]
> All the methods of `app` are defined in this spec.
>
> Methods are either **resource methods**, which define resources, or **query methods**, which query the `App` state.

> l[app.resources]
> An `app` holds [Resources](rule:l/bsl.resource) defined against it.

> l[app.resources.static]
> Resources that are defined at the top level (outside of all actions) are said to be **static**.

> l[app.resources.dynamic]
> Resources that are not static are said to be **dynamic**.
> Dynamic resources are created inside action closures and exist only for the duration of the operation.
> They are cleaned up automatically when the action ends.

> l[app.resources.context.named]
> In the **static context** (top-level script), `app.resource(name)` creates and registers a named resource.
> In the **action context** (inside an action closure), `app.resource(name)` returns a **reference** to an existing static resource.
> If no static resource with that name exists, it is a script error.
> `app.external_volume(name)` is the one exception; see [volume.external.dynamic](rule:l/volume.external.dynamic).

> l[app.resources.context.immutable]
> Static resources are immutable in the action context.
> Calling a builder method on a static resource inside an action closure is a script error, regardless of how the resource handle was obtained — whether re-fetched via `app.resource(name)` inside the closure or captured from an outer-scope `let` binding.
> This applies to every builder method on every static resource type (e.g. `Volume.write`, `Service.exported`, `Deployment.healthcheck`, `Job.deadline`, `Ingress.tls`).
> Anonymous resources created inside the action context are mutable for the lifetime of the operation.

> l[app.resources.context.anonymous]
> Anonymous resources (those created without a name argument) may only be created in the action context.
> Attempting to create an anonymous resource at the top level is a script error.
> `Ingress` and `ExternalVolume` have no anonymous form in any context.

> l[app.resources.names]
> Most resources are defined with a name.
> If two methods use the _same name_, the methods return (a different handle to) the _same resource_.
>
> ```rhai
> let a = app.volume("data");
> let b = app.volume("data");
> // these are the same volume
> ```
>
> Names are also used to select resources using the Collection methods.
> Named resources created in the action context are references to existing static resources, not new definitions; see `l[app.resources.context.named]`.

> l[app.description]
> The `app.description(text: string)` method attaches a free-form human-readable description to the application as a whole. The description is surfaced to operators in the management interfaces (CLI, web UI, OI) alongside the app name.
>
> The description is purely informational and has no behavioural effect. Calling `description()` more than once on the app replaces the previous value. The description is `None` by default.
>
> The method returns `app` so calls may be chained.

> l[app.canopy-type]
> The `app.canopy_type(type: string)` method declares the Canopy application type the app is, such as `tamanu-central`.
> The runtime reports the app to Canopy as an application of that type, as described in [Canopy Reporting](runtime.md#canopy-reporting).
>
> A type is lowercase letters and digits in hyphen-separated segments, beginning with a letter; any other value throws.
> Canopy refuses any other type, and refuses a whole report when one application in it is malformed, so a type it would refuse would stop the host's report along with the app's.
> Calling `canopy_type()` more than once replaces the previous value.
> An app that never calls it has no Canopy type, and is not reported to Canopy as an application.
> Calling it inside an action closure is a script error: the type describes what the app is, not what one operation does.
>
> The method returns `app` so calls may be chained.
