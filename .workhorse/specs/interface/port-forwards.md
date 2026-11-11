---
id: FWD
---

# Port forwards

Applies to port forwards, through which an operator's client reaches a port of an [app](../overview.md#app)'s service over the [Operator Interface](../overview.md#operator-interface) connection.

## Borrowed terms

| term | meaning |
| --- | --- |
| bidirectional stream, finish, reset, stop sending, application error code | as defined by QUIC, [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) sections 2, 3 and 19 |
| QUIC datagram | an unreliable DATAGRAM frame payload, as defined by [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221) |
| maximum datagram size | the largest QUIC datagram payload an endpoint can currently send on a path, as [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221) section 5 describes |
| EDNS(0) payload size | the UDP payload size a DNS requester advertises, as defined by [RFC 6891](https://www.rfc-editor.org/rfc/rfc6891) section 6.2.3 |

## Terminology

### Port forward

A relay the server runs between a client and one port of an app's service, for as long as the request that created it stays open.

### Control stream

The bidirectional stream that carries a port forward's `/forwards/start` request, its response and its status messages, and stays open for the forward's lifetime.

### Forward key

A 16-bit number the server allocates to a port forward, identifying the forward among the forwards of one connection.

## Requesting a forward

A client requests a [port forward](#port-forward) by sending `/forwards/start` on a new [control stream](#control-stream).

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | yes | the app's name |
| `service` | string | yes | the name of a service the app's definition declares |
| `port` | integer, 0 to 65535 | yes | a port of that service, as `service.port()` names it |
| `proto` | string | yes | `"tcp"` or `"udp"` |

A service port is declared for `tcp` when a pod in the app attaches to it with [`pod.tcp()`](../language/pod.md#port-bindings) or [`pod.http()`](../language/pod.md#port-bindings), and for `udp` when a pod attaches to it with [`pod.udp()`](../language/pod.md#port-bindings).

The server MUST reject a `proto` other than `"tcp"` or `"udp"` with `requirements_invalid`.
The server MUST reject a request naming an app that is not registered with `not_found`.
The server MUST reject the request with `not_installed` unless the app's [install phase](../overview.md#install-phase) is `Installed`.
The server MUST reject a request naming a service the app does not declare, or a port the service does not declare for `proto`, with `not_found`.
For a UDP forward, the server MUST open its relay toward the service before it responds, and MUST reject the request with `internal` if it cannot.
A rejected request MUST create no forward.

On success the server MUST write the response, followed by a line feed, to the control stream.
The result is:

| member | type | meaning |
| --- | --- | --- |
| `forward_id` | string | the forward's identifier, a UUID in its hyphenated text form |
| `forward_key` | integer, 0 to 65535 | the forward's [forward key](#forward-key) |
| `max_udp_payload` | integer or null | for a UDP forward, the [largest payload](#udp-tunnelling) the server can relay toward the client; `null` for a TCP forward |

After an error response the server MUST finish its sending side of the control stream.

## Forward keys

The server MUST allocate a forward key to every forward, TCP and UDP alike, and the forward MUST hold it for its lifetime.
The server MUST NOT allocate one key to two live forwards on one connection, so a connection can hold at most 65536 forwards at once.
The server MUST reuse the keys of forwards that have ended.
If every key is held, the server MUST reject the request with `server_busy`.

## Status messages

After its response, the server MAY write status messages to the control stream, each a JSON object followed by a line feed.

```json
{ "status": { "level": "<level>", "message": "<text>" } }
```

`level` is `"warn"` or `"error"`, and `message` is text for the operator.
A client SHOULD show status messages to its user.
A client MUST NOT treat a status message as ending the forward.

The server MUST report each of the following by a status message:

- a datagram from the service dropped because it exceeds the maximum datagram size;
- a failure receiving from the service on a UDP forward's relay;
- a failure sending to the service on a UDP forward's relay, once when failures begin and again, with the number of failed sends, when a send next succeeds;
- datagrams toward the service dropped because the forward's relay is not keeping up with them, once when dropping begins and again, with the number dropped, when a datagram next gets through;
- a tunnelled TCP connection the server could not open to the service.

## TCP tunnelling

A client carries each TCP connection through a TCP forward on its own [forward data stream](wire.md#port-forward-data).
When the server accepts a forward data stream, it MUST open a TCP connection to the service's address at the forward's port and relay bytes between the two.
The server MUST deliver to the service every byte that follows the stream's [opening line](wire.md#opening-line).
The client MUST relay each local connection it accepts to its own forward data stream in the same way, so that both ends of the tunnel behave alike.

Each end MUST relay each direction independently, so that data waiting to be delivered in one direction never holds up the other.
When one side finishes sending, each end MUST finish that direction toward the other side and keep relaying the opposite direction.
The tunnel ends once both directions have finished.

If either side fails, including the server failing to connect to the service or the stream naming no live TCP forward, each end MUST tear down both directions, and MUST reset the stream and stop sending on it with application error code 0 rather than finish it.

> [!NOTE]
> A reset tells the other end that the transfer was cut short; a finished stream would let it take a truncated transfer for a complete one.
> A connection that half-closes after sending its request still receives the whole response.

If a forward data stream opens while the server's [stream limit](wire.md#stream-limit) is reached, the server MUST reset the stream and stop sending on it with application error code 0.

## UDP tunnelling

A client carries each UDP datagram through a UDP forward as a [forward datagram](wire.md#port-forward-data), prefixed with the forward's key.
The server MUST forward each datagram's payload to the service's address at the forward's port, and MUST send each datagram the service returns to the client with the same key prefix.
The server MUST discard a datagram shorter than two bytes, and a datagram whose key no UDP forward on the connection holds.

`max_udp_payload` MUST be the server's maximum datagram size toward the client, less the two bytes of the key, at the time of the request.
The value can change as the path's maximum datagram size estimate is refined.
A client SHOULD size application-level buffers, such as an EDNS(0) payload size, to the smaller of `max_udp_payload` and its own maximum datagram size less two.

> [!NOTE]
> A path's maximum datagram size can differ in each direction, and each end knows only its own sending limit.
> A client presenting forwards through a TUN interface can set the interface's MTU to the smaller of the two limits, so that the operating system enforces it before packets reach the forward.

## Relay resilience

A relay MUST distinguish a condition affecting one datagram from a condition affecting the forward.
A relay MUST drop and report a datagram that exceeds the maximum datagram size, and continue.
A relay MUST relay a zero-length datagram.
A relay MUST report a transient socket error, including an ICMP port-unreachable surfacing on a connected socket, and continue.
A relay MUST NOT end except when its forward ends or the connection is lost.
A failure receiving from or sending to the service MUST NOT end the relay or the forward.
The client MUST classify these conditions as the server does.

## Forward lifetime

A port forward remains active until one of the following occurs:

- the client finishes or resets its sending side of the control stream;
- an operator stops the forward with [`/forwards/stop`](#stopping-a-forward);
- the connection is lost;
- the app leaves `Installed`;
- a new [resource graph](../overview.md#resource-graph) takes effect for the app that no longer declares the forward's service, or its port for the forward's `proto`.

When an app leaves `Installed`, the server MUST end every forward of that app.
When a new resource graph takes effect for an app, the server MUST end every forward of that app whose service or port the graph no longer declares, and MUST leave the app's other forwards active.
A forward MUST NOT outlive a resource graph change that removes its port, including a change that takes effect while the forward is being established, in which case the request MUST be rejected with `not_found`.

When a forward ends, the server MUST close all of its tunnelled connections, finish its sending side of the control stream, and stop relaying its datagrams.

## Forward events

When the server has written a forward's response, it MUST emit a `ForwardStarted` [event](events.md#session-events).
When a forward ends for any reason, the server MUST emit a `ForwardStopped` event.

## Listing forwards

`/forwards/list` lists the active port forwards.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `app` | string | no | restricts the list to that app's forwards |

The result is `{ "forwards": [ ... ] }`, an array of forward records.
Without `app` the server MUST list every active forward on the host.

| member | type | meaning |
| --- | --- | --- |
| `forward_id` | string | the forward's identifier |
| `app` | string | the app the forward reaches |
| `service` | string | the service the forward reaches |
| `port` | integer | the service port the forward reaches |
| `proto` | string | `"tcp"` or `"udp"` |
| `opened_at` | string | when the forward was established, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |
| `actor` | object | the [actor](wire.md#actor-attribution) the opening request is attributed to |

## Stopping a forward

`/forwards/stop` ends an active port forward, sent on its own control request stream.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `forward_id` | string | yes | the forward's identifier |

On success the result is `{}`.
The server MUST let any operator stop any forward, whichever connection opened it.
The server MUST reject a `forward_id` that is not a UUID with `requirements_invalid`.
The server MUST reject a `forward_id` that names no active forward with `not_found`.
A stopped forward MUST end as [forward lifetime](#forward-lifetime) specifies.

## Concurrency

The server MUST allow several port forwards to be active at once, including several to the same service port.
