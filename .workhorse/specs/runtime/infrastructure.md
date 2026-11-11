---
id: INFRA
---

# Host infrastructure

## Fault Handling

<!-- from runtime: Fault Handling -->

> r[infra.pod.subnet]
> No two concurrently running pod instances may share a network prefix.
> A prefix must therefore not be derived from a part of an instance's identity that distinct instances can share: in particular, instances carrying no unique identifier of their own must still be distinguished by the rest of their identity.
> Deriving a prefix, however widely, does not on its own satisfy this requirement — only allocating prefixes and recording the allocation makes it a guarantee rather than a probability.

## Infrastructure

<!-- from runtime: Infrastructure -->

> r[infra.key.file-permissions]
> Private key files must be created with owner-read/write-only permissions. The runtime
> must refuse to use a key file whose group or world permission bits are set, and must
> report an error.

> r[infra.db.file-permissions]
> The database file must be created with owner-read/write-only permissions. The runtime
> must refuse to open a database file whose group or world permission bits are set, and
> must report an error.

> r[infra.db.busy-timeout]
> When a database write is blocked by a concurrent writer, the runtime must wait and retry
> rather than failing immediately.

> r[infra.node.prefix]
> The runtime must derive a stable per-node /48 IPv6 prefix from the host machine identity.
> The prefix follows the ULA format `fd5e:edXX:XXXX::/48`, where the 24-bit host portion is
> derived from `/etc/machine-id`.

> r[infra.node.degraded]
> If the container runtime or the process manager cannot be reached at startup, the runtime
> must not crash or crash-loop. It must still bring up the operator interface, database, and
> reconciliation loop so the failure is observable, and it must file a system-wide fault whose
> description identifies which backend is unavailable and states that no workloads will be
> actuated until the runtime is restarted with working backends. While degraded, the runtime
> must not attempt to actuate workloads or infrastructure that depend on the unavailable
> backends. Recovery requires a restart; the runtime is not required to reconnect on its own.

> r[infra.pod.network]
> Each pod instance must be connected to a dedicated IPv6-only bridge network. The network
> prefix is a /64 derived from the node prefix and the instance identity. No IPv4 subnet
> must be allocated on pod networks.
> The network's gateway is the first host address of its prefix, so it follows from the
> prefix and is distinct for every distinct prefix.

> r[infra.pod.network.cleanup]
> At startup, the runtime must remove each workload container that belongs to an app that is no longer registered, and the pod network it used. A pod network is kept only while a container that belongs to a registered app, or to a live operator session, is using it.
> Whether a network is in use must be judged after those containers have been removed, so that removing a container is never what leaves its network behind with nothing to revisit it.
> A container or network listing that cannot be read must not be taken for an empty one: networks are not removed on the strength of a failed listing.

> r[infra.pod.mount]
> Pods that mount a service must resolve the `localmount` hostname to a stable node-wide
> address at `fd5e:XXYY:ZZWW:fffe::1`. nftables DNAT rules scope traffic to this address
> by source pod prefix, so multiple pods may mount the same service simultaneously without
> collision. The mount endpoint address does not need to be assigned to any network
> interface.

> r[infra.dataplane.output-nat]
> The runtime must install DNAT rules in an nftables `output` chain (in addition to the
> `prerouting` chain) so that host-originated traffic directed at ingress ports on any
> local address is redirected to the proxy. The output rules must be restricted to
> locally-destined packets (`fib daddr type local`) and to the IPv6 address family, and
> must mirror the port and protocol set of the prerouting ingress rules.

> r[infra.dataplane.service-dnat]
> The runtime must install DNAT6 rules in the nftables `prerouting` chain that translate
> traffic destined for a service's stable IPv6 address and declared service port to a
> backing pod's address and pod-side port. When multiple backends are available, the
> runtime must distribute new connections across them. These rules are applied atomically
> alongside ingress and mount rules.

> r[infra.dataplane.mount-dnat]
> Mount DNAT rules must translate directly from the pod's mount endpoint address and mount
> port to a backing pod's address and pod-side port, without an intermediate hop through
> the service address. When multiple backends are available, the runtime must distribute
> new connections across them.

> r[infra.dataplane.forward-policy]
> The nftables forward chain must drop unsolicited new inbound forwarded connections directed
> at any address within the node's /48 prefix. The forward chain must explicitly permit:
> conntrack-established and related connections; new connections that have been redirected by
> a prerouting NAT rule (ingress traffic); and traffic whose source and destination both fall
> within the node's /48 prefix. The chain's default policy must remain accept so that
> forwarded traffic unrelated to the node's /48 prefix is not affected.
