---
id: COL
---

# Collections

Applies to the `Collection` type, through which a script gathers [resources](overview.md#resource) to act on, and to the `col` function that forms one from any value.

## Terminology

### Collection

A value of type `Collection`: a group of zero or more resources, of any [resource types](overview.md#resource-type), in no defined order.

## Coercion

`col(value)` is a function that MUST accept a value of any type and return a collection, as the table gives.

| value | collection returned |
| --- | --- |
| a `Collection` | that collection |
| an `App` | every resource of the app's [resource graph](overview.md#resource-graph) |
| a named `Deployment`, `Job`, `Service`, `Ingress`, `Volume` or `ExternalVolume` | that resource alone |
| an anonymous `Deployment`, `Job` or `Service` | that resource alone |
| an `HttpService` | the service it is a view of, alone |
| an array | every resource in the collections its elements coerce to |
| an anonymous `Volume` | an empty collection |
| any other value | an empty collection |

An action handle is among the values that coerce to an empty collection.

> [!NOTE]
> An action is invoked through [`Action.invoke`](rule:l/action.call), not acted on as a resource, so no collection holds one.

## Query methods

A `Collection` MUST offer the methods `one`, `only`, `except` and `select`.
An `App` MUST offer the same four methods, with the result each gives on `col(app)`.

`collection.one()` MUST return one resource from the collection, or the unit value `()` if the collection is empty.

> [!NOTE]
> `one` suits a collection known to hold at most one resource, since the collection's order does not decide which resource it returns.

`collection.only(other)` MUST return a collection of every resource in `collection` that is also in `col(other)`.
`collection.except(other)` MUST return a collection of every resource in `collection` that is not in `col(other)`.
For both, a resource is in `col(other)` when a resource of the same resource type and name is.

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
