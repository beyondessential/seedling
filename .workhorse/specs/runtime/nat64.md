---
id: NAT64
---

# NAT64

Applies to the NAT64 translation and DNS64 synthesis the [runtime](../overview.md#runtime) provides so that workloads on IPv6-only [pod networks](infrastructure.md#pod-network) reach IPv4 destinations.

## Borrowed terms

| term | meaning |
| --- | --- |
| NAT64 | Stateful translation of IPv6 packets to IPv4 and back, [RFC 6146](https://www.rfc-editor.org/rfc/rfc6146). |
| DNS64 | Synthesis of AAAA records for names that have only A records, [RFC 6147](https://www.rfc-editor.org/rfc/rfc6147). |
| well-known prefix | The prefix `64:ff9b::/96` reserved for algorithmic IPv4-embedded IPv6 addresses, [RFC 6052 §2.1](https://www.rfc-editor.org/rfc/rfc6052#section-2.1). |
| `ipv4only.arpa` | The name whose A records are `192.0.0.170` and `192.0.0.171`, queried for AAAA records to discover DNS64, [RFC 7050](https://www.rfc-editor.org/rfc/rfc7050). |
| IPv4-mapped address | An IPv6 address of the form `::ffff:a.b.c.d` representing an IPv4 address, [RFC 4291 §2.5.5.2](https://www.rfc-editor.org/rfc/rfc4291#section-2.5.5.2). |

## Terminology

### Active NAT64

The condition in which the runtime provides its own NAT64 translator for its host's workloads.

## Mode

The runtime MUST accept a `--nat64` option taking one of the values below:

| value | NAT64 is active |
| --- | --- |
| `auto` | when [detection](#detection) finds no existing NAT64 |
| `enabled` | always |
| `disabled` | never |

The option MUST default to `auto`.
The runtime MUST decide whether NAT64 is active once, when it starts.

## Detection

In `auto` mode, the runtime MUST detect existing NAT64 by resolving `ipv4only.arpa` through the host's resolver when it starts.
If the response contains an IPv6 address other than the IPv4-mapped form of `192.0.0.170` or `192.0.0.171`, the network provides NAT64 and DNS64, and NAT64 MUST NOT be active.
Otherwise NAT64 MUST be active.

## Translator

While NAT64 is active, the runtime MUST run a stateful NAT64 translator using the well-known prefix.
Traffic from workload containers to an address within the well-known prefix MUST reach the translator.
The runtime MUST ensure that IPv6 and IPv4 forwarding are enabled on the host.

## Translator lifecycle

While NAT64 is active, the translator MUST be operational before any workload container starts.
The runtime MUST set the translator up when it starts, and again when an app is installed after a period with none installed.
The runtime SHOULD tear the translator down when no app is installed.

The runtime MUST NOT tear the translator down when it shuts down.

> [!NOTE]
> Workload containers keep running across a restart of the runtime, and depend on the translator to reach IPv4 destinations.

If the translator cannot be set up while NAT64 is active, the runtime MUST report an error and file a system [fault](../overview.md#fault) of kind `nat64_setup_failed`.
It MUST NOT start workload containers until set-up succeeds, and MUST retry set-up on each later reconciliation iteration.
The runtime MUST clear that fault when set-up succeeds.

A teardown that cannot establish whether the translator is installed has failed.
The runtime MUST NOT report the translator removed while it remains installed.
After a failed teardown, the runtime MUST attempt the teardown again on each iteration while no app is installed.

## IPv6 egress

When it starts, the runtime MUST determine whether the host has IPv6 egress to the internet.
The host has IPv6 egress when its main routing table holds a default IPv6 unicast route (`::/0`) and a non-loopback interface carries an address within `2000::/3`.
The determination MUST rest only on the host's routing and address state, and MUST NOT depend on DNS resolution or on sending traffic.
If that state cannot be read, the host MUST be taken to have no IPv6 egress.

The runtime MUST record the outcome once, and MUST NOT change it while it runs.

## DNS64

While NAT64 is active, the [resolver](resolver.md) MUST synthesise AAAA records under the well-known prefix for names that have only A records.
While NAT64 is not active, the resolver MUST NOT synthesise AAAA records.

While NAT64 is active and the host has no [IPv6 egress](#ipv6-egress), the resolver MUST synthesise AAAA records under the well-known prefix for every name that has A records, including names that also have AAAA records of their own.
In every other case, the resolver MUST return a name's own AAAA records unchanged.

> [!NOTE]
> Without forced synthesis, a workload would attempt native IPv6 to a dual-stack destination that the host cannot route.
