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
- Whether the Service sees the client's address is not this card's: L6 covers passing it outbound, alongside A2's inbound recovery.

- The idle period is fixed in the proxy, not declared in the BSL; a per-ingress setting can be added later if an app needs one.
- Connection IDs are supported, with a return routability check before sending to a changed address; pion/dtls provides both.

## Build

- [ ] A Go layer4 handler module for DTLS, in this repository, built into the image by `xcaddy --with` and listed in `docker/caddy/required-modules.txt`; bump the image build serial
- [ ] Emit it on a DTLS ingress's UDP L4 route ahead of `proxy`, taking the certificate by SNI from the TLS app
- [ ] Handshake gating (cookie exchange), per-session upstream flow, refusal when no flow can be opened, idle expiry, close on client close
- [ ] Connection IDs and the return routability check for both versions
- [ ] Image check fixture (`everything_on()`) exercises a DTLS route
- [ ] Update `docs/networking.md` and the `proxy.rs` comment that say DTLS datagrams pass through
