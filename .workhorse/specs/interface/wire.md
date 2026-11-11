---
id: WIRE
---

# Streams and wire format

Applies to how requests, responses and session data are carried on the streams and datagrams of an OI [connection](transport.md#connection).

## Borrowed terms

| term | meaning |
| --- | --- |
| bidirectional stream, unidirectional stream | QUIC streams, as defined in [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) section 2 |
| client-initiated, server-initiated | which end opened a stream, as defined in RFC 9000 section 2.1 |
| finish | to end the sending part of a stream normally, with a STREAM frame carrying the FIN bit, as defined in RFC 9000 section 3.1 |
| reset | to end a stream abruptly, with RESET_STREAM or STOP_SENDING, as defined in RFC 9000 section 3 |
| datagram | a QUIC DATAGRAM frame's payload, as defined in [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221) |
| JSON object, JSON value | as defined in [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259) |

## Terminology

### Opening line

The JSON object, encoded on a single line, with which a bidirectional stream begins.

### Control request

A [method](overview.md#method) call answered with one response, after which its stream ends.

### Subscription request

A method call whose response is followed by data on a server-initiated unidirectional stream.

### Session request

A method call whose stream stays open after its response, to carry the session the call opened.

### Actor

The person or system on whose behalf a client makes a request.

## Stream dispatch

A client MUST begin every bidirectional stream it opens with an opening line.
The opening line ends at the first line feed (0x0A) or, where nothing follows the JSON object, at the end of the stream.
An opening line that further data follows MUST end with a line feed, and MUST NOT exceed 65,536 bytes including it.

If the opening line has a `forward` member, the server MUST treat the stream as a [port forward data stream](#port-forward-data).
Otherwise the server MUST treat the stream as a call of the method its `method` member names.

The server MUST begin every server-initiated bidirectional stream with an opening line too, and the member it carries names the stream's kind.
A `canopy` member names a [Canopy relay stream](#canopy-relay-streams), and no other kind is defined.
A client MUST reset a server-initiated bidirectional stream whose kind it does not recognise, so that the server can tell an unsupported kind from an empty answer.

A server-initiated unidirectional stream carries no opening line.
Its content is defined by the request that caused the server to open it.

## Control requests

A client MUST write a request as one JSON object and then finish its side of the stream.
The request MAY be followed by a line feed.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `method` | string | yes | the method to call |
| `actor` | object | no | the [actor](#actor) the request is made for |
| `params` | object | no | the method's parameters |

A method that defines parameters MUST refuse a request whose `params` is absent or does not match the method's parameter shape, with `requirements_invalid`.
A method that defines no parameters MUST ignore `params`.

The server MUST accept a request of up to 4,194,304 bytes.
The server MUST answer an unknown method with `not_found`.
The server MUST answer a request it cannot parse as a request object, including one longer than it accepts, with `requirements_invalid`.

The server MUST write its response as one JSON object and then finish the stream.
The end of the stream ends the response, and no length prefix or line feed delimits it.
A client SHOULD accept a response of up to 4,194,304 bytes.

A successful response MUST have the form `{ "result": <value> }`, where the value is the JSON value the method defines.
An error response MUST have the form `{ "error": { "code": <string>, "message": <string> } }`, where `code` is one of the [error codes](#error-codes) and `message` describes the failure for a person.

A failed request MUST leave the connection usable for further requests.

## Actor attribution

| member | type | required | meaning |
| --- | --- | --- | --- |
| `kind` | string | no | the means by which the actor was authenticated, such as `ctl`, `password`, `tailscale` or `dev` |
| `id` | string | no | a stable identifier for the actor, such as an email address or a key fingerprint |
| `display` | string | no | a label for the actor, for a person to read |
| `session` | string | no | an opaque identifier for the actor's session |

The actor is the client's assertion, and the server MUST record it as the client supplied it.
The server MUST refuse a request whose `actor` is not an object, or carries a member of another type than the table gives, with `requirements_invalid`.

If a request has no `actor` member, the server MUST attribute it to an actor whose `kind` is `ctl`, whose `id` is the [fingerprint](transport.md#fingerprint) of the client's key, and whose `display` is that key's label in the [authorised key set](transport.md#authorised-key-set), or the fingerprint where the key has no label.
That actor has no `session`.

The server MUST attach the actor of a request to every [event](events.md#event-types) the request causes, and to the [audit log](../runtime/audit-log.md#storage) entries it records for the request.

## Resource type names

Wherever a message names a [resource type](../language/overview.md#resource-type), it MUST use one of `deployment`, `job`, `service`, `ingress`, `volume`, `external_volume` and `external_service`.

## Error codes

| code | meaning |
| --- | --- |
| `not_found` | the method, or the app, resource, action, generation, shell session, port forward or other object the request names, does not exist |
| `not_installed` | the request needs the app installed, and it is not |
| `already_installed` | an install was requested for an app that is installed or uninstalling |
| `install_in_progress` | an install was requested for an app whose install is running |
| `operation_in_progress` | the request conflicts with a lifecycle operation running or queued for the app, or with a change to the app being applied; `message` names which |
| `already_queued` | a lifecycle operation is already queued for the app |
| `requirements_invalid` | the request cannot be parsed, or its actor or parameters are missing, malformed or out of range; `message` names each offending field |
| `script_error` | a definition failed to parse or evaluate; `message` gives the error |
| `validation_failed` | a [parameter validator](params.md#validation) rejected the proposed values; `message` names each rejecting parameter and its reason |
| `bundle_invalid` | a definition bundle breaks the [bundle rules](definitions.md#bundle-rules); `message` names the offending path or field |
| `unsupported_seedling` | a definition does not [support](definitions.md#seedling-version-support) the running Seedling version |
| `fetch_failed` | a definition could not be [fetched](definitions.md#fetching), or an image [pulled](images.md#pulling-an-image), from its registry; `message` gives the cause |
| `registry_not_allowed` | the registry a reference names is not on the [registry allowlist](images.md#registry-allowlist) |
| `server_busy` | the server's [stream limit](#stream-limit) was reached |
| `internal` | a failure inside the server, such as a storage error, kept the request from completing |
| `canopy_disabled` | Canopy access is turned off on the host, and the request was refused for that reason alone |

## Subscription requests

`/events/subscribe` and `/logs/stream` are subscription requests.

The server MUST answer a subscription request on its bidirectional stream exactly as it answers a control request, before it opens any unidirectional stream for it.
If the response is an error, the server MUST NOT open a unidirectional stream for the request.

A client MUST read the bidirectional stream to its end and classify the response before it waits for the unidirectional stream.
A client MUST treat a bidirectional stream that ends without a response as an error.
A client MUST report an error response's `code` and `message` to its caller.
A client MUST NOT wait without limit for the unidirectional stream after a successful response, since the server can fail to open it.

After a successful `/events/subscribe`, the server MUST open one unidirectional stream and write [events](events.md#subscription) to it as JSON objects, each followed by a line feed, for as long as the connection lasts.
After a successful `/logs/stream`, the server MUST open one unidirectional stream and write [log entries](logs.md#log-stream-request) to it as JSON objects, each followed by a line feed.
Without follow mode the server MUST finish the stream once it has written the entries the request selects.
In follow mode the server MUST keep the stream open and write new entries as they appear, until the connection ends.

## Session requests

`/shells/start`, `/volumes/shell` and `/forwards/start` are session requests.

The server MUST write the response to a session request as one JSON object followed by a line feed, and MUST keep the stream open after a successful response.
Every later frame the server writes on the stream MUST be a JSON object followed by a line feed, as the method defines it.
A session request that fails before its session opens MUST be answered with an error response, after which the server MUST finish the stream.

### Shell sessions

A shell session uses three streams:

- the client-initiated bidirectional stream the request arrived on, the session stream;
- a server-initiated unidirectional stream carrying the shell's standard output;
- a server-initiated unidirectional stream carrying the server's own diagnostics about the session, as [shell sessions](shells.md) specify.

On the session stream, the client MUST write the request's opening line and then the shell's standard input as raw bytes, for the rest of the stream's life.
The server MUST write the [handshake response](shells.md#opening-an-app-shell) on the session stream and, when the session ends, the [exit frame](shells.md#session-end), and nothing else.

Each end MUST relay standard input and output independently.
Output waiting to be delivered MUST NOT hold up standard input, nor delay the server noticing that the session has ended.

## Port forward data

Each TCP connection tunnelled through a port forward uses one client-initiated bidirectional stream.
Its opening line MUST be `{ "forward": "<forward_id>" }`, naming the forward the [`/forwards/start`](port-forwards.md#requesting-a-forward) response gave.
After the opening line, each direction carries the tunnelled connection's bytes unchanged.
Each direction of the stream MUST be finished when the matching direction of the tunnelled connection ends, and the stream MUST be reset when the tunnelled connection fails, as [TCP tunnelling](port-forwards.md#tcp-tunnelling) specifies.

Each UDP datagram tunnelled through a port forward is carried as one QUIC datagram.
A datagram MUST begin with the forward's 16-bit `forward_key`, in network byte order, immediately followed by the UDP payload.
The server MUST ignore a datagram shorter than 2 bytes, and one whose key names no forward of the connection.
A payload too large for the path's datagram limit is dropped, and reported as a [forward status](port-forwards.md#status-messages) message.

## Canopy relay streams

Each request relayed to Canopy uses one server-initiated bidirectional stream.
Each direction carries a header line, a JSON object followed by a line feed, then the body as raw bytes to the end of the stream, with no length prefix.
A header line MUST NOT exceed 65,536 bytes including its line feed.

The server MUST write the request's header line and body and then finish its side of the stream.
The client MUST write the response's header line and body and then finish its side.
The header lines' members are defined by the [Canopy relay](canopy-relay.md#relay-streams).

## Stream limit

The server MUST limit the number of client-initiated bidirectional streams it serves at once, across all connections.
The limit defaults to 64, and the daemon's `--max-streams` option sets it.

The server MUST count a stream against the limit from when the stream opens.
The server MUST stop counting a port forward data stream, a subscription request or a session request once it has dispatched the stream.
If a stream opens while the limit is reached, the server MUST answer it at once with a `server_busy` error response and finish it, whatever the stream's kind.
A client SHOULD retry a request refused with `server_busy` after a delay.

> [!NOTE]
> The limit bounds the memory held by request bodies being read.
> Long-lived streams leave the count once dispatched, so open sessions do not starve control requests.
