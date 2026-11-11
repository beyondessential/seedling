---
id: DNS
---

# Resolver

Applies to the DNS resolver the [runtime](../overview.md#runtime) runs for the workload containers on a [host](../overview.md#host).

## Borrowed terms

| term | meaning |
| --- | --- |
| TTL | The time a resource record may be cached, [RFC 1035 §3.2.1](https://www.rfc-editor.org/rfc/rfc1035#section-3.2.1). |

## Resolver addresses

The resolver MUST be reachable at the addresses below, derived from the [host prefix](infrastructure.md#host-prefix):

| address | value |
| --- | --- |
| resolver network | `<host prefix>:fd00::/64` and `10.89.254.0/24` |
| resolver address | `<host prefix>:fd00::53` |
| host side of the resolver network | `<host prefix>:fd00::1` and `10.89.254.1` |

The resolver MUST listen at the resolver address.
The resolver address MUST NOT change while the host prefix does not.

> [!NOTE]
> The last group of the resolver address is hexadecimal `53`, matching the DNS port, so that operators can write firewall and monitoring rules against a predictable address.
> The resolver network carries IPv4 so that the resolver can reach IPv4 upstream servers.

## Resolver lifecycle

The runtime MUST run the resolver as an infrastructure container, providing DNS forwarding and caching to every workload container.
The runtime MUST have the resolver running and healthy before it starts any workload container.

While any app is installed, the runtime MUST check the resolver's health on each [reconciliation](reconciliation.md#reconciliation-loop) iteration.
It MUST restart a resolver that is not running.
It SHOULD restart a running resolver only after its health check has failed on three consecutive iterations.
The runtime MUST stop the resolver when no app is installed or installing.

On an iteration that fails to bring the resolver up, or does not finish doing so, the runtime MUST file a system [fault](../overview.md#fault) of kind `resolver_failed` naming the failure.
The runtime MUST clear that fault on an iteration that finds the resolver running.

## Upstreams

The runtime MUST accept a `--dns-upstreams` option naming the upstream DNS servers, as a comma-separated list of IP addresses with ports (`192.0.2.1:53`, `[2001:db8::1]:53`).
When the option is given, the resolver MUST forward to those servers.

When the option is not given, the resolver MUST forward to the host's system resolver at `127.0.0.54:53`.
This MUST work although that address is on the host's loopback interface, out of the resolver's reach.
The runtime MUST relay the resolver's queries to it through the host side of the resolver network, at port 53.

> [!NOTE]
> Forwarding to the host's resolver lets containers inherit the host's split DNS, per-link resolvers and search domains.
> `127.0.0.54` is the system resolver's stub address that accepts queries relayed from a source outside the loopback range.

## Resolver configuration

The resolver MUST forward every query to its upstreams.
The resolver SHOULD cache responses.
It MUST NOT serve a cached response beyond its TTL, and SHOULD NOT serve one for more than 30 seconds.

The resolver MUST synthesise AAAA records as [DNS64](nat64.md#dns64) specifies.

## Resolver replacement

The resolver MUST serve the configuration the runtime currently generates, from the resolver image the runtime runs.
The runtime MUST record on each resolver instance the configuration and address it was started with.

If the running resolver's image differs from the image the runtime runs, or its recorded configuration or address differs from the current one or is absent, the runtime MUST replace it.
The replacement MUST listen at the resolver address.
Because one instance can hold the resolver address at a time, the runtime MUST stop the running resolver before starting the replacement.

The runtime MUST persist which resolver instance is active, recording the replacement as active before starting it.
A replacement interrupted at any point MUST be completed on a later iteration without operator action.

> [!NOTE]
> Resolution is unavailable to workloads between the running resolver stopping and the replacement starting.

## Container DNS

The runtime MUST configure every workload container with the resolver address as its only DNS server, so that the container's `/etc/resolv.conf` names the resolver.
