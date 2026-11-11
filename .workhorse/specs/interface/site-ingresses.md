---
id: INGM
---

# Site ingress methods

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an operator manages the [site ingresses](../runtime/site-resources.md#site-ingress) of a [host](../overview.md#host), their [attachments](../runtime/site-resources.md#attachment), and the [discovery providers](../runtime/site-resources.md#discovery-provider) that maintain discovered site ingresses.

## Changes and attribution

The runtime MUST store each change a method below makes durably before the server answers the request or emits the change's event.
The server MUST attach the request's [actor](wire.md#actor-attribution) to every event a request emits.
Each event is recorded in the audit log as [AUD](../runtime/audit-log.md#entries) specifies, and a method that emits no event records no audit entry.

The server MUST refuse a request whose `name` breaks the [name rules](../language/overview.md#names) with `requirements_invalid`.

## Site ingress records

`/ingresses/site/list` and `/ingresses/site/show` report a site ingress as an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the site ingress's name |
| `hostname` | string | the hostname, in lower case |
| `description` | string | the description, absent where the site ingress has none |
| `source` | string | the site ingress's [source](../runtime/site-resources.md#sources), `"manual"` or `"discovered"` |
| `discovered_provider` | string | the name of the discovery provider that maintains the site ingress, present only where `source` is `"discovered"` |
| `discovered_key` | string | the provider key, present only where `source` is `"discovered"` |
| `tls_provider` | string | the [TLS provisioning mode](../runtime/site-resources.md#tls-provisioning-modes), `"acme"`, `"tailscale"`, `"internal"` or `"none"` |
| `stale` | boolean | whether the site ingress is stale |
| `created_at` | string | when the site ingress was created, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `attachments` | array of objects | the site ingress's attachments, ordered by port and then by protocol in ascending byte order |

Each member of `attachments` is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `port` | integer | the attachment's port |
| `protocol` | string | the attachment's [protocol](../runtime/site-resources.md#attachments), `"tcp"`, `"udp"`, `"http"` or `"http2"` |
| `target_kind` | string | `"forward"` or `"redirect"` |
| `target_app` | string | the app a forward target routes to, present only for a forward |
| `target_service` | string | the service of `target_app` a forward target routes to, present only for a forward |
| `redirect_url` | string | the URL a redirect target answers with, present only for a redirect |
| `redirect_code` | integer | the status code a redirect target answers with, present only for a redirect |
| `redirect_preserve_path` | boolean | whether a redirect target keeps the request's path, present only for a redirect |
| `created_at` | string | when the attachment was first made, as an RFC 3339 timestamp |

## Listing and inspection

`/ingresses/site/list` takes no parameters.
Its result is an array holding a [site ingress record](#site-ingress-records) for each site ingress on the host, ordered by name in ascending byte order.

`/ingresses/site/show` reports one site ingress.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |

The server MUST refuse the request with `not_found` if no site ingress has that name.
Its result is the site ingress record.

Neither method emits an event.

## Creation

`/ingresses/site/create` creates a manual site ingress with no attachments.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |
| `hostname` | string | yes | the hostname the site ingress serves |
| `description` | string or null | no | a description for operators |
| `tls_provider` | string | no, default `"acme"` | the TLS provisioning mode, `"acme"`, `"internal"` or `"none"` |

The server MUST refuse the request with `requirements_invalid` if `name` is a [reserved name](../runtime/identity.md#reserved-names), or if a site ingress of either source already has that name.
The server MUST refuse a `hostname` that breaks the rules for an [app ingress's hostname](../language/ingress.md#hostnames) with `requirements_invalid`.
The server MUST refuse any other `tls_provider` with `requirements_invalid`.

> [!NOTE]
> The `tailscale` mode is valid only on a site ingress its discovery provider created, as [TLS provisioning modes](../runtime/site-resources.md#tls-provisioning-modes) specifies, so a manual site ingress cannot take it.

The runtime MUST store the hostname in lower case, whatever case the request wrote it in.
Several site ingresses MAY share a hostname, and their attachments then contend as [ingress conflicts](../runtime/site-resources.md#ingress-conflicts) specifies.

On success the result is `{ "created": true }`.
The server MUST emit a `SiteIngressCreated` [event](events.md#site-resource-events) with `source` `"manual"`.

## Update

`/ingresses/site/update` changes the description or TLS provisioning mode of a manual site ingress.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |
| `description` | string or null | no | a string sets the description, `null` clears it, and an absent member leaves it as it is |
| `tls_provider` | string | no | the new TLS provisioning mode, `"acme"`, `"internal"` or `"none"`; an absent member leaves it as it is |

The server MUST refuse the request with `not_found` if no site ingress has that name.
The server MUST refuse the request with `requirements_invalid` if the site ingress is discovered.
The server MUST refuse any other `tls_provider` with `requirements_invalid`.

On success the result is `{ "updated": true }`.
When the update changes the description or the TLS provisioning mode, the server MUST emit a `SiteIngressUpdated` event carrying the site ingress's hostname, TLS provisioning mode and description as they stand after the update.
An update that changes neither MUST succeed, change nothing and emit no event.

A change of TLS provisioning mode takes effect on the site ingress's attachments as [TLS provisioning modes](../runtime/site-resources.md#tls-provisioning-modes) and [plaintext ingresses](../runtime/proxy.md#plaintext-ingresses) specify.

## Deletion

`/ingresses/site/delete` deletes a site ingress and its attachments.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |

The server MUST refuse the request with `not_found` if no site ingress has that name.
The server MUST refuse the request with `requirements_invalid` if the site ingress is discovered and its discovery provider still reports it.
An operator MAY delete a discovered site ingress that is stale.

On success the result is `{ "deleted": true }`.
The server MUST emit a `SiteIngressAttachmentRemoved` event for each attachment the deletion removes, and a `SiteIngressDeleted` event whose `source` is the deleted site ingress's source.

## Attachments

`/ingresses/site/attach/forward` attaches a forward target to a site ingress, and `/ingresses/site/attach/redirect` attaches a redirect target.
Both take these members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |
| `port` | integer | yes | the attachment's port, from 1 to 65535 |
| `protocol` | string | yes | the attachment's protocol, `"tcp"`, `"udp"`, `"http"` or `"http2"` |

`/ingresses/site/attach/forward` also takes these members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `target_app` | string | yes | the app to route to |
| `target_service` | string | yes | the service of that app to route to |

`/ingresses/site/attach/redirect` also takes these members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `redirect_url` | string | yes | the URL to redirect to, beginning `http://` or `https://` |
| `redirect_code` | integer | no, default 307 | the redirect's status code, one of the [route redirect codes](../language/http-service.md#redirect-codes) |
| `redirect_preserve_path` | boolean | no, default `true` | whether the redirect keeps the request's path and query |

The server MUST refuse the request with `not_found` if no site ingress has that name.
The server MUST refuse with `requirements_invalid` a `port` outside 1 to 65535, a `protocol` the table does not list, a `target_app` or `target_service` that breaks the name rules, a `redirect_url` beginning with neither `http://` nor `https://`, and a `redirect_code` that is not a route redirect code.
The server MUST refuse a redirect whose `protocol` is `"tcp"` or `"udp"` with `requirements_invalid`.

The server MUST accept a forward target whose app or service does not exist.
Such an attachment raises the [`site_ingress_target_missing`](../runtime/faults.md#ingress-and-service-faults) fault until its target exists.

Where `redirect_preserve_path` is `true`, the runtime MUST answer with a `Location` of `redirect_url` with any trailing `/` removed, followed by the request's path and query string.
Where it is `false`, the runtime MUST answer with a `Location` of `redirect_url` as given.

If the site ingress has no attachment on the `(port, protocol)`, the runtime MUST add one, and the server MUST emit a `SiteIngressAttachmentAdded` event.
If it has one, the runtime MUST replace that attachment's target in one step, and the server MUST emit a `SiteIngressAttachmentUpdated` event.
Replacing a target MUST NOT change the attachment's `created_at`, which decides ingress conflicts.
Attaching the target an attachment already has MUST succeed, change nothing and emit no event.
Either event MUST carry the attachment's target as it stands after the request.

On success the result of either method is `{ "attached": true }`.

`/ingresses/site/detach` removes an attachment.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the site ingress's name |
| `port` | integer | yes | the attachment's port |
| `protocol` | string | yes | the attachment's protocol |

The server MUST refuse the request with `not_found` if no site ingress has that name, or it has no attachment on the `(port, protocol)`.
On success the result is `{ "detached": true }`.
The server MUST emit a `SiteIngressAttachmentRemoved` event carrying the removed attachment's target.

The attachment methods apply to manual and discovered site ingresses alike.

## Discovery status

`/ingresses/site/discovery/status` takes no parameters.
Its result is `{ "providers": [ ... ] }`, with one object for each discovery provider the runtime runs.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the provider's name, `"tailscale"` for the [Tailscale discovery provider](../runtime/site-resources.md#tailscale-discovery) |
| `healthy` | boolean or null | `true` when the provider's most recent poll reached it and it reported a running identity, `false` when that poll did not, and `null` before the provider's first poll |
| `last_poll_at` | string | when the most recent poll finished, as an RFC 3339 timestamp, absent before the first poll |
| `last_error` | string | why the most recent poll was not healthy, absent where it was healthy or no poll has finished |
| `identity` | object | the identity the most recent poll reported, absent where it reported none |
| `ingresses` | array of objects | the site ingresses the provider maintains, ordered by name in ascending byte order |

A client MUST NOT report a provider whose `healthy` is `null` as unhealthy.

For the Tailscale discovery provider, `identity` has these members.

| member | type | meaning |
| --- | --- | --- |
| `hostname` | string | the host's tailnet DNS name, without a trailing `.` |
| `node_id` | string | the host's stable Tailscale node identity |
| `backend_running` | boolean | whether the host is logged in to its tailnet |

Each member of `ingresses` has these members.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the site ingress's name |
| `provider` | string | the provider's name |
| `key` | string | the provider key |
| `hostname` | string | the site ingress's hostname |
| `stale` | boolean | whether the site ingress is stale |

The method emits no event.

## Discovery refresh

`/ingresses/site/discovery/refresh` takes no parameters.
On receiving it, the runtime MUST start a poll of every discovery provider it runs, without waiting for the provider's next scheduled poll.
A poll already in progress when the request arrives MUST NOT stand in for that poll.

The server MUST answer without waiting for the poll to finish, and the result is `{ "refreshed": true }`.
A client learns the poll's outcome from [discovery status](#discovery-status).

The method emits no event of its own.
The changes a poll makes to discovered site ingresses are the discovery provider's, as [sources](../runtime/site-resources.md#sources) specifies, whatever started the poll, and their events MUST NOT carry an actor.
