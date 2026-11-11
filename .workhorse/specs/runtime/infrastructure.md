---
id: INFRA
---

# Host infrastructure

Applies to the host-level state the [runtime](../overview.md#runtime) keeps beneath apps on a [host](../overview.md#host): its key and database files, its address space, pod networks, and the packet rules that carry traffic between them.

## Borrowed terms

| term | meaning |
| --- | --- |
| unique local address | An IPv6 address within `fc00::/7`, routable only within a site, [RFC 4193](https://www.rfc-editor.org/rfc/rfc4193). |

## Terminology

### Host prefix

The IPv6 /48 unique local prefix the runtime derives for its host, within which every IPv6 address it assigns falls.

### Pod network

The IPv6 bridge network, with its own /64 prefix within the host prefix, to which one pod instance is connected.

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

The runtime MUST connect each pod instance to a pod network of its own.
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

## Mount endpoint addressing

The mount endpoint MUST be the address `<host prefix>:fffe::1`, the same for every pod on the host.
The runtime MUST resolve the hostname `localmount` to the mount endpoint in every pod that mounts a service.

> [!NOTE]
> The mount endpoint is reached through translation, so no network interface needs to carry the address.

The runtime MUST translate traffic from a pod's network prefix to the mount endpoint, at a port the pod mounts, directly to the address and pod-side port of a backend of the mounted service.
The translation MUST NOT pass through the service's own address.
The runtime MUST scope each translation to the mounting pod's network prefix.
Several pods MAY mount services at the same port, each reaching its own mounted service.
Where a mounted service has several backends, the runtime MUST distribute new connections across them.

## Service addresses

The runtime MUST NOT give two services on a host the same address.
A service's address MUST lie outside every pod network prefix.
A service's address MUST differ from every address the runtime reserves for its own infrastructure, including the mount endpoint.

## Service translation

The runtime MUST translate traffic addressed to a service's address and declared service port to the address and pod-side port of one of the service's backends.
The translation MUST apply to traffic forwarded through the host and to traffic originating on the host.
Where a service has several backends, the runtime MUST distribute new connections across them.

The runtime MUST apply the service translation, mount translation, [ingress redirection](#ingress-redirection) and forwarding policy rules together, as one atomic replacement of the rules it last applied.
These rules are [absolute state](reconciliation.md#absolute-state).

## Ingress redirection

The runtime MUST redirect traffic arriving from the network at an ingress port on any of the host's own addresses to the proxy, at the same port.
The ingress ports and their protocols are:

| source | port | protocols |
| --- | --- | --- |
| an app's [ingress](../language/ingress.md) | the port it declares | UDP for an ingress that terminates DTLS; TCP, and UDP as well, for one that terminates HTTPS; TCP otherwise |
| an app ingress's [HTTP redirect](../language/ingress.md#redirect-to-https) | the port the redirect names | TCP |
| a [site ingress attachment](site-resources.md#attachments) | the attachment's port | UDP for a `udp` attachment; otherwise TCP, and UDP as well for an `http` or `http2` attachment on a site ingress that terminates TLS |

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
- packets of a connection whose destination the runtime's ingress redirection rewrote;
- packets whose source address is also within the host prefix.

The runtime MUST NOT drop or otherwise filter forwarded traffic addressed outside the host prefix.
