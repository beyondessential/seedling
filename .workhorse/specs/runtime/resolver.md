---
id: DNS
---

# Resolver

## Resolver

<!-- from runtime: Infrastructure / Resolver -->

> r[infra.resolver]
> The runtime must run a resolver infrastructure container that provides DNS forwarding and
> caching to all workload containers. The resolver container follows the same lifecycle as
> the proxy container: it is started when workloads are present and torn down when no
> workloads remain.

> r[infra.resolver.config]
> The runtime must generate a resolver configuration that forwards all queries to the
> configured upstreams (see [infra.resolver.upstreams](rule:r/infra.resolver.upstreams)) and
> caches responses. When [NAT64 is active](rule:r/infra.nat64.mode), the configuration must
> additionally enable DNS64 synthesis of AAAA records under the well-known prefix
> `64:ff9b::/96`.

> r[infra.resolver.upstreams]
> The runtime must accept a command-line argument that specifies an explicit list of
> upstream DNS servers (`host:port` addresses) for the resolver to forward to. When the
> argument is unset, the runtime must arrange for the resolver to forward to the host's
> system DNS (so containers inherit the host's split-DNS and search-domain configuration);
> this arrangement must work even when the host system DNS listens only on loopback
> addresses unreachable from inside the resolver container.

> r[infra.resolver.address]
> The resolver must listen on a stable node-wide IPv6 address derived from the node prefix,
> so that all workload containers can reach it at a predictable address. The address is
> `<prefix>:fd00::53` within the resolver network, where `53` is the hexadecimal last
> group, so that operators can write firewall and monitoring rules against it.

> r[infra.resolver.startup]
> The runtime must ensure the resolver container is running and healthy before beginning the
> reconciliation loop. On each startup, the runtime verifies resolver health and restarts
> the resolver if necessary.

> r[infra.resolver.upgrade]
> When the resolver container image changes, the runtime must replace the running resolver
> with one started from the new image, in the resolver slot that is not active, at the
> [resolver address](rule:r/infra.resolver.address). The resolver address can be held by one
> container at a time, so the replacement stops the running resolver first and resolution
> is briefly unavailable. The active slot is persisted in the database, and a replacement
> interrupted at any point must be completed on a later reconciliation tick without
> operator action.

> r[infra.resolver.config.apply]
> The resolver must serve the configuration the runtime currently generates. The runtime
> records on each resolver container the configuration and address it was started with, and
> when that differs from what is currently generated, or is absent, it must replace the
> container as for an [image change](rule:r/infra.resolver.upgrade).

## Container DNS

<!-- from runtime: Infrastructure / Container DNS -->

> r[infra.pod.dns]
> Every workload container must be configured to use the [resolver](rule:r/infra.resolver) as
> its DNS server. The runtime must pass the resolver's address to the container runtime so
> that the container's `/etc/resolv.conf` points at the resolver.
