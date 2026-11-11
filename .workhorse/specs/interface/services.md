---
id: SVCM
---

# Service methods

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an operator manages [site services](../runtime/site-resources.md#site-service), lists the services apps declare, and maps apps' [external services](../language/service.md#external-service).

## Borrowed terms

| term | meaning |
| --- | --- |
| IPv6 text form | the textual representation of an IPv6 address, as defined in [RFC 4291](https://www.rfc-editor.org/rfc/rfc4291) section 2.2 |

## Service targets

A service target names the service an external service mapping routes to.
It is a JSON object whose `kind` member selects its other members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `kind` | string | yes | `"site"` or `"app"` |
| `name` | string | with `kind` `"site"` | the site service's name |
| `app` | string | with `kind` `"app"` | the name of the app declaring the service |
| `service` | string | with `kind` `"app"` | the service's name, as the app declares it |

The server MUST refuse a service target whose `kind` is neither value, or which lacks a member its `kind` requires, with `requirements_invalid`.
The server MUST refuse a service target whose `name`, `app` or `service` breaks the [name rules](../language/overview.md#names) with `requirements_invalid`.

A service target exists when its `kind` is `"site"` and a site service named `name` exists, or when its `kind` is `"app"`, an app named `app` is registered, and that app's current definition declares a service named `service`.

## Endpoint objects

An endpoint object carries the members of a [site service endpoint](../runtime/site-resources.md#site-services).

| member | type | required | meaning |
| --- | --- | --- | --- |
| `service_port` | integer | yes | the port on which the site service exposes the endpoint, from 1 to 65535 |
| `protocol` | string | yes | `"tcp"`, `"udp"` or `"http"` |
| `remote_host` | string | yes | the address traffic is forwarded to |
| `remote_port` | integer | yes | the port traffic is forwarded to, from 1 to 65535 |

An endpoint is identified by all four members together.

`remote_host` MUST be one of:

- an IPv6 address in the IPv6 text form, with no enclosing brackets and no zone index;
- an IPv4 address as four decimal numbers from 0 to 255 separated by dots, none with a leading zero;
- a DNS name.

A DNS name, for `remote_host`, is a string of at most 253 characters made of labels separated by single dots, with no leading or trailing dot.
Each label MUST be 1 to 63 characters of ASCII letters, ASCII digits and hyphens, and MUST NOT begin or end with a hyphen.
The name MUST contain at least one ASCII letter.
The name MUST NOT be `localhost`, compared case-insensitively.

> [!NOTE]
> An endpoint is reached from a pod's network, where `localhost` would name the pod and not the operator's machine.
> A name of digits and dots alone would be read as a malformed IPv4 address.

How the runtime routes each form of `remote_host` is specified in [endpoint addressing](../runtime/site-resources.md#endpoint-addressing).

The server MUST refuse an endpoint object that breaks any rule above with `requirements_invalid`, whose `message` names the offending member.

## Site service creation

`/services/site/create` creates a site service.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site service's name |
| `description` | string | no | a description of the site service for operators |
| `endpoints` | array of [endpoint objects](#endpoint-objects) | yes | the site service's endpoints |

The server MUST refuse a `name` that breaks the [name rules](../language/overview.md#names) with `requirements_invalid`.
The server MUST refuse a `name` under which a site service exists with `requirements_invalid`.
The server MUST refuse an absent or empty `endpoints` with `requirements_invalid`, since a site service has [one or more endpoints](../runtime/site-resources.md#site-services).
The server MUST refuse `endpoints` holding two identical endpoints with `requirements_invalid`.

The runtime MUST store the site service and all its endpoints in one step, so that a refused or failed request leaves none of them stored.

On success the result is `{ "created": true }`.
The server MUST then emit a `SiteServiceCreated` [event](events.md#site-resource-events), followed by one `SiteServiceEndpointAdded` event for each endpoint.
Each event carries the request's [actor](wire.md#actor-attribution), and the [audit log](../runtime/audit-log.md#entries) records each.

## Site service listing

`/services/site/list` takes no parameters.
Its result is an array with one record for each site service, ordered by name in ascending byte order.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the site service's name |
| `description` | string | the site service's description; absent where it has none |
| `created_at` | string | when the site service was created, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `endpoints` | array of [endpoint objects](#endpoint-objects) | the site service's endpoints, ordered by `service_port`, then `protocol`, `remote_host` and `remote_port`, each in ascending order |

The method emits no event and records no audit entry.

## Site service deletion

`/services/site/delete` deletes a site service and its endpoints.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site service's name |

The server MUST refuse a `name` that breaks the name rules with `requirements_invalid`.
The server MUST refuse a `name` under which no site service exists with `not_found`.
The server MUST refuse the request with `requirements_invalid` while an external service mapping targets the site service, as [site service lifecycle](../runtime/site-resources.md#site-service-lifecycle) requires, and its `message` MUST name such a mapping's app and external name.
The runtime MUST check for mappings and delete the site service as one indivisible step, so that no mapping is left targeting a deleted site service.

On success the result is `{ "deleted": true }`.
The server MUST then emit a `SiteServiceDeleted` [event](events.md#site-resource-events) carrying the request's actor, which the audit log records.

## Site service endpoint changes

`/services/site/endpoint/add` adds an endpoint to a site service, and `/services/site/endpoint/remove` removes one.
Both take the members of an [endpoint object](#endpoint-objects) together with the site service's name.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site service's name |
| `service_port` | integer | yes | as in an endpoint object |
| `protocol` | string | yes | as in an endpoint object |
| `remote_host` | string | yes | as in an endpoint object |
| `remote_port` | integer | yes | as in an endpoint object |

For either method, the server MUST refuse a `name` that breaks the name rules with `requirements_invalid`, and a `name` under which no site service exists with `not_found`.

For `/services/site/endpoint/add`, the server MUST refuse an endpoint that breaks the [endpoint object](#endpoint-objects) rules with `requirements_invalid`.
The server MUST refuse an endpoint the site service already has with `requirements_invalid`.
On success the result is `{ "added": true }`.
The server MUST then emit a `SiteServiceEndpointAdded` [event](events.md#site-resource-events) carrying the request's actor, which the audit log records.

For `/services/site/endpoint/remove`, the server MUST refuse an endpoint the site service does not have with `not_found`.
The server MUST refuse to remove a site service's only endpoint with `requirements_invalid`, since a site service has one or more endpoints.
On success the result is `{ "removed": true }`.
The server MUST then emit a `SiteServiceEndpointRemoved` event carrying the request's actor, which the audit log records.

## Resolver status

`/services/site/resolver-status` reports the runtime's resolution of the DNS names that site service endpoints give as their `remote_host`.
It takes no parameters.
Its result is `{ "entries": [ ... ] }`, with one entry for each such DNS name that the runtime has looked up at least once.

| member | type | meaning |
| --- | --- | --- |
| `host` | string | the DNS name |
| `aaaa` | array of strings | the IPv6 addresses the runtime holds for the name from AAAA records, each in the IPv6 text form |
| `a` | array of strings | the IPv4 addresses the runtime holds for the name from A records, each in dotted decimal form |
| `last_attempt_failed` | boolean | `true` where the most recent lookup of the name failed for both address families |
| `degraded_family` | string or null | `"AAAA"` or `"A"` where the most recent lookup failed for that family and returned records for the other, and `null` otherwise |
| `age_seconds` | integer | the whole seconds since the most recent lookup of the name |
| `ttl_remaining_seconds` | integer | the whole seconds until the runtime's answer for the name expires and it looks the name up again |

Where `degraded_family` names a family, that family's addresses are its last answer, as [partial DNS answers](../runtime/site-resources.md#partial-dns-answers) specifies.
A DNS name that an endpoint gives and that has no entry has not yet been looked up.

The method emits no event and records no audit entry.

## App service listing

`/services/app/list` lists every named service the registered apps declare, and `/services/exported/list` lists those of them that are [exported](../language/service.md#export).
Neither takes parameters.

The result of each is an array with one record for each service in the current definition of each registered app, ordered by app name and then by service name, each in ascending byte order.
The result of `/services/exported/list` MUST hold only exported services.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the name of the app declaring the service |
| `service_name` | string | the service's name |
| `http` | boolean | whether the service is an [HTTP service](../language/http-service.md#http-service) |
| `exported` | boolean | whether the service is exported; present in `/services/app/list` only |
| `description` | string | the service's export description; absent where the service is not exported or was exported with none |

Neither method emits an event or records an audit entry.

## Mapping creation

`/services/external/map` maps an app's external service to a target.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `external_name` | string | yes | the external service's name |
| `target` | [service target](#service-targets) | yes | the service to route the external service to |

The server MUST refuse an `app` or `external_name` that breaks the name rules with `requirements_invalid`.
The server MUST refuse an `app` that is not registered with `not_found`.
The server MUST refuse the request with `requirements_invalid` if the app's external service of that name is already mapped.
The server MUST refuse a `target` that does not [exist](#service-targets) with `requirements_invalid`.

On success the result is `{ "mapped": true }`.
The server MUST then emit an `ExternalServiceMapped` [event](events.md#site-resource-events) carrying the request's actor, which the audit log records.
The runtime routes the external service's traffic to the target as [external services](../language/service.md#external-services) specifies.

## Remapping

`/services/external/remap` replaces the target of an existing mapping.
It takes the parameters of [mapping creation](#mapping-creation).

The server MUST refuse an `app` or `external_name` that breaks the name rules with `requirements_invalid`.
The server MUST refuse an `app` that is not registered with `not_found`.
The server MUST refuse the request with `not_found` if the app's external service of that name is not mapped.
The server MUST refuse a `target` that does not exist with `requirements_invalid`.
The runtime MUST read the previous target and store the new one as one indivisible step.

On success the result is `{ "remapped": true }`.
The server MUST then emit an `ExternalServiceRemapped` [event](events.md#site-resource-events) carrying the new target, the previous target and the request's actor, which the audit log records.

## Unmapping

`/services/external/unmap` removes a mapping.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `external_name` | string | yes | the external service's name |

The server MUST refuse an `app` or `external_name` that breaks the name rules with `requirements_invalid`.
The server MUST refuse the request with `requirements_invalid` while the app's current definition declares an external service named `external_name`.
The server MUST refuse the request with `not_found` if the app is not registered, or its external service of that name is not mapped.

On success the result is `{ "unmapped": true }`.
The server MUST then emit an `ExternalServiceUnmapped` [event](events.md#site-resource-events) carrying the request's actor, which the audit log records.

> [!NOTE]
> An operator retargets the mapping of an external service the app declares with [remapping](#remapping).
> An app's mappings are discarded when it is [deregistered](../runtime/app-teardown.md#deregistration).

## Mapping listing

`/services/external/list` lists external service mappings.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's mappings |

The server MUST refuse an `app` that breaks the name rules with `requirements_invalid`.
The result is an array with one record for each mapping, ordered by app name and then by external name, each in ascending byte order.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the app's name |
| `external_name` | string | the external service's name |
| `target` | [service target](#service-targets) | the service the external service is mapped to |

The method emits no event and records no audit entry.

## Declared external services

`/services/external/declared` takes no parameters.
Its result is an array with one record for each external service in the current definition of each registered app, whether mapped or not, ordered by app name and then by external service name, each in ascending byte order.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | the name of the app declaring the external service |
| `name` | string | the external service's name |
| `description` | string | the external service's [description](../language/overview.md#descriptions); absent where it has none |

The method emits no event and records no audit entry.
