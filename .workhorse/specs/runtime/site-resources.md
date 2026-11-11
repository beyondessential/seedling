---
id: SITE
---

# Site resources

Applies to the volumes, services and ingresses that [operators](../overview.md#operator) manage on a [host](../overview.md#host) independently of any [app](../overview.md#app), and to the mappings that bind an app's external volumes and services to them.

## Borrowed terms

| term | meaning |
| --- | --- |
| A record | a DNS resource record carrying an IPv4 address, as defined by [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035) |
| AAAA record | a DNS resource record carrying an IPv6 address, as defined by [RFC 3596](https://www.rfc-editor.org/rfc/rfc3596) |
| NAT64 | translation from IPv6 clients to IPv4 servers, as defined by [RFC 6146](https://www.rfc-editor.org/rfc/rfc6146) |
| Well-Known Prefix | the IPv6 prefix `64:ff9b::/96` for IPv4-embedded addresses, as defined by [RFC 6052](https://www.rfc-editor.org/rfc/rfc6052) |
| redirect | an HTTP response with a 3xx status code, as defined by [RFC 9110 §15.4](https://www.rfc-editor.org/rfc/rfc9110#section-15.4) |
| ACME | the Automatic Certificate Management Environment protocol, as defined by [RFC 8555](https://www.rfc-editor.org/rfc/rfc8555) |

## Terminology

### Site volume

A named volume an operator manages on a host, which apps can use through their external volumes.

### Site service

A named service an operator manages on a host, which forwards to addresses outside the host's apps.

### Site ingress

A named hostname an operator or a discovery provider manages on a host, through which traffic can reach an app's services.

### Attachment

The binding of one port and protocol on a site ingress to a target.

### Discovery provider

A part of the runtime that creates and maintains site ingresses from the host's own configuration.

## Site volumes

A site volume is one of three kinds:

| kind | storage |
| --- | --- |
| managed | a directory the runtime creates and maintains in the [data directory](actuation.md#data-directory), as a BTRFS subvolume where the data directory is on BTRFS |
| bind | a host path the operator supplies, mounted as it is |
| snapshot | a read-only BTRFS snapshot of another volume, created by the runtime |

A site volume is mapped read-write or read-only.
The runtime MUST mount a snapshot site volume read-only, whatever its mapping says.

Site volumes MUST be created and deleted only through operator commands.
The runtime MUST create a managed site volume's storage when the volume is created.

When an operator deletes a managed site volume, the runtime MUST [hold](actuation.md#held-volumes) its data.
When an operator deletes a snapshot site volume, the runtime MUST delete its storage.
When an operator deletes a bind site volume, the runtime MUST forget the volume and MUST leave the host path untouched.

The runtime MUST emit an [event](../interface/events.md) and record an [audit log](audit-log.md#entries) entry when an operator creates or deletes a site volume.
A deletion event MUST identify the site volume's kind, and the held volume created when the deletion held the volume's data.
A snapshot, a promotion and a [restore](actuation.md#held-volumes) each bring a site volume into being with an event of their own, and the runtime MUST NOT emit a creation event for them as well.

### Site volume names

The runtime MUST reject, as an invalid request, every operation that would bring a new site volume into being under a name that is taken.
These operations are creating a site volume, snapshotting to one, promoting a snapshot into one, and [restoring a held volume](actuation.md#held-volumes) as one.
The runtime MUST reject the operation before it touches any storage.

A name is taken when a site volume is recorded under it, and when storage exists at the name without a record.
Checking that a name is free and taking it MUST be one indivisible step, so that of two concurrent operations naming the same site volume, exactly one proceeds and the other is rejected.

> [!NOTE]
> Storage operations do not refuse a name in use on their own terms: a snapshot taken onto an existing directory is placed inside it, leaving a subvolume nested in an unrelated volume with no record pointing at it.
> Unrecorded storage is refused rather than taken over because leftovers cannot be told from an operator's volume whose record is not yet written.

### Site volume snapshots

An operator MAY snapshot an app volume or a managed site volume into a new snapshot site volume, when that volume is stored as a BTRFS subvolume.
A snapshot site volume MUST record the volume it was taken from.
The runtime MUST reject a request to snapshot a volume that is not stored as a BTRFS subvolume.

The runtime MUST emit an event when it creates a snapshot site volume, identifying the new snapshot and its source volume, app or site.

### Snapshot promotion

An operator MAY promote a snapshot site volume into a new managed site volume under a name the operator chooses, when the data directory is on BTRFS.
The runtime MUST create the new volume as a writable copy of the snapshot's contents, and record it as a managed site volume.
Promotion MUST leave the snapshot unchanged.
On a host whose data directory is not on BTRFS, the runtime MUST reject a promotion request.

The runtime MUST emit an event when it promotes a snapshot, identifying the new managed site volume and the source snapshot.

## External mappings

An operator maps an app's external volume to a site volume or an app's exported volume, and an app's external service to a site service or another app's service.

The runtime MUST emit an event and record an audit log entry when an external volume mapping or an external service mapping is created, removed, or retargeted.
The event MUST identify the app, the external name, and the new target, or that the mapping was removed.
A retargeting event MUST also identify the previous target.

## Site services

A site service has a name, an optional description, and one or more endpoints.
Each endpoint has these members:

| member | meaning |
| --- | --- |
| `service_port` | the port on which the site service exposes this endpoint |
| `protocol` | `tcp`, `udp` or `http` |
| `remote_host` | the address traffic is forwarded to: an IPv6 literal, an IPv4 literal, or a DNS name |
| `remote_port` | the port traffic is forwarded to |

The `(service_port, protocol)` pairs across a site service's endpoints are the ports the site service exposes.
The runtime MUST distribute traffic for a site service's `(service_port, protocol)` across the `(remote_host, remote_port)` of every endpoint with that pair, as for any [service with several backends](../language/service.md#routing).

### Endpoint addressing

The runtime MUST route each endpoint as follows:

| `remote_host` | routed |
| --- | --- |
| an IPv6 literal | natively, to that address |
| an IPv4 literal | through NAT64 |
| a name with AAAA records, on a host with IPv6 egress | natively, to the AAAA addresses |
| a name with A records and no usable AAAA records | through NAT64, to the A addresses |

On a host without IPv6 egress, AAAA records are unusable.
The runtime MUST route through NAT64 by embedding the IPv4 address in the Well-Known Prefix.

The runtime MUST report an endpoint as unroutable when it needs NAT64 and NAT64 is not [active](nat64.md#active-nat64), or when its name has only AAAA records and the host has no IPv6 egress.
The runtime MUST report an endpoint as unresolvable when resolving its name has failed 5 consecutive times.
An unroutable or unresolvable endpoint MUST raise the [site service faults](faults.md#ingress-and-service-faults) for its site service.
An unroutable or unresolvable endpoint MUST NOT affect the routing of other endpoints, site services or apps.

### Partial DNS answers

The runtime MUST resolve a name from the address families whose lookups succeeded.
When the lookup for one family fails and the other returns records, the runtime MUST resolve the endpoint from those records.
The runtime MUST keep the failed family's last answer.
The runtime MUST look the name up again promptly, without waiting for the answer's lifetime to pass.
A failed lookup MUST NOT be recorded as the family having no records.

A failure MUST count towards the unresolvable threshold only when no lookup returned records and at least one lookup failed.
The `/services/site/resolver-status` method of the [Operator Interface](../overview.md#operator-interface) MUST name each family whose lookup is failing, for as long as it fails.

### Site service lifecycle

Site services MUST be created and deleted, and have endpoints added and removed, only through operator commands.

The runtime MUST reject the deletion of a site service while an external service mapping targets it.
Deleting a site service MUST remove it and its endpoints.

The runtime MUST emit an event and record an audit log entry when a site service is created or deleted, and when an endpoint is added or removed.
An endpoint event MUST identify the site service and the endpoint's `(service_port, protocol, remote_host, remote_port)`.

## Site ingresses

A site ingress has a name, a hostname, an optional description, a TLS provisioning mode, a source, and zero or more attachments.

### Sources

A site ingress is either manual or discovered.
A manual site ingress MUST be created, updated and deleted only through operator commands.
A discovered site ingress MUST be created, updated and deleted only by its discovery provider, and carries the provider's name and a provider key that stays the same when the hostname changes.

The runtime MUST reject an operator command to delete a discovered site ingress while its discovery provider still reports it.
When a discovery provider stops reporting a site ingress it created without reporting a different identity, the runtime MUST mark the site ingress stale and MUST NOT delete it.
The runtime MUST NOT serve a stale site ingress, and MUST serve it again once its provider reports it.

Deleting a site ingress MUST delete its attachments.

### TLS provisioning modes

| mode | certificate for the hostname |
| --- | --- |
| `acme` | issued by a public certificate authority over ACME |
| `internal` | issued by the runtime's internal certificate authority |
| `tailscale` | obtained from the discovery provider's own certificate facility; valid only on a site ingress discovered by `tailscale` |
| `none` | none; no attachment terminates TLS |

An `http` or `http2` attachment on a site ingress whose mode is `none` is a [plaintext ingress](proxy.md#plaintext-ingresses).

### Attachments

An attachment is keyed by `(port, protocol)` within its site ingress, and its target is one of:

| target | effect |
| --- | --- |
| forward | traffic for the attachment's port and protocol is routed to a named service of a named app |
| redirect | requests for the attachment's port and protocol are answered with a redirect to a configured URL, with a configured status code, optionally keeping the request path |

An attachment's protocol is one of:

| protocol | the attachment accepts | a forward target's service receives |
| --- | --- | --- |
| `tcp` | TCP connections, passed through without TLS termination | the TCP stream as it arrived |
| `udp` | UDP datagrams, passed through without DTLS termination | the datagrams as they arrived |
| `http` | HTTP, over TLS unless the site ingress's mode is `none` | plaintext HTTP/1.1 |
| `http2` | HTTP, over TLS unless the site ingress's mode is `none` | cleartext HTTP/2 |

The runtime MUST serve a redirect target only on an `http` or `http2` attachment.
An attachment takes its hostname and TLS provisioning mode from its site ingress.
A site ingress MAY carry several attachments that differ in `(port, protocol)`.

The runtime MUST emit an event and record an audit log entry when a site ingress is created, updated or deleted, by an operator or by a discovery provider, and when an attachment is added, updated or removed.
The event MUST identify the site ingress, and for an attachment the `(port, protocol)` and the target.

### Tailscale discovery

The Tailscale discovery provider maintains one discovered site ingress, named `tailscale`, for the host's identity on its tailnet.
Its hostname MUST be the host's tailnet DNS name, and its provider key the host's stable Tailscale node identity.

When the node is renamed, the runtime MUST update the site ingress's hostname in place and keep its attachments.
When the node identity changes, the runtime MUST delete the discovered site ingress and create a new one.
When the provider reports that the node is logged out or has no identity, the site ingress becomes stale.

The discovered site ingress's certificate is obtained as [Tailscale certificates](tls/certificates.md#tailscale-certificates) specifies.

### Ingress conflicts

When an app's ingress and a site ingress attachment both claim the same `(hostname, port)`, the runtime MUST leave both out of the proxy configuration for as long as the conflict lasts.
The runtime MUST raise the [`ingress_conflict`](faults.md#ingress-and-service-faults) fault for each side.

When attachments of two site ingresses both claim the same `(hostname, port)`, the runtime MUST serve the attachment created first, and MUST leave the other out of the proxy configuration.
The runtime MUST raise the `ingress_conflict` fault for the site ingress whose attachment it leaves out.
Once the served attachment no longer claims the hostname and port, the runtime MUST serve the other.

A conflict on one port MUST NOT affect the serving of other ports on the same hostname.
