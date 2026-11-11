---
id: COL
---

# Collections

## Collection

<!-- from language: Collection -->

> l[collection.interface]
> A `Collection` is an abstract interface of things that can be zero or more Resources.
> Workload control methods often operate on or with Collections.
> Collections can hold different resource types, and can hold Collections.
> Order within a collection is not defined.
>
> An array of Collections is a Collection of the contents.
>
> All Resources are a Collection of the resource itself and all resources that are contained (not references) in it.

> l[collection.one]
> `col.one()` is a method which returns any one Resource from the collection (or null if the collection is empty).
>
> This is most useful for collections which have zero or one resources within.

> l[collection.only]
> `col.only(other: Collection)` is a method which returns a Collection of all resources in this Collection that are present in the `other` collection.

> l[collection.except]
> `col.except(other: Collection)` is a method which returns a Collection of all resources in this Collection that are _not_ present in the `other` collection.

> l[collection.select]
> `col.select(criterion: object)` is a method which returns a Collection of all the resources within this Collection that match the `criterion`.
>
> The criterion is an object map where all keys are optional. Resources must match all keys to be selected. All possible keys are defined in this spec.

> l[collection.select.types]
> `types`: must match one of an array of [ResourceTypes](rule:l/const.resource-type.enum).

> l[collection.select.names]
> `names`: must match one of an array of resource names

> l[collection.select.name-patterns]
> `name_patterns`: must match at least one glob pattern

> l[collection.col]
> `col(val)` is a free function that coerces any value into a `Collection`.
>
> The following values are accepted:
> - A `Collection` — returned as-is.
> - An `App` — yields a Collection of all its named resources.
> - Any named Resource (Deployment, Service, HttpService, Job, Ingress, named Volume, ExternalVolume) — yields a Collection of that single resource.
> - An array — yields a Collection of all elements coerced the same way (a union).
> - An anonymous Volume (without a name) — yields an empty Collection.
> - Any other value — yields an empty Collection.
>
> Action handles are not coercible to a Collection: `col(action)` yields an empty Collection, because actions are invoked via [`Action.invoke()`](rule:l/action.call), not scheduled as resources.
