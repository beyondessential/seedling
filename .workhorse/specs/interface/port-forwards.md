---
id: FWD
---

# Port forwards

## Port Forwards

<!-- from interface: Port Forwards -->

> i[forward.request]
> `/forwards/start { app, service, port, proto }` requests a port forward to the named service at the given service-side port number.
> `service` is the name of a Service defined in the app's BSL script.
> `port` is a port number on that Service as defined by `service.port()`.
> `proto` is either `"tcp"` or `"udp"`.
> The port is declared for `proto` when a pod in the app attaches to that Service Port: with [`pod.tcp()`](rule:l/pod.tcp) or [`pod.http()`](rule:l/pod.http) for TCP, with [`pod.udp()`](rule:l/pod.udp) for UDP.
> A request for a port the Service does not declare for `proto` fails with `not_found`, and no forward is created.
> Returns `{ "forward_id": "<string>", "forward_key": <u16>, "max_udp_payload": <uint> | null }` on success.
> `forward_id` is used for control operations such as `/forwards/stop`.
> `forward_key` is the compact 2-byte identifier used in QUIC datagram headers for UDP forwards (see [datagram.forward](rule:i/datagram.forward)); it is not used for TCP forwards.
> `max_udp_payload` is the maximum UDP payload the server can forward from the service back to the client, as defined in [forward.mtu](rule:i/forward.mtu); it is `null` for TCP forwards.
> For a UDP forward the server opens its relay toward the service before replying; if it cannot, the request fails with `internal` and no forward is created.
> The control stream that carried the request is kept open for the lifetime of the forward; closing it tears down the forward.

> i[forward.relay.resilience]
> A forward's relay must distinguish a condition affecting one datagram from one affecting the forward.
> A datagram that cannot be carried — because it exceeds the path MTU — is dropped and reported; a zero-length datagram is legal and is relayed; a transient socket error, including an ICMP port-unreachable surfacing on a connected socket, is reported and relaying continues.
> Only the loss of the underlying connection ends the relay.
> Both ends of a forward apply this classification identically.
> A relay that has ended must not continue to be reported as active: its termination is surfaced to the party that opened the forward.

> i[forward.mtu]
> For UDP forwards, `max_udp_payload` is the server's own `max_datagram_size() - 2` — the maximum payload the server can send toward the client (i.e. service responses).
> The client already has its own send limit for the client-to-server direction available locally from its QUIC connection.
> Path MTU may be asymmetric; for bidirectional protocols (e.g. DNS), clients should use the minimum of their local send limit and `max_udp_payload` when configuring application-level buffer sizes such as EDNS0.
> For a TUN-based client, setting the TUN interface MTU to that minimum causes the operating system to enforce the limit before packets reach the forwarding layer.
> This value reflects the path MTU estimate at the time of the request and may change as the estimate is refined.

> i[forward.tunnel.tcp]
> Each individual TCP connection forwarded through a TCP port forward uses a dedicated bidi stream as defined in [stream.forward](rule:i/stream.forward).
> The server accepts the stream, opens a TCP connection to the target service address and port, and relays bytes between the two.
> The client relays each local connection it accepts to its own stream in the same way, so both ends of the tunnel behave alike.
> Each direction is relayed independently: data waiting to be delivered in one direction never holds up the other.
> When one side finishes sending, the relay finishes that direction toward the other side and keeps relaying the opposite direction, so a connection that half-closes after sending its request still receives the whole response.
> The tunnel ends once both directions have finished.
> If either side fails, including the server failing to connect to the target, both directions are torn down and the stream is reset rather than finished, so the other end cannot mistake a truncated transfer for a complete one.

> i[forward.tunnel.udp]
> Each UDP datagram forwarded through a UDP port forward is carried as a QUIC datagram as defined in [datagram.forward](rule:i/datagram.forward).
> The server extracts the `forward_key`, looks up the target service address and port, and forwards the payload as a UDP datagram.
> Responses from the service are sent back as QUIC datagrams with the same `forward_key` prefix.

> i[forward.key-exhaustion]
> `forward_key` is a 16-bit value allocated to every forward, TCP and UDP alike, and held for the forward's lifetime.
> No two live forwards on one connection hold the same key, so each connection can have at most 65536 concurrent forwards.
> When allocating a new key, the server scans for an unused slot and reuses keys freed by closed forwards.
> If all 65536 keys are in use the request fails with an `"internal"` error.

> i[forward.status]
> After the initial response, the server may send additional newline-terminated JSON objects on the control stream's send half to report runtime conditions.
> Each status message has the shape `{ "status": { "level": "<level>", "message": "<text>" } }` where `level` is `"warn"` or `"error"`.
> Clients should display these messages but must not treat them as fatal; the forward remains active unless the control stream closes.
> Conditions reported include:
>
> - a UDP datagram dropped because it exceeds the path MTU;
> - a socket error on a UDP forward's relay toward the service;
> - a tunneled TCP connection the server could not open to the target;
> - datagrams toward the service dropped because the forward's relay is not keeping up with them, reported once when dropping starts and again, with the number dropped, when a datagram next gets through.

> i[forward.lifetime]
> A port forward remains active until any of the following occur:
>
> - The client closes the control stream.
> - The client sends `/forwards/stop { forward_id }` on a new control stream.
> - The connection is lost.
> - The forwarded service or port is no longer present in the app's AppDef after a script update takes effect (see [forward.script-update](rule:i/forward.script-update)).

> i[forward.script-update]
> When a new AppDef takes effect for an app (either immediately on `/apps/update` or at the next evaluation boundary if an operation was in progress), the server must check all active forwards for that app.
> Any forward whose target service name, or port for the forward's `proto` as [forward.request](rule:i/forward.request) defines it, is no longer declared in the new AppDef must be torn down: all tunneled connections are closed, the control stream is closed, and a `ForwardStopped` event is emitted.
> Forwards whose target service and port still exist in the new AppDef are unaffected.

> i[forward.start]
> When a forward is established, a `ForwardStarted` event is emitted on the event feed.
> When a forward ends for any reason, a `ForwardStopped` event is emitted on the event feed.

> i[forward.record]
> A forward record contains the following fields: `forward_id`, `app`, `service`, `port`, `proto`, and `opened_at` (RFC 3339).

> i[forward.list]
> `/forwards/list { app? }` returns an array of forward records for all currently active port forwards.
> If `app` is provided, only forwards for that app are returned; otherwise all active forwards across all apps are returned.

> i[forward.stop]
> `/forwards/stop { forward_id }` explicitly tears down an active port forward, closing all of its tunneled connections.
> Any operator may stop any forward regardless of which connection opened it.
> Returns `{}` on success, or `not_found` if the forward does not exist.

> i[forward.concurrent]
> Multiple port forwards may be active concurrently, including to the same service.
