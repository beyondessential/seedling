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
    fd5e:XXYY:ZZWW:KKUU:..../128    pod container address (SLAAC)
    fd5e:XXYY:ZZWW:fffe::1/128      node-wide mount endpoint
    fd5e:XXYY:ZZWW:ff00::/64        seedling-proxy network (Caddy)
    10.89.255.0/24                   seedling-proxy IPv4 subnet (dual-stack)

The `KK` byte is a `ResourceKind` discriminant:

| Value | Kind            |
|-------|-----------------|
| 0     | Parameter       |
| 1     | Service         |
| 2     | HttpService     |
| 3     | Ingress         |
| 4     | Deployment      |
| 5     | Job             |
| 6     | Volume          |
| 7     | ExternalVolume  |
| 8     | Action          |

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

Seedling sets the `localmount` hostname inside each pod container to the node-wide mount
endpoint `fd5e:XXYY:ZZWW:fffe::1`. This address lives in the `fffe` subnet, which is above
the resource-kind range and the proxy discriminant and can never collide with a pod or service
address. Containers route it via their pod bridge gateway; nftables prerouting DNAT intercepts
the packet before any forwarding decision. No address assignment to bridge interfaces is
needed.

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

An Ingress exposes a service to external traffic. All ingress traffic
flows through Caddy, which runs in a container on the dual-stack
`seedling-proxy` network. The proxy network has both an IPv6 /64
(`fd5e:XXYY:ZZWW:ff00::/64`) and a fixed IPv4 /24 (`10.89.255.0/24`).

Caddy uses a custom image (`localhost/seedling-caddy:latest`) built
with the [caddy-l4](https://github.com/mholt/caddy-l4) plugin,
enabling it to proxy both HTTP and raw TCP/UDP streams. The
Containerfile is at `Containerfile.caddy` in the repository root. If
the image is not present locally, seedling builds it automatically
from an embedded copy of the Containerfile on first startup.

Caddy runs in one of two container slots — `seedling-caddy-blue` and
`seedling-caddy-green` — for blue/green upgrades. The active slot is
recorded in the database; the default for fresh installations is blue.
During an image upgrade, the new container is started in the inactive
slot, configured, and health-checked before traffic is switched over.

### nftables ingress rules

Every ingress port gets nftables DNAT rules in both the prerouting and
output chains that redirect traffic to Caddy. For IPv6:

    meta nfproto ipv6 fib daddr type local tcp dport <port>
    dnat ip6 to [<caddy_ipv6>]:<port>

For IPv4 (dual-stack):

    meta nfproto ipv4 fib daddr type local tcp dport <port>
    dnat ip to <caddy_ipv4>:<port>

Both prerouting and output chains carry identical rules. The output
chain rules catch host-originated traffic (e.g., `curl localhost:80`)
via the `fib daddr type local` guard.

The `fib daddr type local` guard is essential: without it, the
prerouting dport rule would catch Caddy's own upstream traffic to
`service_ip:<port>` and loop it back to Caddy.

### Loopback hairpin NAT

When host-originated traffic to `localhost:80` is DNAT'd to Caddy in
the output chain, the source address stays `127.0.0.1` (or `::1`).
Caddy receives the packet and tries to respond to the loopback
address, which routes to its own container loopback — the reply never
leaves the container.

A `postrouting` chain with masquerade rules fixes this:

    meta nfproto ipv4 ip saddr 127.0.0.0/8 masquerade
    meta nfproto ipv6 ip6 saddr ::1 masquerade

MASQUERADE rewrites the source to the bridge gateway IP so the
response goes back through the bridge. Conntrack reverses both the
SNAT and DNAT on the return path, delivering the response to the
original caller with the expected loopback source address.

### HTTP ingress

Ingresses with `.http()` or `.http2()` use Caddy's HTTP reverse proxy.
Caddy matches on the `Host` header and proxies to the service upstream
over IPv6:

    client → host:80
      → nftables DNAT → Caddy
      → Caddy matches Host, reverse-proxies to http://[<service_ip>]:80
      → nftables service DNAT: service_ip:80 → pod_ip:3000
      → pod receives on :3000

Caddy handles TLS termination, ACME certificate management, path-based
routing, and HTTP/HTTPS redirects.

### TCP/UDP ingress (Caddy L4)

Ingresses without HTTP termination use Caddy's L4 plugin. Caddy
listens on the ingress port and proxies raw TCP or UDP streams to the
service upstream:

    client → host:5432
      → nftables DNAT → Caddy
      → Caddy L4 proxies to [<service_ip>]:5432
      → nftables service DNAT: service_ip:5432 → pod_ip:5432
      → pod receives on :5432

The L4 config is generated as `layer4.servers` entries in the Caddy
JSON, separate from the `http.servers` entries used for HTTP ingress.

Because all ingress flows through Caddy, dual-stack works uniformly
for both HTTP and TCP/UDP: IPv4 clients connect to Caddy over IPv4,
and Caddy proxies upstream over IPv6. No IPv4 addresses are needed on
pod networks.

### Caddy admin API

Caddy's configuration is applied via its admin API (`POST /config/`).
The JSON payload always includes `"admin": {"listen": ":2019"}` to
preserve the admin listener across full-config replacements. Caddy
listens on `:2019` on all interfaces inside its container.

## nftables table structure

All rules live in a single `inet` (dual-stack) table:

    table inet seedling_net {
      chain prerouting {
        type nat hook prerouting priority dstnat; policy accept;
        # ingress DNAT (IPv6 + IPv4)
        # mount DNAT
        # service DNAT
      }
      chain output {
        type nat hook output priority dstnat; policy accept;
        # ingress DNAT (same targets as prerouting)
      }
      chain postrouting {
        type nat hook postrouting priority srcnat; policy accept;
        # loopback masquerade (hairpin NAT for localhost access)
      }
      chain forward {
        type filter hook forward priority filter; policy accept;
        # single rule: accept fd5e:ed00::/24 ↔ fd5e:ed00::/24
      }
    }

The entire table is flushed and rebuilt atomically on every
reconciliation tick. The forward chain carries a single blanket accept
rule allowing all forwarded traffic within the seedling ULA range. The
postrouting chain carries masquerade rules for loopback-sourced traffic
(see [Loopback hairpin NAT](#loopback-hairpin-nat) above).

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