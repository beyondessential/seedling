---
id: RLY
---

# Canopy relay

Applies to the [Operator Interface](../overview.md#operator-interface) methods and streams by which a connected client carries the [runtime](../overview.md#runtime)'s requests to Canopy.

## Borrowed terms

| term | meaning |
| --- | --- |
| origin-form | the request-target form of a path and optional query, [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112) §3.2.1 |
| field line combination | joining the values of a repeated header field, [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) §5.3 |
| bidirectional stream | a QUIC stream carrying data in both directions, [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) §2.1 |

## Terminology

### Canopy

The external fleet service that a host reports its status to and obtains certificates from.

### Offer

A registration by which a client undertakes to carry the runtime's Canopy requests over its Operator Interface connection, issuing them under the client's own Canopy identity.

### Offering client

The client that holds an [offer](#offer).

### Live offer

An offer that has been registered and has not ended.

## Offers

`/canopy/offer` registers an offer on the connection the request arrives on.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `agent` | string | yes | the offering program's name and version |
| `endpoint` | string | yes | the Canopy base URL the offering client reaches |
| `via` | string | no | how the offering client authenticates to Canopy |

The runtime MUST record `agent`, `endpoint` and `via` for display to operators, and MUST NOT vary its behaviour on their values.
The result is `{ offer_id }`, an opaque string identifying the offer.
On success, the runtime MUST emit a `CanopyOffered` [event](events.md#session-events).

An offer belongs to the connection that made it.
The runtime MUST accept `/canopy/offer` and `/canopy/withdraw` only on a connection over which it can open relay streams.

## Offer lifetime

An offer MUST end when the connection that made it is lost, when the offering client withdraws it, or when Canopy access is [disabled](#disabled-access).
When an offer ends, the runtime MUST emit a `CanopyWithdrawn` event, whose `reason` gives the cause.

| `reason` | cause |
| --- | --- |
| `disconnected` | the connection that made the offer was lost |
| `requested` | the offering client withdrew the offer |
| `disabled` | Canopy access was disabled |

A `CanopyWithdrawn` event with reason `disconnected` MUST NOT carry an actor.

`/canopy/withdraw` takes `offer_id` and ends that offer.
The result is `{}`.
If the offer does not exist, or was made on another connection, the runtime MUST refuse the request with `not_found`.

## Offer selection

Clients MAY hold several live offers at once, on one connection or several.
The runtime MUST send each relayed request to the most recently registered live offer.
When that offer ends, the next most recent live offer MUST serve the following requests.
The runtime MUST select the offer once the request holds one of the [relay slots](#relay-limits), not before it waits for one.

## Disabled access

While Canopy access is disabled, the runtime MUST refuse `/canopy/offer` with `canopy_disabled`.
Disabling Canopy access MUST end every live offer at once, without waiting for the offering clients to disconnect.
The runtime MUST NOT relay a request after disabling.
Relayed requests already in flight MUST be left to finish or reach their [deadline](#relay-limits).

> [!NOTE]
> An in-flight request has already reached Canopy through the offering client, so cancelling it would abandon its response without undoing its effect.

## Relay streams

To relay a request, the runtime opens a bidirectional stream, as [Canopy relay streams](wire.md#canopy-relay-streams) specifies, on the connection holding the selected offer.
It writes a header line, then the request body as raw bytes, then finishes its side of the stream.

| member | type | meaning |
| --- | --- | --- |
| `canopy` | string | the `offer_id` of the selected offer |
| `method` | string | the HTTP method |
| `path` | string | the request target in origin-form |
| `headers` | object | the request's header fields |

The offering client MUST resolve `path` against its Canopy base URL, and MUST supply the authentication Canopy requires.

In `headers`, and in the response header below, each member's name is a header field name in lower case and its value is the field's value.
A field that appears more than once MUST be represented once, with its values joined in order by a comma and a space, as field line combination specifies.
The runtime MUST fail a request carrying a header value that is not visible ASCII text, without relaying it.

> [!NOTE]
> `Set-Cookie` cannot be combined this way, so the relay does not carry it faithfully.

The offering client answers with a response header line followed by the response body as raw bytes, or with an [error](#relay-errors) line and no body, and then finishes the stream.

| member | type | meaning |
| --- | --- | --- |
| `status` | integer | the HTTP status code Canopy returned |
| `headers` | object | the response's header fields |

The offering client MUST report every status Canopy returned in a response header line, including a status outside the 2xx range.
The runtime MUST leave the interpretation of a status to the caller of the relayed request.

A header line, including its terminating newline, MUST NOT exceed 65,536 bytes.

## Relay errors

When the offering client obtained no response from Canopy, it MUST answer with `{ "error": { "code": <string>, "message": <string> } }` and no body.

| `code` | meaning |
| --- | --- |
| `unknown_offer` | `canopy` names no offer the client holds |
| `unreachable` | the client could not obtain a response from Canopy, through connection failure, timeout or protocol error |
| `invalid_request` | the header line is malformed, or describes a request the client cannot construct |

An offering client MUST answer `unknown_offer` to a `canopy` value it cannot parse.

## Relay limits

A relayed body, in either direction, MUST NOT exceed 16 MiB.
The runtime MUST stop reading a response body that exceeds 16 MiB, reset the stream, and fail the request.

The runtime MUST reset a relay stream, and fail the request, if no complete response arrives within 60 seconds of the request being sent.
An offering client SHOULD time out its own request to Canopy sooner, so that it can answer `unreachable`.

The runtime MUST have at most 8 relayed requests in flight at once.
A further request MUST wait for one of them to finish before its stream is opened.
Relay streams MUST NOT count against the [stream concurrency limit](wire.md#stream-limit).

## Unavailability

A Canopy request the runtime makes while Canopy access is disabled, or while no offer is live, MUST fail at once.
The runtime MUST NOT hold such a request for an offer that might appear later.

## Settings

`/canopy/settings/get` takes no parameters.
Its result is `{ enabled, updated_at }`.
`enabled` is the stored [Canopy access setting](../runtime/canopy-reporting.md#canopy-access).
`updated_at` is when the setting was last written, as an integer count of seconds since 1970-01-01T00:00:00Z, or `0` if it never has been.

`/canopy/settings/set` takes `enabled`, a boolean, and turns Canopy access on or off.
The result is `{}`.

## Status

`/canopy/status` takes no parameters and returns the state of Canopy access.

| member | type | meaning |
| --- | --- | --- |
| `enabled` | boolean | whether Canopy access is enabled |
| `offer` | object or null | the offer that would serve the next request, `null` when no offer is live |
| `offers` | integer | the number of live offers |
| `server_id` | string or null | the Canopy server identifier last resolved for the host, `null` if none has been |
| `last_report` | object or null | the outcome of the latest status report, `null` if none has been attempted |
| `applications` | array | one object per app [reported as a Canopy application](../runtime/canopy-reporting.md#applications), ordered by app name |

`offer` carries these members.

| member | type | meaning |
| --- | --- | --- |
| `offer_id` | string | the offer's id |
| `agent` | string | as given in the offer |
| `endpoint` | string | as given in the offer |
| `via` | string or null | as given in the offer |
| `offered_at` | string | when the offer was registered, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |

`last_report` carries `at`, when the report was attempted, as an RFC 3339 timestamp, and `ok`, a boolean saying whether it succeeded.
When `ok` is `false`, it MUST also carry `error`, a string describing the failure.

An `applications` entry carries `app`, the app's name, and `type`, its Canopy application type.
