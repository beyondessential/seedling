---
id: BSL
---

# BES Seedling Language

## Preamble

<!-- from language: None -->

BES Seedling Language, or BSL for short, is a DSL (Domain Specific Language).
It is used to define and manage an application running on a Seedling node in an autonomous way and provide administrative controls to operators.

The terminology used in BSL closely resembles that used for [Kubernetes](https://kubernetes.io), but some of the semantics are different.

Absent specification bugs, anything that is not defined here is either defined in another spec document (e.g. the control plane, the runtime), or is implicitly not allowed.

> l[bsl.syntax]
> BSL is written in [Rhai](https://rhai.rs).

> l[bsl.script]
> A BSL script is a single code listing that defines a Seedling Application.

> l[bsl.scope]
> The runtime must use a distinct [Rhai Scope](https://rhai.rs/book/engine/scope.html) for each BSL script.

> l[bsl.enums]
> Enums are not a native Rhai feature. In BSL, enums are defined as being two things simultaneously at the same name:
>
> - an opaque type, used to describe the enum in type signatures
> - a constant object map, with its keys being the names of the enum variants, and its values being opaque values of the opaque enum type.
>
> All enums available to BSL are defined in the [Constants](#constants) section.

> l[bsl.args.strict]
> A builder or function argument whose type or value does not match its documented signature raises a script error at evaluation time.
> Malformed input is never coerced, defaulted, or silently ignored: an argument of the wrong type does not become an empty string or a default, an element of the wrong type does not disappear from an array, an unrecognised key in an options map is not skipped, and an out-of-range number is not truncated or wrapped into range.
> The error names the argument as the script author wrote it, what was expected, and what was supplied.
> This does not apply where the spec defines a coercion, such as [col](rule:l/collection.col).

> l[bsl.errors]
> Some methods throw exceptions under some circumstances.
> The `try..catch` Rhai construct may be used to handle those exceptions and recover.
> If an exception bubbles to the top of the script, execution is considered failed and will not proceed further.
> Responding to this is a control plane concern and not defined in this spec.

> l[bsl.builder]
> Some methods and functions return "builders", which are types that have further methods which configure one instance of the type piece by piece, rather than all at once.
>
> They have **builder methods** which modify the instance and return the builder for chaining, and may have **instance methods** which create different types from the builder state.

> l[bsl.name]
> Various methods and resources are defined using a `name`.
>
> Unless otherwise specified, names are ASCII alphanumeric with hyphens, must not start with a number, must not start nor end with a hyphen, and must be between 3 and 63 characters long inclusive.
> Names are case-sensitive.
>
> The regular expression `^[a-zA-Z][a-zA-Z0-9-]{1,60}[a-zA-Z0-9]$` may be used to validate a name.

> l[bsl.port]
> Various methods and resources use port numbers.
>
> Unless otherwise specified, a port number must be a non-zero positive integer below 65536.
> If an invalid port is provided, the method must throw.

> l[bsl.resource]
> The term Resource is using a similar definition [as Kubernetes](https://kubernetes.io/docs/reference/using-api/api-concepts/#standard-api-terminology):
> - A _resource type_ is the name used in the spec (Service, Deployment, Job)
> - A list of instances of a resource type is known as a _collection_
> - A single instance of a resource type is called a _resource_

> l[bsl.resource.description]
> Every Resource has a `description(text: string)` builder method which attaches a free-form human-readable description to the resource. The description is surfaced to operators in the management interfaces (CLI, web UI, OI), and is particularly useful for naming the purpose of anonymous resources (such as anonymous jobs spawned inside an action closure) so they are recognisable in operation logs.
>
> The description is purely informational and has no behavioural effect. Calling `description()` more than once on the same resource replaces the previous value. The description is `None` by default.
>
> The method returns the same resource builder so calls may be chained.
