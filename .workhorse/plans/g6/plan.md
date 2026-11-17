# Terminate DTLS on a DTLS ingress

TLS termination on a `Terminate.Tls` ingress already landed on D6: the L4 route carries caddy-l4's `tls` handler ahead of `proxy`, and `layer4.handlers.tls` is a required module.
What remains is DTLS, whose datagrams reach the Service still encrypted.

## Decisions

- DTLS is terminated in the proxy, by a layer4 handler written in this repository and built into the seedling-caddy image. caddy-l4 has no DTLS module.
- The handler is built on pion/dtls, which serves DTLS 1.2 and 1.3.
- It takes its certificate by SNI from the proxy's TLS app, as the L4 `tls` handler does, so runtime-managed certificates (through `get_certificate`) and proxy-held ones both apply with no new certificate path.

## Rejected

- Terminating DTLS in the daemon: proxy-held certificates would have to be read out of the proxy's storage, and the Rust DTLS libraries are thinner.
- Removing `Terminate.Dtls` from the language until a terminator exists.

- Sessions are capped by the proxy's source ports toward one Service (one address, so one port per session). That cap is shared with every L4 ingress already, and sits far above what one site's clients need, so the proxy stays single-address. A wider port range in the proxy's namespace, or more proxy source addresses, are the levers if a host ever needs more.
- Whether the Service sees the client's address is not this card's: it is left to PROXY protocol work.

## Open

- How long an idle DTLS session is kept.
- Whether a session survives a change of client address through connection IDs.
