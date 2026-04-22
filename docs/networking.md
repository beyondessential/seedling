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
    fd5e:XXYY:ZZWW:fd00::/64        seedling-resolver network (CoreDNS)
    fd5e:XXYY:ZZWW:fd00::1/128      resolver bridge gateway + in-process DNS forwarder
    fd5e:XXYY:ZZWW:fd00::53/128     CoreDNS container address
    fd5e:XXYY:ZZWW:fffe::1/128      node-wide mount endpoint
    fd5e:XXYY:ZZWW:ff00::/64        seedling-proxy network (Caddy)
    10.89.254.0/24                   seedling-resolver IPv4 subnet (dual-stack)
    10.89.255.0/24                   seedling-proxy IPv4 subnet (dual-stack)

    64:ff9b::/96                     NAT64 well-known prefix (RFC 6052)

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

Three subnet discriminants sit above the `ResourceKind` range in byte 6
and can never collide with a pod or service prefix:

| Byte  | Subnet                              |
|-------|-------------------------------------|
| 0xfd  | `seedling-resolver` (CoreDNS)       |
| 0xfe  | node-wide mount endpoint (`fffe::1`) |
| 0xff  | `seedling-proxy` (Caddy)            |

The NAT64 prefix `64:ff9b::/96` is not derived from the node prefix; it
is the IANA well-known NAT64 prefix (RFC 6052) and is used unchanged so
that any DNS64-aware resolver on the network — including an external
one — produces synthetic AAAAs that match the same translator pool.

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

The `localmount` hostname resolves to the node-wide mount endpoint
`fd5e:XXYY:ZZWW:fffe::1` (injected via `--add-host localmount:<addr>` on
every pod container). No interface is ever assigned that address;
nftables DNAT rules in the prerouting chain intercept the packet before
any routing decision and rewrite it to the backing pod, gated on the
consuming pod's /64 so each mount binding targets only its own
consumer:

    ip6 saddr <pod_prefix>::/64 ip6 daddr fd5e:XXYY:ZZWW:fffe::1 \
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

    meta nfproto ipv4 ip saddr 127.0.0.0/8 ct status & dnat == dnat masquerade
    meta nfproto ipv6 ip6 saddr ::1 ct status & dnat == dnat masquerade

MASQUERADE rewrites the source to the bridge gateway IP so the
response goes back through the bridge. Conntrack reverses both the
SNAT and DNAT on the return path, delivering the response to the
original caller with the expected loopback source address.

The `ct status & dnat == dnat` guard scopes the masquerade to
connections that actually hit a DNAT rule. Without it, the rule also
catches plain loopback traffic like DNS queries to `127.0.0.53`: the
rewritten source (the host's LAN address) is then silently dropped by
systemd-resolved's BPF filter, which only accepts loopback-sourced
queries on the primary stub.

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

## DNS resolver

Every pod runs with `--dns <resolver_addr>` so its `/etc/resolv.conf`
points at a single node-local resolver. The resolver is a CoreDNS
container running on a dedicated dual-stack bridge network
`seedling-resolver` (IPv6 `fd5e:XXYY:ZZWW:fd00::/64` and IPv4
`10.89.254.0/24`). CoreDNS itself listens at the static address
`fd5e:XXYY:ZZWW:fd00::53`; the `::53` host byte is chosen only for
memorability.

The resolver network is dual-stack so CoreDNS can forward queries to
upstream IPv4 DNS servers when the operator supplies any via
`--dns-upstreams`.

Like Caddy, the resolver runs in blue/green container slots
(`seedling-resolver-blue` / `seedling-resolver-green`) for zero-downtime
image upgrades. The active slot is recorded in the database; upgrades
start the inactive slot at the same well-known address, health-check
the `/health` endpoint on port 8080, then swap.

The Corefile is generated from two inputs:

- `forward .` — the list of upstreams. Either the operator's explicit
  `--dns-upstreams` list, or (default) a single entry pointing at
  seedling's in-process DNS forwarder on the resolver bridge gateway
  (`[<prefix>:fd00::1]:53`).
- `dns64 { prefix 64:ff9b::/96 }` — added if and only if NAT64 is
  active (see below), so lookups of IPv4-only names return a
  synthesised AAAA.

### Host DNS forwarder

When no `--dns-upstreams` is given, seedling would ideally point
CoreDNS at `127.0.0.53` to inherit all of systemd-resolved's features
(split DNS, Tailscale MagicDNS, DNSSEC, per-link resolvers, search
domains). Two problems block the direct approach: CoreDNS runs inside
a container and cannot reach the host's loopback, and `127.0.0.53`
only accepts queries sourced from `127.0.0.0/8`.

Seedling bridges these with a small UDP+TCP forwarder process embedded
in the daemon. It binds to `[<prefix>:fd00::1]:53` — the
netavark-assigned gateway address of the resolver bridge — and
proxies every query to `127.0.0.54:53`, systemd-resolved's "extra
stub". The extra stub, unlike `127.0.0.53`, accepts queries whose
source address is not in `127.0.0.0/8`, so the host's LAN-sourced
packets from the bridge are served normally.

`--dns-upstreams` disables the forwarder entirely; CoreDNS then talks
directly to the operator-supplied servers.

## NAT64 egress

Pod networks carry only IPv6 ULA addresses — no IPv4 and no GUA — so
reaching any IPv4-only destination on the internet requires NAT64. On
hosts where the upstream network does not already provide it, seedling
stands up its own translator.

### Mode selection and detection

The `--nat64` flag takes one of three values:

- `auto` (default) — seedling probes for existing NAT64 infrastructure
  at startup by resolving `ipv4only.arpa` for AAAA (RFC 7050). If a
  synthetic AAAA comes back (i.e. an address outside the canonical
  `192.0.0.170` / `192.0.0.171`), the network already has a NAT64+DNS64
  setup and seedling leaves it alone. Otherwise seedling activates its
  own.
- `enabled` — always activate.
- `disabled` — never activate.

Detection is performed once, before the reconciliation loop starts.

### Translator

When NAT64 is active, seedling installs and runs a stateful translator
using [Jool](https://nicmx.github.io/Jool/):

1. `modprobe jool` loads the kernel module (the package must be
   installed on the host).
2. `jool instance add seedling --netfilter --pool6 64:ff9b::/96`
   creates a translator instance that hooks netfilter directly.
3. `net.ipv6.conf.all.forwarding` and `net.ipv4.conf.all.forwarding`
   are both set to `1`.

Because the instance runs in Jool's netfilter mode, no explicit `ip -6
route` for `64:ff9b::/96` is needed: Jool installs a netfilter hook
that catches matching packets before they reach the routing decision.
No pool4 is configured, so Jool masquerades translated IPv4 traffic
using the host's primary IPv4 address.

Translator setup happens at daemon startup, before any pods are
touched. If NAT64 is required but initialisation fails, the daemon
exits and files a fault — pods must not come up on a node that cannot
reach the IPv4 internet.

### DNS64

When NAT64 is active, the CoreDNS Corefile includes the `dns64` plugin
with the matching `64:ff9b::/96` prefix. For names that have only A
records (no AAAA), CoreDNS synthesises an AAAA record pointing into
the NAT64 prefix.

### End-to-end egress flow

For a pod reaching an IPv4-only host `ipv4only.example.com`:

1. Pod's libc queries `<prefix>:fd00::53` (CoreDNS) for AAAA.
2. Upstream returns NXDOMAIN / empty AAAA; the `dns64` plugin
   synthesises `64:ff9b::<a.b.c.d>`.
3. Pod sends a TCP/UDP packet to `[64:ff9b::<a.b.c.d>]:<port>`.
   Source is the pod's SLAAC ULA; default route sends the packet via
   the pod bridge gateway.
4. On the host, before any forwarding decision, Jool's netfilter hook
   matches the destination against pool6, translates to IPv4,
   rewrites the source to the host's IPv4 address, and creates a
   conntrack entry.
5. The IPv4 packet egresses via the host's normal IPv4 uplink.
6. Replies arrive as IPv4, Jool's conntrack reverses the translation,
   and the resulting IPv6 packet is routed back to the pod via its
   bridge.

For a destination that already has a real AAAA record, DNS64 does not
synthesise and the pod reaches it directly over IPv6 through whatever
native IPv6 connectivity the host has.

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
        # ct state established,related accept
        # ct status dnat accept
        # accept saddr <node/48> daddr <node/48>
        # drop unsolicited new inbound to <node/48>
      }
    }

The entire table is flushed and rebuilt atomically on every
reconciliation tick. The forward chain accepts established/related and
DNAT-redirected traffic, accepts intra-node traffic sourced and
destined within the node's `/48`, and drops unsolicited new inbound
flows directed into that `/48`. The postrouting chain carries
masquerade rules for loopback-sourced traffic (see [Loopback hairpin
NAT](#loopback-hairpin-nat) above).

Jool's netfilter hook for NAT64 translation is installed by the jool
kernel module and lives outside this table. A pod packet destined for
`64:ff9b::/96` is caught by the Jool hook before any routing decision,
translated to IPv4, and then takes the host's normal IPv4 output path
— not the `inet seedling_net` forward chain above.

## Host service interactions

### avahi-daemon

By default `avahi-daemon` binds to every interface on the host, including
the transient Podman bridges that seedling creates for each pod. When a
pod bridge comes up or goes down, avahi tries to discover clients on the
new network and logs noisy warnings about "Got SIGTERM, quitting" /
"interface going down" churn, and it can also leak mDNS responses onto
networks that have no business participating in service discovery.

The fix is to constrain avahi to the host's uplink interface(s) in
`/etc/avahi/avahi-daemon.conf`:

    [server]
    allow-interfaces=eth0

(`deny-interfaces=` also works, but seedling bridge names are picked by
netavark at creation time — typically `podman0`, `podman1`, ... — and
`avahi-daemon.conf` supports neither wildcards nor drop-in snippets, so
an allow-list scoped to the real uplink is much easier to maintain than
a deny-list that grows with every pod.)

Reload avahi after editing: `systemctl reload avahi-daemon`.

## Reconciliation order

### Daemon startup (once, before the loop)

1. Derive the node `/48` prefix from `/etc/machine-id`.
2. Decide NAT64 activation (`--nat64` mode + RFC 7050 probe in `auto`).
3. If NAT64 is to be active, load the `jool` kernel module, create the
   jool instance, and enable IPv4+IPv6 forwarding. Failure here is
   fatal.
4. Start the in-process DNS forwarder (unless `--dns-upstreams` is
   set). It retry-binds until the resolver bridge comes up on the
   first tick.
5. Build system-driver handles (container runtime, process manager,
   data plane) and spawn the reconciliation loop.

### Per-tick phases

Within each reconciliation tick:

1. **Observe + actuate (concurrent)** — `tokio::join!` runs four
   phases side by side:
   - **Pods** — observe and actuate containers for each app; collect
     running pod IPs and update the bridge-name map.
   - **Volumes** — observe and actuate volume resources.
   - **Caddy** — ensure the Caddy container is running and healthy.
   - **Resolver** — ensure the CoreDNS container is running and
     healthy; regenerate the Corefile if upstreams or NAT64 status
     changed; blue/green upgrade on image change.
   If Caddy fails, the nftables and proxy-config apply steps are
   skipped for this tick. Resolver failure is recorded as a fault but
   does not block the rest of the tick — workloads keep whatever
   `/etc/resolv.conf` they already have.
2. **Uninstall** (sequential, needs pod results) — for apps being
   uninstalled, check whether all pods and systemd units are gone.
3. **Compute** (sync, in-memory) — build service routes, nftables
   rules, and the Caddy JSON config from the observed state.
4. **Apply (concurrent)** — another `tokio::join!` writes the three
   data-plane surfaces in parallel:
   - `ip -6 route replace` for every service;
   - the single-table atomic nftables reload (ingress / mount /
     service DNAT rules, plus loopback-masquerade and forward-chain
     policy);
   - the Caddy admin API `POST /config/`.
   Each write's success or failure is recorded as a system fault.
5. **Emit + retire** — surface state-change events and retire any
   unscheduled excess instances.

### Idle teardown

When no apps remain registered, the reconciler flushes routes and
nftables rules, tears down the Caddy and resolver containers, and
removes their networks. The jool NAT64 instance is not torn down in
the current daemon shutdown path.