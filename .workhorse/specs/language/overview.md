---
id: BSL
---

# BES Seedling Language

Applies to BES Seedling Language (BSL), the language in which an [app](../overview.md#app)'s definition is written, and to the rules every part of it shares.

## Terminology

### Script

The BSL source text that defines an app, formed from the script files of its [definition bundle](../overview.md#definition-bundle) as [BND](bundle.md) specifies.

### Resource

A single declared thing an app is made of, such as a deployment, service, ingress or volume.
A named resource is identified by its resource type and its name together.

### Resource type

The kind of a [resource](#resource), named in a script by a variant of [`ResourceType`](constants.md#resourcetype).

### Resource graph

The set of [resources](#resource), each with its configuration, that one evaluation of an app's [script](#script) declares from a given set of parameter values.

### Builder

A value whose builder methods configure one instance of its type piece by piece.

### Builder method

A method of a [builder](#builder) that changes the instance and returns the builder.

### Instance method

A method of a [builder](#builder) that creates a value of a different type from the builder's current state.

### Script error

An exception raised while a [script](#script) is evaluated.
A method that throws raises a script error.

## Evaluation

A script is written in [Rhai](https://rhai.rs/book/).
The runtime MUST evaluate a script as Rhai, with the types, functions, constants and variables the BSL specifications define in its scope.

The runtime MUST evaluate each script in a scope of its own.
A variable or value set by one evaluation MUST NOT be visible to another.

A script MAY catch a script error with Rhai's `try`/`catch` statement and continue.
If a script error propagates to the top level of the script, the evaluation MUST fail, and the runtime MUST NOT execute any further part of the script in that evaluation.

> [!NOTE]
> A script cannot catch the error raised when evaluation reaches one of the [engine limits](../runtime/engine-limits.md).

## Arguments

A function or method MUST raise a script error when an argument's type or value does not match its documented signature.
It MUST NOT coerce, default or ignore malformed input in place of raising the error.

In particular, an argument of the wrong type MUST NOT be treated as an empty string or as a default.
An array element of the wrong type MUST NOT be dropped from the array.
An unrecognised key in an options map MUST NOT be skipped.
An out-of-range number MUST NOT be truncated or wrapped into range.

The error message MUST name the argument as the script author wrote it, the type or range expected, and the type or value supplied.

Where a specification defines a coercion for an argument, such as [`col`](collection.md#coercion), that coercion applies in place of these rules.

## Names

Unless a specification defines other rules for a particular name, a name MUST be between 3 and 63 characters long inclusive, and consist of ASCII letters, ASCII digits and hyphens.
Its first character MUST be a letter, and its last character MUST be a letter or digit.
These rules are those of the regular expression `^[a-zA-Z][a-zA-Z0-9-]{1,61}[a-zA-Z0-9]$`.

A function or method given a name that breaks these rules MUST throw.
Names MUST be compared case-sensitively.

## Ports

Unless a specification defines other rules for a particular port, a port number MUST be an integer from 1 to 65535 inclusive.
A function or method given a port number outside that range MUST throw.

## Builders

A builder method MUST return the builder it was called on, so that a script can chain calls.

## Enums

A BSL enum is two things under one name in a script's scope: an opaque type, which names the enum in signatures, and a constant object map.
The map's keys MUST be the enum's variant names, and its values MUST be the opaque values of the enum's type.

## Descriptions

Every resource type MUST offer a `description(text: string)` builder method, which sets a free-form, human-readable description of the resource.
A resource MUST have no description until a script sets one.
Calling `description` again on the same resource MUST replace the previous description.

The runtime MUST report a resource's description to operators alongside the resource.
A description MUST NOT affect how the runtime treats the resource.

> [!NOTE]
> A description names the purpose of a resource that otherwise has no name, such as an anonymous job started inside an action closure, so that an operator can recognise it in operation logs.
