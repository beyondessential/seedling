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

The IPv6 /48 unique local prefix the runtime derives for its host, within which every address it assigns falls.

### Pod network

The IPv6 bridge network, with its own /64 prefix within the host prefix, to which one pod instance is connected.

### Mount endpoint

The address at which a pod reaches the services it [mounts](../language/pod.md#service-mounts).

## Unavailable backends

If the container runtime or the process supervisor cannot be reached when the runtime starts, the runtime MUST NOT exit or restart repeatedly.
It MUST still bring up the [Operator Interface](../overview.md#operator-interface), the database and the reconciliation loop.

While either backend is unavailable, the runtime MUST NOT actuate workloads, or infrastructure that depends on the unavailable backend.
On each reconciliation iteration while either is unavailable, the runtime MUST file a [`container_runtime_unavailable`](faults.md#host-faults) fault.

The runtime MAY remain without the backends until it is restarted.

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

The runtime MUST derive the host prefix from the host's machine identifier, read from `/etc/machine-id` with surrounding whitespace removed.
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
> Instances that carry no unique identifier of their own are the case that has collided.

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
The mount endpoint need not be assigned to any network interface.

The runtime MUST translate traffic from a pod's network prefix to the mount endpoint, at a port the pod mounts, directly to the address and pod-side port of a backend of the mounted service.
The translation MUST NOT pass through the service's own address.
The runtime MUST scope each translation to the mounting pod's network prefix.
Several pods MAY mount services at the same port, each reaching its own mounted service.
Where a mounted service has several backends, the runtime MUST distribute new connections across them.

## Service translation

The runtime MUST translate traffic addressed to a service's address and declared service port to the address and pod-side port of one of the service's backends.
This MUST apply to traffic forwarded through the host and to traffic originating on the host.
Where a service has several backends, the runtime MUST distribute new connections across them.

The runtime MUST apply the service translation, mount translation, ingress redirection and forwarding policy rules together, as one atomic replacement of the rules it last applied.
These rules are [absolute state](reconciliation.md#absolute-state).

## Host-originated traffic

The runtime MUST redirect traffic originating on the host and addressed to an ingress port on any of the host's own addresses to the proxy, as it redirects traffic arriving from the network.
The redirection MUST cover the same ports and protocols as the redirection of traffic from the network.
It MUST apply only to IPv6 traffic addressed to a local address.

A connection from a host process over the loopback interface that the runtime redirects MUST receive the proxy's replies.

## Forwarding policy

The runtime MUST drop forwarded packets addressed to the host prefix, except:

- packets of a connection already accepted, or related to one;
- packets of a connection whose destination the runtime's ingress redirection rewrote;
- packets whose source address is also within the host prefix.

The runtime MUST NOT drop or otherwise filter forwarded traffic addressed outside the host prefix.
