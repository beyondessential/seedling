# Networking internals

This document describes the low-level networking architecture that
seedling uses to connect containers, services, and the outside world.

## Address space

Every seedling node derives a stable /48 ULA prefix from
`/etc/machine-id`. The raw content is SHA-256 hashed and the first four
bytes of the digest fill octets 2–5:

    fd5e:XXYY:ZZWW::/48

where XX YY ZZ WW are `digest[0..4]`. The `fd5e` prefix is fixed.

Within that /48, addresses are carved up as follows:

    fd5e:XXYY:ZZWW::/48              node prefix
    fd5e:XXYY:ZZWW:KKUU::/64        pod network (one per pod instance)
    fd5e:XXYY:ZZWW:KKUU::1/128      bridge gateway (assigned by netavark)
    fd5e:XXYY:ZZWW:KKUU::1000/128   bridge mount endpoint (assigned by seedling)
    fd5e:XXYY:ZZWW:KKUU:..../128    pod container address (SLAAC)
    fd5e:XXYY:ZZWW:ff00::/64        seedling-proxy network (Caddy)
    10.89.255.0/24                   seedling-proxy IPv4 subnet (dual-stack)

The `KK` byte is a `ResourceKind` discriminant:

| Value | Kind            |
|-------|-----------------|
| 0     | Parameter       |
| 1     | Service         |
| 2     | HttpService     |
| 3     | ExternalService |
| 4     | Ingress         |
| 5     | Deployment      |
| 6     | Job             |
| 7     | Volume          |
| 8     | ExternalVolume  |
| 9     | Action          |

The `UU` byte and bytes 8–15 come from the resource instance's persisted
UUID (bytes 0–8 of the UUID). This makes every resource instance's /128
address and every pod's /64 prefix stable across restarts.

The proxy network uses `0xff` as its subnet discriminant (byte 6), which
is above the ResourceKind range and can never collide with a pod prefix.

## Pod networks

Each pod instance gets its own Podman bridge network named
`seedling-<display_name>` with a /64 prefix derived from the instance
identity. Netavark (Podman's network backend) assigns:

- `::1` on the bridge as the gateway
- A SLAAC address to the container

Seedling additionally assigns `::1000` on the host side of the bridge
as the **mount endpoint**. This is the DNAT target that containers hit
when they connect to `localmount`. The `::2` address is intentionally
avoided because netavark sequentially assigns it to the first container
on the network, which would collide.

The bridge interface only exists in the kernel while at least one
container is connected to the network. On startup, seedling's bridge
reconciliation phase silently skips interfaces that don't exist yet
(ENODEV) and retries on the next tick once the container has attached.

## Service routing

Every Service resource gets a stable /128 IPv6 address derived from
the node prefix and the service's persisted instance ID:

    fd5e:XXYY:ZZWW:01UU:UUUU:UUUU:UUUU:UUUU/128

where `01` is the Service kind discriminant and the remaining bytes are
from the UUID.

An `ip -6 route replace` installs a host route for each service:

| Backends | Route                                                           |
|----------|-----------------------------------------------------------------|
| 0        | `blackhole <svc_ip>/128 proto static`                           |
| 1        | `<svc_ip>/128 via <pod_ip> proto static`                        |
| N        | `<svc_ip>/128 nexthop via <pod1> nexthop via <pod2> ...`        |

These routes provide IP-level reachability and ECMP load balancing at
the address layer. Before adding new routes each tick, all existing
seedling-managed routes (static /128s in `fd5e::/16`) are deleted.

## Port translation (service DNAT)

The BSL port model distinguishes **service ports** (endpoint-side) from
**pod ports** (container-side). A deployment binding like
`.http(3000, traffic.route("/"))` means the container listens on 3000
but the service exposes port 80.

The IP routes above only translate addresses, not ports. Port
translation is handled by nftables DNAT rules in the prerouting chain:

    meta nfproto ipv6 ip6 daddr <service_ip> tcp dport 80 dnat ip6 to [<pod_ip>]:3000

For multiple backends, nftables `numgen inc mod N` provides round-robin
load balancing:

    meta nfproto ipv6 ip6 daddr <svc_ip> tcp dport 80 \
      dnat ip6 to numgen inc mod 2 map { 0: <pod1>, 1: <pod2> } : 3000

Service DNAT rules are rebuilt from scratch on every reconciliation tick
alongside all other nftables rules.

## Service mounts

A pod can consume another service via `.mount(svc.port(80))`. Inside
the container, the service is reachable at `localmount:80`.

The `localmount` hostname resolves to the pod's bridge mount endpoint
(`prefix::1000`). nftables DNAT rules in the prerouting chain translate
this to the backing pod:

    ip6 saddr <pod_prefix>::/64 ip6 daddr <pod_prefix>::1000 \
      tcp dport 80 dnat ip6 to [<backend_ip>]:<backend_port>

Mount rules resolve backends at rule-building time using the same
backend collection as service DNAT rules. This avoids a double-DNAT
problem: nftables only processes the prerouting chain once per packet,
so chaining mount DNAT → service DNAT would not work.

## Ingress

An Ingress exposes a service to external traffic. There are two paths
depending on whether the ingress terminates HTTP.

### HTTP ingress (through Caddy)

Ingresses with `.http()` or `.http2()` route through Caddy, which runs
in a container on the dual-stack `seedling-proxy` network. The proxy
network has both an IPv6 /64 (`fd5e:XXYY:ZZWW:ff00::/64`) and a fixed
IPv4 /24 (`10.89.255.0/24`).

The traffic path for an external IPv6 client:

    client → host:80
      → nftables prerouting:
          meta nfproto ipv6 fib daddr type local tcp dport 80
          dnat ip6 to [<caddy_ipv6>]:80
      → Caddy receives, matches Host header, reverse-proxies
      → upstream: http://[<service_ip>]:80
      → nftables prerouting (service DNAT):
          ip6 daddr <service_ip> tcp dport 80
          dnat ip6 to [<pod_ip>]:3000
      → pod receives on :3000

For IPv4 clients, a parallel set of rules DNATs to Caddy's IPv4 address
on the proxy bridge:

    meta nfproto ipv4 fib daddr type local tcp dport 80
    dnat ip to <caddy_ipv4>:80

Caddy then proxies upstream over IPv6 to the service backends. This
gives dual-stack ingress without any IPv4 on pod networks.

Both prerouting and output chains carry identical ingress DNAT rules.
The output chain rules catch host-originated traffic (e.g., `curl
localhost:80`) via the `fib daddr type local` guard.

The `fib daddr type local` guard is essential: without it, the
prerouting dport-80 rule would catch Caddy's own upstream traffic to
`service_ip:80` and loop it back to Caddy.

Caddy's configuration is applied via its admin API (`POST /config/`).
The JSON payload always includes `"admin": {"listen": ":2019"}` to
preserve the admin listener across full-config replacements. Caddy
listens on `:2019` on all interfaces inside its container.

### Direct ingress (TCP/UDP, no HTTP termination)

Ingresses without `.http()` / `.http2()` bypass Caddy entirely and
DNAT straight to pod backends:

    meta nfproto ipv6 fib daddr type local tcp dport <port> \
      dnat ip6 to [<pod_ip>]:<pod_port>

Multiple backends use `numgen inc mod N` for round-robin, same as
service DNAT rules. Direct ingresses are IPv6-only; IPv4 support
for non-HTTP ingress is deferred to NAT64.

## nftables table structure

All rules live in a single `inet` (dual-stack) table:

    table inet seedling_net {
      chain prerouting {
        type nat hook prerouting priority dstnat; policy accept;
        # ingress DNAT (IPv6 + IPv4 for HTTP, IPv6 for direct)
        # mount DNAT
        # service DNAT
      }
      chain output {
        type nat hook output priority dstnat; policy accept;
        # ingress DNAT (same targets as prerouting)
      }
      chain forward {
        type filter hook forward priority filter; policy accept;
        # single rule: accept fd5e:ed00::/24 ↔ fd5e:ed00::/24
      }
    }

The entire table is flushed and rebuilt atomically on every
reconciliation tick. The forward chain carries a single blanket accept
rule allowing all forwarded traffic within the seedling ULA range.

## Reconciliation order

The reconciler runs these phases sequentially each tick:

1. **Pods** — observe and actuate containers for each app. Collects
   running pod IPs and updates the bridge name map.
2. **Uninstall** — for apps being uninstalled, checks whether all pods
   and systemd units are gone.
3. **Bridges + Volumes** — ensures `::1000` mount endpoints are assigned
   on bridge interfaces; observes and actuates volumes.
4. **Routes** — builds and applies `ip -6 route replace` for every
   service across all apps.
5. **Caddy** — ensures the Caddy container is running and healthy. If
   this fails, phases 6 and 7 are skipped for this tick.
6. **nftables** — builds and atomically applies all ingress, mount, and
   service DNAT rules.
7. **Proxy config** — builds the Caddy JSON config from ingress/service
   pairs and applies it via the admin API. Caches the config to disk
   for upgrade continuity.