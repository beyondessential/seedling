---
id: COL
---

# Collections

Applies to the `Collection` type, through which a script gathers [resources](overview.md#resource) to act on, and to the `col` function that forms one from any value.

## Terminology

### Collection

A value of type `Collection`: a group of zero or more resources, of any [resource types](overview.md#resource-type), in no defined order.

## Membership

A collection MUST hold each resource at most once, however many times it was gathered.
Two named resources are the same resource when they share a resource type and name.
An anonymous resource is the same resource only as itself, by the [identity](../runtime/identity.md#anonymous-instances) it holds for its lifetime.

A collection MUST be resolved each time it is used, against the app's resources at that moment.
A collection formed from `col(app)` before a resource is declared therefore holds that resource when it is used afterwards.

> [!NOTE]
> A script commonly gathers `col(app)` once at its top level and uses it in action closures, where it is meant to cover every resource the script declares.

## Coercion

`col(value)` is a function that MUST accept a value of any type and return a collection, as the table gives.

| value | collection returned |
| --- | --- |
| a `Collection` | that collection |
| an `App` | every named resource of the app |
| a named `Deployment`, `Job`, `Service`, `Ingress`, `Volume`, `ExternalVolume` or `ExternalService` | that resource alone |
| an anonymous `Deployment`, `Job` or `Service` | that resource alone |
| an `HttpService` | the service it is a view of, alone |
| an array | every resource in the collections its elements coerce to |
| an anonymous `Volume` | an empty collection |
| any other value | an empty collection |

An action handle is among the values that coerce to an empty collection.

> [!NOTE]
> An action is invoked through [`Action.invoke`](action.md#action-invocation), not acted on as a resource, so no collection holds one.
>
> An anonymous volume belongs to the pod that mounts it, and is set up and cleaned up with that pod, so no collection holds one either.

## Query methods

A `Collection` MUST offer the methods `one`, `only`, `except` and `select`.
An `App` MUST offer the same four methods, with the result each gives on `col(app)`.

`collection.one()` MUST return one resource from the collection, or the unit value `()` if the collection is empty.

> [!NOTE]
> `one` suits a collection known to hold at most one resource, since the collection's order does not decide which resource it returns.

`collection.only(other)` MUST return a collection of every resource in `collection` that is also in `col(other)`.
`collection.except(other)` MUST return a collection of every resource in `collection` that is not in `col(other)`.
For both, a resource is in `col(other)` when the [same resource](#membership) is.

## Selection

`collection.select(criterion: map)` MUST return a collection of every resource in `collection` that matches `criterion`.
A resource matches a criterion when it matches every key the criterion holds, so an empty criterion matches every resource.

| key | type | a resource matches when |
| --- | --- | --- |
| `types` | array of `ResourceType` | its resource type is one in the array |
| `names` | array of strings | its name is one in the array |
| `name_patterns` | array of strings | its name matches at least one of the patterns |

`select` MUST throw if `criterion` holds any key other than these.

A name pattern MUST match a resource's whole name, case-sensitively.
In a pattern, `*` matches any sequence of zero or more characters, `?` matches exactly one character, and every other character matches itself.
