---
id: INFRA
---

# Host infrastructure

Applies to the host-level state the [runtime](../overview.md#runtime) keeps beneath apps on a [host](../overview.md#host): its key and database files, its address space, pod networks, and the packet rules that carry traffic between them.

## Borrowed terms

| term | meaning |
| --- | --- |
| neighbour discovery | The ICMPv6 messages by which nodes on a link resolve each other's link-layer addresses and confirm reachability, [RFC 4861](https://www.rfc-editor.org/rfc/rfc4861). |
| unique local address | An IPv6 address within `fc00::/7`, routable only within a site, [RFC 4193](https://www.rfc-editor.org/rfc/rfc4193). |

## Terminology

### Host prefix

The IPv6 /48 unique local prefix the runtime derives for its host, within which every IPv6 address it assigns falls.

### Pod network

The IPv6 bridge network, with its own /64 prefix within the host prefix, to which one pod instance, or the container of one [shell session](../interface/shells.md#shell-session), is connected.

### Mount endpoint

The address at which a pod reaches the services it [mounts](../language/pod.md#service-mounts).

## Unavailable backends

If the container runtime or the process supervisor cannot be reached when the runtime starts, the runtime MUST NOT exit or restart repeatedly.
It MUST still bring up the [Operator Interface](../overview.md#operator-interface), the database and the reconciliation loop.

While either backend is unavailable, the runtime MUST NOT actuate workloads, or infrastructure that depends on the unavailable backend.
On each reconciliation iteration while either is unavailable, the runtime MUST file a [`container_runtime_unavailable`](faults.md#host-faults) fault.

The runtime MUST probe each unavailable backend again on later reconciliation iterations.
Successive probes of an unavailable backend MUST be spaced by an increasing delay up to a fixed maximum, and the runtime MUST keep probing at that maximum however long the backend stays unavailable.
Once both backends are reachable, the runtime MUST clear `container_runtime_unavailable` and actuate workloads as usual.

## File permissions

The runtime MUST create every private key file, the database file, and every file kept beside the database file for the database's own use, with permissions granting read and write to the owner and nothing to group or others (mode `0600`).
Each such file MUST carry those permissions from the moment it is created, before any content is written to it.
The runtime MUST refuse to use a key file or database file whose group or other permission bits are set, and MUST report an error naming the file and its mode.

## Database

The runtime MUST keep its database in the file `seedling.db` in its [data directory](actuation.md#data-directory).

A writer that updates a stored record that another writer also updates MUST change only the fields it owns.
It MUST leave every other field as it was, including fields added to the record after the writer was written.

The runtime MUST wait for a database write that is blocked by a concurrent writer, rather than failing it immediately.
It SHOULD wait at least five seconds before failing such a write.

## Host prefix derivation

The runtime MUST derive the host prefix from the host's machine identifier, as the [platform](../platform/linux.md#host-facilities) provides it.
The prefix MUST be `fd5e:` followed by the first four bytes of the SHA-256 ([FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)) digest of that identifier, as a /48:

| bytes | value |
| --- | --- |
| 0 to 1 | `fd 5e` |
| 2 to 5 | bytes 0 to 3 of the digest |

If the machine identifier cannot be read or is empty, the runtime MUST NOT start.

> [!NOTE]
> For a digest beginning `12 34 56 78`, the host prefix is `fd5e:1234:5678::/48`.
> The prefix is stable for as long as the machine identifier is, so addresses derived from it survive restarts.

## Pod networks

The runtime MUST connect each pod instance, and each shell session's container, to a pod network of its own.
A pod network MUST be IPv6 only, and the runtime MUST NOT allocate an IPv4 subnet on it.
Its prefix MUST be a /64 within the host prefix, disjoint from every address the runtime reserves for its own infrastructure.
Its gateway MUST be the first host address of its prefix (`<prefix>::1`).

The runtime MUST NOT give two concurrently running pod instances the same pod network prefix.
It MUST allocate each instance's prefix and record the allocation.

> [!NOTE]
> A prefix derived from an instance's identity guarantees uniqueness only to a probability, however wide the derivation, and a derivation from a part of the identity that distinct instances share guarantees nothing.
> Instances that carry no unique identifier of their own are where derived prefixes collide.

## Pod network cleanup

When it starts, the runtime MUST remove each workload container belonging to an app that is not registered, and the pod network that container used.
It MUST then keep a pod network only while a container belonging to a registered app, or to a live operator session, is using it, and remove the rest.
The runtime MUST judge whether a pod network is in use only after removing those containers.

The runtime MUST NOT take a container or network listing it could not read for an empty one.
If a listing cannot be read, the runtime MUST NOT remove any pod network on the strength of it.

The runtime MUST NOT remove the networks of its own infrastructure containers during this cleanup.

## Pod network sources

The runtime MUST drop every packet arriving from a pod network whose source address lies outside that network's prefix, before applying any translation to it.
Packets with a link-local or unspecified source address are exempt.
The runtime MUST keep this rule in force whenever a container on a pod network is running: before it first applies the rules it holds as absolute state, while an application of them is withheld, and across a restart of the runtime.

> [!NOTE]
> Every other rule that scopes traffic by its source prefix relies on this one: without it, a container that can send raw packets could take on another pod's address and be translated or forwarded as that pod.

## Mount endpoint addressing

The mount endpoint MUST be the address `<host prefix>:fffe::1`, the same for every pod on the host.
The runtime MUST resolve the hostname `localmount` to the mount endpoint in every pod that mounts a service, and in the container of every shell session whose [job](../language/job.md) mounts one.

> [!NOTE]
> The mount endpoint is reached through translation, so no network interface needs to carry the address.

The runtime MUST translate traffic from a pod's network prefix to the mount endpoint, at a port the pod mounts, directly to the address and pod-side port of a backend of the mounted service.
The translation MUST NOT pass through the service's own address.
The runtime MUST scope each translation to the mounting pod's network prefix.
Several pods MAY mount services at the same port, each reaching its own mounted service.
Where a mounted service has several backends, the runtime MUST distribute new connections across them.

The runtime MUST translate traffic from a shell session's container to the services its job mounts as it does for a pod, scoped to the session's pod network.
The runtime MUST have those translations in place before the session's container starts, and MUST keep them until the session ends.

## Service addresses

The runtime MUST NOT give two services on a host the same address.
A service's address MUST lie outside every pod network prefix.
A service's address MUST differ from every address the runtime reserves for its own infrastructure, including the mount endpoint.

## Service translation

The runtime MUST translate traffic addressed to a service's address and declared service port to the address and pod-side port of one of the service's backends.
The translation MUST apply to traffic arriving from the [proxy network](proxy.md#proxy-addresses) and to traffic originating on the host.
The runtime MUST NOT translate traffic addressed to a service's address that arrives from anywhere else.
Where a service has several backends, the runtime MUST distribute new connections across them.

> [!NOTE]
> A pod reaches a service through the mount endpoint, never through the service's address.

The runtime MUST apply the service translation, mount translation and [ingress redirection](#ingress-redirection) rules together, as one atomic replacement of the rules it last applied.
These rules are [absolute state](reconciliation.md#absolute-state).

## Ingress redirection

The runtime MUST redirect traffic arriving from the network at an ingress port on any of the host's own addresses to the proxy, at the same port.
The ingress ports and their protocols are:

| source | port | protocols |
| --- | --- | --- |
| an app's [ingress](../language/ingress.md) | the port it declares | UDP for an ingress that terminates DTLS; TCP, and UDP as well, for one that terminates HTTPS; TCP otherwise |
| an app ingress's [HTTP redirect](../language/ingress.md#redirect-to-https) | the port the redirect names | TCP |
| a [site ingress attachment](site-resources.md#attachments) | the attachment's port | UDP for a `udp` attachment; otherwise TCP, and UDP as well for an `http` or `http2` attachment on a site ingress that terminates TLS |

The runtime MUST redirect a port for a protocol only where the proxy serves a [claim](proxy.md#shared-ports) on that port and protocol.
An ingress, redirect or site ingress attachment that is left out of the proxy configuration, whether for a [conflict](../language/ingress.md#claims-across-apps) or because the attachment's target cannot be resolved, MUST NOT cause its port to be redirected.

The runtime MUST redirect IPv6 traffic to the proxy's IPv6 address.
Where the proxy has an IPv4 address, the runtime MUST also redirect IPv4 traffic to it.
The runtime MUST NOT redirect the ports of an app that is uninstalling.

## Host-originated traffic

The runtime MUST redirect traffic originating on the host and addressed to an ingress port on any of the host's own addresses to the proxy, as it [redirects](#ingress-redirection) traffic arriving from the network.
The redirection MUST cover the same ports and protocols as the redirection of traffic from the network.
It MUST apply only to traffic addressed to a local address.

A connection from a host process over the loopback interface that the runtime redirects MUST receive the proxy's replies.

## Forwarding policy

The runtime MUST drop forwarded packets addressed to the host prefix, except:

- packets of a connection already accepted, or related to one;
- packets of a connection whose destination the runtime rewrote, by ingress redirection, mount translation or service translation;
- packets arriving from the proxy network;
- TCP and UDP packets from within the host prefix, addressed to the [resolver address](resolver.md#resolver-addresses) at port 53.

The runtime MUST NOT forward traffic from one pod network to another other than through a [mount translation](#mount-endpoint-addressing).
The runtime MUST keep this policy in force whenever a container on a pod network is running: before it first applies the rules it holds as absolute state, while an application of them is withheld, and across a restart of the runtime.

The runtime MUST NOT drop or otherwise filter forwarded traffic addressed outside the host prefix.

## Pod traffic to the host

The runtime MUST drop packets arriving from a pod network and addressed to any of the host's own addresses, except:

- packets of a connection already accepted, or related to one;
- neighbour discovery messages.

The runtime MUST keep this rule in force whenever a container on a pod network is running: before it first applies the rules it holds as absolute state, while an application of them is withheld, and across a restart of the runtime.

> [!NOTE]
> Replies to the host's own connections into a pod, such as its probes and port-forward relays, are packets of an accepted connection.
> Traffic from a pod to an ingress port on one of the host's addresses is redirected to the proxy before it would reach the host, so it is forwarded rather than delivered here.
