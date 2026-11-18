# Terminate DTLS on a DTLS ingress

TLS termination on a `Terminate.Tls` ingress already landed on D6: the L4 route carries caddy-l4's `tls` handler ahead of `proxy`, and `layer4.handlers.tls` is a required module.
What remains is DTLS, whose datagrams reach the Service still encrypted.

## Decisions

- DTLS is terminated in the proxy, by a layer4 handler written in this repository and built into the seedling-caddy image. caddy-l4 has no DTLS module.
- The handler is built on pion/dtls, which serves DTLS 1.2 and 1.3.
- It takes its certificate from the proxy's TLS app, looked up by the ingress's hostname rather than the client's SNI, so runtime-managed certificates (through `get_certificate`) and proxy-held ones both apply with no new certificate path.
- Ports are shared by hostname where the proxy can route by it: a claim is `(port, transport, Option<hostname>)`, `None` for pass-through. Claims of one kind (TLS, HTTP/3, plaintext HTTP) for different hostnames share; anything else conflicts. DTLS is treated as carrying no SNI: a DTLS ingress claims its whole UDP port and presents its own hostname's certificate whatever the client names. SNI sharing for DTLS can be added if a use case emerges. A TLS handshake with no SNI, or naming no ingress on the port, is refused. A conflicting app ingress is left out whole, not per transport.

## Rejected

- Terminating DTLS in the daemon: proxy-held certificates would have to be read out of the proxy's storage, and the Rust DTLS libraries are thinner.
- Removing `Terminate.Dtls` from the language until a terminator exists.

- Sessions are capped by the proxy's source ports toward one Service (one address, so one port per session). That cap is shared with every L4 ingress already, and sits far above what one site's clients need, so the proxy stays single-address. A wider port range in the proxy's namespace, or more proxy source addresses, are the levers if a host ever needs more.
- Whether the Service sees the client's address is not this card's: L6 covers passing it outbound, alongside A2's inbound recovery.

- The idle period is fixed in the proxy, not declared in the BSL; a per-ingress setting can be added later if an app needs one.
- Sessions survive a config apply that leaves their ingress as it was; an upgrade may end them.
- A refused session is logged and holds a `dtls_session_refused` condition fault.
- The DTLS session rules live in PRX ("DTLS-terminating ingresses"); ING keeps the termination, version and certificate rules.
- Connection IDs are supported, with a return routability check before sending to a changed address; pion/dtls provides both.

## Build

- [ ] A Go layer4 handler module for DTLS, in this repository, built into the image by `xcaddy --with` and listed in `docker/caddy/required-modules.txt`; bump the image build serial
- [ ] Emit it on a DTLS ingress's UDP L4 route ahead of `proxy`
- [ ] Handshake gating (cookie exchange), per-session upstream flow, refusal when no flow can be opened, idle expiry, close on client close
- [ ] Connection IDs and the return routability check for both versions
- [ ] Discard non-DTLS datagrams without reply
- [ ] Sessions survive a config reload that leaves the ingress as it was: keep session state outside the handler instance (a usage pool keyed by listener) so a reprovisioned handler adopts it
- [ ] Log each refused session naming the ingress; get refusals to the runtime (proxy log or admin endpoint) and hold `dtls_session_refused` as a condition fault, cleared after five quiet minutes or when the ingress goes
- [ ] Confirm TLS ingresses accept 1.2 and 1.3 (the proxy's default) and pin it in a config test
- [ ] Image check fixture (`everything_on()`) exercises a DTLS route
- [ ] Claim model in reconcile: compute claims per `(port, transport)`, resolve conflicts into `ingress_conflict` across apps and site ingresses, replacing `conflicting_listener_ports`
- [ ] Fail evaluation when two ingresses of one app make conflicting claims
- [ ] TLS on TCP shared with HTTPS: front the HTTPS server with the `caddy.listeners.layer4` wrapper, matching `Terminate.Tls` hostnames by SNI and falling through to HTTP; key L4 servers by port and transport with one SNI-matched route per hostname
- [ ] DTLS handler presents the ingress's own certificate regardless of SNI, looked up by the ingress's hostname
- [ ] Update `docs/networking.md` and the `proxy.rs` comment that say DTLS datagrams pass through
