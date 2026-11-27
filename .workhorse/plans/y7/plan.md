# Restrict pod-to-pod traffic to mounts

## Technical notes

### Tables

- Three nftables tables: the translation table (absolute state, flushed and rebuilt on each apply), the isolation table (static, below), and the sessions table ([Shell sessions](#shell-sessions)). Each has one writer, and no apply touches another's table.
- The isolation rules (pod network sources, forwarding policy, pod traffic to the host, the NAT64 marks and drops) live in a table of their own, separate from the translation table the absolute-state apply flushes and rebuilds. Every isolation rule depends only on the host prefix, fixed bridge names and the NAT64 prefix, never on any app's contribution, so the table is static: built once from the host prefix, and re-applied wholesale and idempotently.
- The runtime installs the isolation table at startup, before the first reconcile. It never removes it, on shutdown or in `clear_all`, because workload containers outlive the runtime. Workload units are transient systemd units that don't survive a reboot, so after a reboot nothing starts a workload before the runtime has installed the table.
- The forward chain moves into the isolation table. The translation table keeps only nat-type chains (prerouting, output, postrouting masquerade).

### Recognising pod bridges

- The runtime names each pod network's bridge when it creates the network, passing `network_interface` to Podman: `sdp-` followed by the four hex digits of the allocated /64's subnet ID. The name is unique because the prefix is allocated and recorded, and it fits the 15-character limit. Netavark reuses the configured name every time it recreates the bridge (it deletes a managed bridge once its last container leaves), so the name holds for the network's whole life.
- Rules match pod bridges with `iifname "sdp-*"` / `oifname "sdp-*"`. A bridge exists only once netavark creates it for the network's first container, and the rule already covers it then, with nothing to update.
- The proxy network gets a fixed bridge name (`sdl-proxy`), and the resolver network one too (`sdl-resolver`), so the forwarding policy can name the proxy bridge.
- The wildcard is a name-pattern match. Justify it in the relevant `etc/ci/` allowlist: it's a packet filter on a reserved interface-name prefix, and a false match filters more traffic, never less. Add the prefix to `crates/core/src/reserved.rs` and to `docs/networking.md`, so operators know interface names starting `sdp-` are taken.
- Existing networks keep their Podman-assigned bridge names until recreated. Seedling is unshipped, so there's no migration: tests and dev hosts recreate their networks.
- Explicitly rejected: pre-creating bridges with an interface group (netavark skips the gateway address, sysctls and link-up for a bridge that already exists, and deletes it when empty). Also rejected: a strict reverse-path check on every interface with no names (it breaks asymmetric multi-uplink routing).

### Pod network sources

- A filter chain at prerouting, raw priority (-300), ahead of the dstnat chain (-100) and of Jool's IPv6 hook (`dstnat + 25`, so -75).
- `ip6 saddr <host prefix> fib saddr . iif oif missing drop` on every interface except `lo`. It puts each internal /64 on the one interface its route points at. A pod can't use another pod's address, nor the proxy's or the resolver's. It also drops host-prefix sources arriving from an uplink.
- `iifname "sdp-*"`: drop IPv4 outright. Drop IPv6 whose source is outside the host prefix, except `::` and `fe80::/10`. Together with the reverse-path rule, a pod bridge only passes sources within its own /64.

### Forwarding policy

- Accept in order: established/related; `ct status dnat`; `iifname "sdl-proxy"`; TCP/UDP to the resolver address at port 53 with a host-prefix source. Then drop the rest addressed to the host prefix. Traffic addressed outside the host prefix passes, having already met the source rule.
- `ct status dnat` stays a blanket accept. Once service translation is limited to the proxy network and the host, every DNAT the runtime performs is legitimate: ingress redirection, mounts, and service translation for the proxy or the host.
- Service translation in prerouting gains `iifname "sdl-proxy"`. The output-chain copy (host-originated) stays as it is.

### Pod traffic to the host

- An input-hook filter chain in the isolation table, for `iifname "sdp-*"`. Accept established/related, `ct status dnat` (from a pod, only mount translation rewrites a destination to the host), and ICMPv6 router solicitation, neighbour solicitation and neighbour advertisement. Drop the rest.
- The input hook only sees locally addressed packets, so "the host's own addresses, multicast included" needs no address list: the kernel's routing decision supplies it as the packet arrives.

### NAT64 traffic to the host

How Jool runs, read from its source (v4.1.15, netfilter mode, empty pool4):

- It translates at IPv6 prerouting, priority `dstnat + 25`, after our mount translation, and sends the IPv4 result with `dst_output`. The IPv4 packet never passes prerouting, forward or output. Locally addressed, it meets only the input hook, on `lo`, with no conntrack entry. Routed out, it meets only postrouting.
- With an empty pool4, the pool is the host's own universe-scope IPv4 addresses, ports 61001 to 65535. A packet translated to one of those addresses at a port in that range is hairpinned inside Jool into another session's mapping, and sent out as IPv6 with `dst_output` (IPv6 postrouting only).
- Jool copies the IPv6 packet's mark into the IPv4 route lookup and onto the translated packet, including through a hairpin.

The design:

- **Marks.** In the isolation table's prerouting filter chain, after dstnat and before Jool (priority -90): on `iifname "sdp-*"` packets to `64:ff9b::/96`, set a NAT64-from-pod mark bit. Where `ct status dnat` holds, set a mount bit as well. Both bits are ORed into `meta mark` under a mask, leaving other bits alone. Pick bits unlikely to clash with VPN fwmarks, and document them in `docs/networking.md`.
- **Exact drops, no address list.**
  - IPv4 input: drop packets carrying the pod bit without the mount bit. This catches every local delivery of a pod's NAT64 traffic, whatever addresses the host holds at that moment.
  - IPv4 postrouting, `oifname` the proxy or resolver bridge: drop the same packets.
  - IPv6 postrouting, `oifname "sdp-*"`: drop packets carrying the pod bit. Only a hairpin can bring a pod's own outgoing mark back out towards a pod.
- **Redirect.** The runtime keeps a named set in the translation table of the host's IPv4 addresses in `64:ff9b::` form, plus the proxy and resolver IPv4 subnets. Every apply flushes the translation table, so each apply writes the addresses the host holds at that moment into the set's definition, and rtnetlink IPv4 address events add or remove elements between applies. Both go through one owner, so an event can't land in a table that's about to be replaced and get lost. The ingress-redirection rules gain a copy for pod traffic to `@host_v4_nat64` at an ingress port, rewriting it to the proxy's IPv6 address. That happens in dstnat, before Jool sees it, so the redirected packet never reaches NAT64. While the set lags an address change, traffic to the new address isn't redirected and falls to the exact drops: it fails closed.
- Explicitly rejected: Jool address-dependent filtering against hairpins (it costs pods endpoint-independent mappings), and dropping rather than redirecting at ingress ports (it contradicts the spec).

### Shell sessions

- Mount rules are built only from running app pods (`build_mount_rules`). A shell session's job needs mount translations for its own prefix in place before its container starts, and removed when the session ends.
- Session mount rules go in a third table, `seedling_sessions`, with one nat prerouting chain, owned by the shell session code. A session builds its rules with the same function that builds a pod's mount rules, then adds them before its container starts, after its network exists. It deletes them by handle when the session ends, on every exit path that also removes the session's network.
- Sessions don't survive a runtime restart (stray shell containers are removed), so the runtime deletes and recreates the whole `seedling_sessions` table at startup.
- A session never waits on reconciliation, so a withheld apply can't block an operator's shell. A failure to add the rules ends session startup the way a network creation failure does: the session's network is removed and the session reports an error.
- Explicitly rejected: having sessions register with the reconciler and wait for an apply that includes their mounts. A withheld apply would block every shell with mounts.
