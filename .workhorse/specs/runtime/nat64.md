---
id: NAT64
---

# NAT64

## NAT64

<!-- from runtime: Infrastructure / NAT64 -->

> r[infra.nat64.mode]
> The runtime must accept a NAT64 mode setting via a command-line argument. The accepted
> values are:
>
> - `auto` (default) — the runtime probes for existing NAT64 infrastructure on startup and
>   enables its own NAT64 only if none is detected.
> - `enabled` — the runtime always provides NAT64.
> - `disabled` — the runtime never provides NAT64.

> r[infra.nat64.detection]
> In `auto` mode, the runtime must detect existing NAT64 infrastructure by resolving the
> well-known name `ipv4only.arpa` (RFC 7050) for AAAA records. If the response contains a
> synthesised AAAA record (i.e. an address outside the `ipv4only.arpa` canonical addresses
> `192.0.0.170` and `192.0.0.171`), the network already provides NAT64 and DNS64; the
> runtime must not activate its own. Otherwise, the runtime must activate NAT64. Detection
> is performed once at startup.

> r[infra.nat64.translator]
> When NAT64 is active, the runtime must configure a stateful NAT64 translator using the
> well-known prefix `64:ff9b::/96`. The translator must be operational before any workload
> containers are started.

> r[infra.nat64.translator.lifecycle]
> The runtime must ensure the NAT64 translator is configured whenever workloads can run
> and removed when they cannot. Specifically: the translator must be set up at daemon
> startup (when NAT64 is required) and on every transition from no-workloads-registered
> back to workloads-registered. It must be torn down on the transition to
> no-workloads-registered. The translator must be operational before any workload
> container is started. The runtime must not remove the translator on daemon shutdown:
> workload containers continue running across daemon restarts and depend on NAT64 to
> reach the IPv4 internet, so the kernel translator state must survive the daemon
> process. If the translator cannot be initialised and NAT64 is required (`enabled` mode
> or `auto` mode with no external NAT64 detected), the runtime must report an error and
> file a fault.
>
> A teardown that cannot establish whether the translator is installed has failed; it must
> not be taken for the translator being absent. The runtime must not report the translator
> removed while it remains installed, and must attempt the teardown again while no
> workloads are registered.

> r[infra.nat64.forwarding]
> When NAT64 is active, the runtime must ensure that IPv6 and IPv4 forwarding are enabled on
> the host, and that a route for `64:ff9b::/96` exists so that container traffic destined
> for the NAT64 prefix reaches the translator.

> r[infra.nat64.dns64]
> When NAT64 is active, the [resolver configuration](rule:r/infra.resolver.config) must enable
> DNS64 so that lookups for IPv4-only names return synthesised AAAA records under
> `64:ff9b::/96`. When NAT64 is not active, DNS64 synthesis must be disabled in the
> resolver configuration.

> r[infra.nat64.ipv6-egress]
> At startup, the runtime must determine whether the host has working IPv6 egress to the
> internet. The determination must be based on observable host network state (such as the
> presence of a default IPv6 route and at least one globally-scoped IPv6 source address on a
> non-loopback interface) and must not depend on DNS resolution or outbound internet traffic.
> The outcome is recorded once at startup and does not change for the lifetime of the
> process.

> r[infra.nat64.dns64.force-translation]
> When the runtime is providing its own NAT64 translator (i.e. is not relying on external
> NAT64+DNS64 infrastructure) and the host lacks working IPv6 egress (see
> [infra.nat64.ipv6-egress](rule:r/infra.nat64.ipv6-egress)), the resolver configuration must
> synthesise AAAA records under `64:ff9b::/96` for all IPv4-resolvable names, including
> names that already have a real AAAA record. This ensures workload containers reach
> dual-stack destinations via the NAT64 translator rather than attempting native IPv6 that
> the host cannot route. In all other cases, real AAAA records must be preserved so that
> native IPv6 remains the preferred path.
