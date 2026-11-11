---
id: APP
---

# The app global

Applies to the `app` variable, through which a [script](overview.md#script) declares an [app](../overview.md#app)'s [resources](overview.md#resource) and describes the app.

## Terminology

### Static context

Evaluation of a script outside every closure the runtime runs as a [lifecycle operation](../overview.md#lifecycle-operation).

### Action context

Evaluation of a script inside a closure the runtime runs as a lifecycle operation, such as an [action closure](rule:l/action.type) or a [parameter change handler](parameter.md#change-handlers).

### Static resource

A resource declared in the static context.

### Dynamic resource

A resource created in the action context.

### Anonymous resource

A resource created without a name.

## The app variable

The runtime MUST make the global variable `app`, of type `App`, available throughout a script, at the top level and within every closure.
BSL MUST NOT offer a script any way to construct an `App`.

## Named resources

In the static context, a method of `app` that takes a resource's name, such as `app.deployment(name)`, MUST declare the named resource of that method's [resource type](overview.md#resource-type) and return a handle to it.
Calls that give the same resource type and name MUST return handles to the same resource.
A resource of one type MAY share its name with a resource of another type.

In the action context, the same method MUST return a reference to the static resource of that resource type and name, and MUST NOT declare a resource.
It MUST throw if no static resource of that resource type and name exists.
[`app.external_volume`](rule:l/volume.external.dynamic) behaves differently in the action context.

## Static resource immutability

In the action context, calling a builder method on a static resource MUST throw, whether the script obtained the handle within the closure or captured it from the static context.
This applies to every builder method of every resource type.

## Anonymous resources

A script MAY create an anonymous resource only in the action context, by calling `app.deployment()`, `app.job()`, `app.service()` or `app.volume()` with no arguments.
Calling any of these with no arguments in the static context MUST throw.
An anonymous resource remains open to its builder methods for the rest of the lifecycle operation that created it.

## Dynamic resource lifetime

A dynamic resource MUST exist only for the lifecycle operation that created it.
The runtime MUST clean up every dynamic resource when that lifecycle operation ends.

## App description

`app.description(text: string)` MUST set a free-form, human-readable description of the app, and MUST return `app`.
An app MUST have no description until a script sets one.
Calling `app.description` again MUST replace the previous description.

The runtime MUST report the app's description to operators alongside its name.
The description MUST NOT affect how the runtime treats the app.

## Canopy type

`app.canopy_type(type: string)` MUST declare the Canopy application type of the app, such as `tamanu-central`, and MUST return `app`.
Calling `app.canopy_type` again MUST replace the previous type.
A definition that never calls it declares no Canopy type.
How the runtime reports an app with a Canopy type is specified in [CNR](../runtime/canopy-reporting.md).

A Canopy type MUST consist of one or more segments joined by hyphens, each segment one or more lowercase ASCII letters and digits, beginning with a letter, as the regular expression `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` gives.
`app.canopy_type` MUST throw when given any other value.

> [!NOTE]
> Canopy refuses a whole report when one application in it has a type Canopy does not accept, so a malformed type would stop the host's report for every app on it.

`app.canopy_type` MUST throw when called in the action context.
