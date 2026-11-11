---
id: RLY
---

# Canopy relay

## Canopy Relay

<!-- from interface: Canopy Relay -->

Seedling has no Canopy identity of its own. Instead a connected client may offer to carry Seedling's Canopy requests, issuing them under its own identity and returning the responses. The relay is a generic HTTP conduit: Seedling supplies a method, path, headers, and body, and receives whatever Canopy answered.

> i[canopy.offer]
> `/canopy/offer { agent, endpoint, via? }` registers the calling connection as a Canopy provider.
> `agent` names the offering program, `endpoint` is the Canopy base URL it will reach, and `via` is an optional free-form note on how it authenticates.
> All three are recorded for operator display only; no behaviour depends on their values.
> Returns `{ offer_id }`, an opaque identifier for the registration.
> A `CanopyOffered` event is emitted.

> i[canopy.offer.lifetime]
> An offer remains registered until the connection that made it is lost, the offering client calls [canopy.withdraw](rule:i/canopy.withdraw), or Canopy access is disabled.
> A `CanopyWithdrawn` event is emitted whenever an offer ends, carrying a `reason` that distinguishes connection loss, voluntary withdrawal, and revocation by the disabled setting.

> i[canopy.offer.selection]
> Several offers may be registered at once, across the same or different connections.
> Relayed requests are sent to the most recently registered offer that is still live.
> Earlier offers remain registered and become eligible again if the newer one ends.

> i[canopy.offer.disabled]
> While Canopy access is disabled per [canopy.settings](rule:i/canopy.settings), `/canopy/offer` is refused with `canopy_disabled`.
> Disabling also revokes every live offer immediately, rather than waiting for the offering clients to disconnect, so no further request can be relayed.
> Requests already in flight are left to finish or to reach their deadline. Their side effects at Canopy have already happened — the offering client issued them before the setting changed, and a request that has been sent cannot be unsent — so cancelling them would abandon a response without preventing anything, and they are bounded by [canopy.relay.limits](rule:i/canopy.relay.limits) regardless.

> i[canopy.withdraw]
> `/canopy/withdraw { offer_id }` ends an offer registered by the calling connection.
> Returns `{}` on success, or `not_found` if the offer does not exist or belongs to another connection.

> i[canopy.relay]
> To relay a request, the server opens a bidirectional stream per [stream.canopy](rule:i/stream.canopy) on the connection holding the selected offer and writes the header:
> ```json
> { "canopy": "<offer_id>", "method": "<string>", "path": "<string>", "headers": { } }
> ```
> `path` is the request target in origin form (path and query, no scheme or authority); resolving it against a base URL is the offering client's responsibility, as is supplying authentication.
> `headers` is an object map of lower-cased header name to value. The request body, if any, follows the header line as raw bytes.
> A header that appears more than once is represented once, with its values combined in order and separated by a comma and a space, per the field-line combination rules of RFC 9110. `Set-Cookie`, which those rules exempt, is therefore not faithfully represented; the relay is not intended to carry it.
>
> The offering client replies with either a response header line
> ```json
> { "status": <int>, "headers": { } }
> ```
> followed by the raw response body, or an error frame per [canopy.relay.error](rule:i/canopy.relay.error).
>
> Any status Canopy returned is reported as a `status`, including non-2xx ones: individual endpoints give specific codes meaning, so interpreting them is the caller's business and not the relay's.

> i[canopy.relay.error]
> When the offering client obtained no response from Canopy at all, it writes
> ```json
> { "error": { "code": "<string>", "message": "<string>" } }
> ```
> and closes the stream without a body. The following codes are defined:
>
> | Code | Meaning |
> |---|---|
> | `unknown_offer` | The `offer_id` in the request header is not one the client currently holds. |
> | `unreachable` | The client could not obtain a response from Canopy (connection failure, timeout, or protocol error). |
> | `invalid_request` | The request header was malformed or described a request the client cannot construct. |

> i[canopy.relay.limits]
> The server bounds each relayed request in three ways:
>
> - It stops reading a response body once it exceeds 16 MiB, and resets the stream.
> - It resets the stream if no complete response arrives within 60 seconds.
> - It allows at most 8 relayed requests to be in flight at once; further requests wait for a slot rather than opening a stream.
>
> Relay streams are server-initiated and so do not count against [stream.concurrency-limit](rule:i/stream.concurrency-limit), which bounds client-initiated request streams.

> i[canopy.unavailable]
> A relayed request attempted while Canopy access is disabled, or while no offer is live, fails immediately with `canopy_disabled` or `canopy_unavailable` respectively.
> Requests are never queued for a provider that might appear later.

> i[canopy.settings]
> `/canopy/settings/get` returns `{ enabled, updated_at }`.
> `/canopy/settings/set { enabled }` turns Canopy access on or off. The setting is stored durably and defaults to enabled.
> Returns `{}` on success.

> i[canopy.status]
> `/canopy/status` returns the current state of Canopy access:
>
> - `enabled`: whether Canopy access is turned on.
> - `offer`: the offer that would serve the next request, or `null` when none is live. When present, an object with fields `offer_id`, `agent`, `endpoint`, `via`, and `offered_at` (RFC 3339).
> - `offers`: the number of live offers, which may exceed one.
> - `server_id`: the Canopy server identifier last resolved for this instance, or `null` if none has been.
> - `last_report`: the outcome of the most recent status report, or `null` if none has been attempted. When present, an object with fields `at` (RFC 3339), `ok` (boolean), and `error` (string, present only when `ok` is false).
> - `applications`: an array with one object per app [reported as a Canopy application](rule:r/canopy.app.key), sorted by app name, with fields `app` and `type`.
