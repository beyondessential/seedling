# Restrict pod-to-pod traffic to mounts

## Technical notes

### Tables

- Three nftables tables: the translation table (absolute state, flushed and rebuilt on each apply), the isolation table (static, below), and the sessions table ([Shell sessions](#shell-sessions)). Each has one writer, and no apply touches another's table.
- The isolation rules (pod network sources, forwarding policy, pod traffic to the host, the NAT64 marks and drops) live in a table of their own, separate from the translation table the absolute-state apply flushes and rebuilds. Every isolation rule depends only on the host prefix, fixed bridge names and the NAT64 prefix, never on any app's contribution, so the table is static: built once from the host prefix, and re-applied wholesale and idempotently.
- The runtime installs the isolation table at startup, before the first reconcile. It never removes it, on shutdown or in `clear_all`, because workload containers outlive the runtime. Workload units are transient systemd units that don't survive a reboot, so after a reboot nothing starts a workload before the runtime has installed the table.
- If the isolation table can't be installed, the runtime starts no workload container and no shell session, the way it holds back while a backend is unavailable. It files `packet_filter_failed` under its own subject (`isolation`, beside the existing `apply_rules`) and retries each reconcile iteration with increasing back-off up to a cap, keeping going at the cap. It clears that subject only once the table is installed. Containers already running from before a restart keep the table that survived the restart.
- The forward chain moves into the isolation table. The translation table keeps only nat-type chains (prerouting, output, postrouting masquerade).

### Recognising pod bridges

- The runtime names each pod network's bridge when it creates the network, passing `network_interface` to Podman: `sdp-` followed by the four hex digits of the allocated /64's subnet ID. The name is unique because the prefix is allocated and recorded, and it fits the 15-character limit. Netavark reuses the configured name every time it recreates the bridge (it deletes a managed bridge once its last container leaves), so the name holds for the network's whole life.
- Rules match pod bridges with `iifname "sdp-*"` / `oifname "sdp-*"`. A bridge exists only once netavark creates it for the network's first container, and the rule already covers it then, with nothing to update.
- The proxy network gets a fixed bridge name (`sdl-proxy`), and the resolver network one too (`sdl-resolver`), so the forwarding policy can name the proxy bridge.
- The wildcard is a name-pattern match, so a false match filters more traffic, never less. No `etc/ci/` grep covers nftables interface patterns. Nothing in Seedling creates interfaces for operators, so `reserved.rs` has nothing to reject. `docs/networking.md` records that interface names starting `sdp-` and `sdl-` are taken.
- A network created before this change has a Podman-assigned bridge name. At startup the runtime removes any pod or infrastructure network whose bridge name isn't the one it would choose, with that network's containers, and reconciliation recreates them. Without this, the proxy exception wouldn't match an old proxy bridge.
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

- **Marks.** In the isolation table's prerouting filter chain, after dstnat and before Jool (priority -90): on `iifname "sdp-*"` packets to `64:ff9b::/96`, set a NAT64-from-pod mark bit. Where `ct status dnat` holds, set a mount bit as well. Both bits are ORed into `meta mark` under a mask, leaving other bits alone. Pick bits outside the masks common host tools use for policy routing (Tailscale uses `0xff0000`), and document them in `docs/networking.md`. When the network provides NAT64 rather than Jool, the marked packets are plain forwarded IPv6, so the marks reach the host's policy routing with no NAT64 to read them: they must not change how those packets are routed.
- **Exact drops, no address list.**
  - IPv4 input: drop packets carrying the pod bit without the mount bit. This catches every local delivery of a pod's NAT64 traffic, whatever addresses the host holds at that moment.
  - IPv4 postrouting, `oifname` the proxy or resolver bridge: drop the same packets.
  - IPv6 postrouting, `oifname "sdp-*"`: drop packets carrying the pod bit. Only a hairpin can bring a pod's own outgoing mark back out towards a pod.
- **Redirect.** The runtime keeps a named set in the translation table of the host's IPv4 addresses in `64:ff9b::` form, plus the proxy and resolver IPv4 subnets. Every apply flushes the translation table, so each apply writes the addresses the host holds at that moment into the set's definition, and rtnetlink IPv4 address events add or remove elements between applies. Both go through one owner, so an event can't land in a table that's about to be replaced and get lost. The ingress-redirection rules gain a copy for pod traffic to `@host_v4_nat64` at an ingress port, rewriting it to the proxy's IPv6 address. That happens in dstnat, before Jool sees it, so the redirected packet never reaches NAT64. While the set lags an address change, traffic to the new address isn't redirected and falls to the exact drops: it fails closed.
- IPv6-only hosts: the network usually provides NAT64, so Jool isn't active and a pod's NAT64 traffic leaves as forwarded IPv6, with no host IPv4 address to protect. Where Jool is active without IPv4 egress, the only IPv4 routes are the proxy and resolver bridges and the host's gateways on them, which the mark drops already cover.
- Explicitly rejected: Jool address-dependent filtering against hairpins (it costs pods endpoint-independent mappings), and dropping rather than redirecting at ingress ports (it contradicts the spec).

### Shell sessions

- Mount rules are built only from running app pods (`build_mount_rules`). A shell session's job needs mount translations for its own prefix in place before its container starts, and removed when the session ends.
- Session mount rules go in a third table, `seedling_sessions`, with one nat prerouting chain, owned by the shell session code. A session builds its rules with the same function that builds a pod's mount rules, then adds them before its container starts, after its network exists. It deletes them by handle when the session ends, on every exit path that also removes the session's network.
- A session's rules follow the mounted services' backends, as a pod's do. The reconciler keeps the backend view of its last successful apply. A session builds its rules from that view when it starts, and after each successful apply the data plane rewrites every registered session's rules from the new view. While applies are withheld, session rules keep the last applied backends, as pod mount rules do.
- The data plane is the single writer of the sessions table. Registration, deregistration and the post-apply rewrite all go through it, serialised with its other applies.
- Sessions don't survive a runtime restart (stray shell containers are removed), so the runtime deletes and recreates the whole `seedling_sessions` table at startup.
- A session never waits on reconciliation, so a withheld apply can't block an operator's shell. A failure to add the rules ends session startup the way a network creation failure does: the session's network is removed and the session reports an error.
- Explicitly rejected: having sessions register with the reconciler and wait for an apply that includes their mounts. A withheld apply would block every shell with mounts.

## Checklist

### Specs

- [x] Host infrastructure: pod network sources, forwarding policy, service translation, pod traffic to the host, shell session mounts
- [x] Proxy: proxy addresses
- [x] Service: routing limited to mounts, ingresses and the host

### Bridge names

- [x] `crates/core/src/system.rs`: `pod_bridge_name(prefix)` (`sdp-` plus the /64's subnet ID as four hex digits), and constants for `sdl-proxy`, `sdl-resolver` and the `sdp-` prefix. Unit tests for the derivation.
- [x] `ContainerRuntime::create_network` takes the bridge name. `crates/linux/src/podman.rs` sends it as `network_interface`. Update `stub.rs`, `unavailable.rs` and `backends.rs`.
- [x] Pass the name from every caller: `actuator/pod.rs` (`ensure_network`), `oi/shells/session.rs`, `oi/shells/volume_session.rs`, `caddy/startup.rs`, `resolver/startup.rs`, and the test call sites.
- [x] `startup_cleanup.rs`: remove a pod or infrastructure network whose recorded bridge name differs from the chosen one, with its containers, before reconciliation recreates them. An unreadable listing removes nothing. Test with the stub runtime.

### Isolation table

- [x] New module `crates/linux/src/data_plane/isolation.rs`: table `seedling_isolation`, built from the host prefix alone, replaced wholesale in one batch (add table, flush table, add chains and rules), so the rules never lapse during a replace.
- [x] Prerouting filter chain at raw priority: the reverse-path rule for host-prefix sources on every interface but `lo`; on `sdp-*`, drop IPv4 and drop IPv6 sources outside the host prefix, except `::` and `fe80::/10`. Cite `INFRA#pod-network-sources`.
- [x] Prerouting filter chain at priority -90: OR the pod NAT64 mark bit into pod packets to `64:ff9b::/96`, plus the mount bit where `ct status dnat` holds. Mark constants and mask in one place.
- [x] Forward filter chain: established/related, `ct status dnat`, `iifname "sdl-proxy"`, and TCP/UDP from the host prefix to the resolver address at port 53 are accepted; the rest to the host prefix is dropped. Cite `INFRA#forwarding-policy`.
- [x] Input filter chain: on `sdp-*`, accept established/related, `ct status dnat` and ICMPv6 types 133, 135 and 136, and drop the rest. For IPv4, drop packets with the pod bit but no mount bit. Cite `INFRA#pod-traffic-to-the-host`.
- [x] Postrouting filter chain: IPv4 to `sdl-proxy` or `sdl-resolver` with the pod bit but no mount bit is dropped; IPv6 to `sdp-*` with the pod bit is dropped (hairpins).
- [x] `DataPlane` gains `ensure_isolation()`. Linux and stub implementations.
- [x] Startup: install the isolation table before startup cleanup and before any actuation or shell session can start. On failure, gate actuation and session start, file `packet_filter_failed` with subject `isolation`, and retry with back-off from the reconcile loop.
- [x] Unit tests in `crates/linux/src/data_plane/tests.rs` for each chain's statements, and a core test that a failed install withholds actuation and files the fault, then clears it on success.
- Built with chains `pod_sources`, `nat64_marks`, `forwarding`, `to_host` and `nat64_drops` (nft's parser rejects `mark` and `input` as chain names). Retry pacing copies the backends' (5s doubling to a 300s cap, held there) rather than `RetryGate`, which cycles back down. Faults spec amended so `packet_filter_failed` carries the subject `isolation` for this table.

### Translation table

- [x] `crates/linux/src/data_plane.rs` / `nft.rs`: drop the forward chain and its rule builders (`ct_state_established_related_accept`, `ct_status_dnat_accept`, `seedling_forward_stmts`, `drop_unsolicited_inbound_stmts`) from the translation table; they move to the isolation table.
- [x] `service_dnat_rule_stmts`: separate builders for the prerouting copy (gains `iifname "sdl-proxy"`) and the output copy (unchanged). Cite `INFRA#service-translation`.
- [x] IPv4 ingress redirection becomes unconditional. The proxy's IPv4 address is required: it comes from the proxy network's own IPv4 subnet, which exists whatever the host's uplink, IPv6-only hosts included. The readiness wait in `caddy/startup.rs` waits for both addresses, `EnsuredCaddy` carries both, and a proxy still missing one at the deadline fails startup with `caddy_failed`. `caddy_v4` stops being an `Option` through `phases.rs`, `rules.rs` and `types.rs`.
- [x] Named set `host_v4_nat64` in the translation table: the host's IPv4 addresses in `64:ff9b::` form, plus the proxy and resolver IPv4 subnets. Each apply writes the current addresses, read through rtnetlink.
- [x] Ingress redirection gains, per ingress port and protocol, a rule for traffic from `sdp-*` to `@host_v4_nat64` at that port, rewriting it to the proxy's IPv6 address.
- [x] `NftablesDataPlane` subscribes to rtnetlink IPv4 address events and adds or removes set elements. The listener and `apply_rules` share one lock, so an event can't land in a table that's about to be replaced. The listener loop backs off and resubscribes on error, and records the failure in the same fault.
- [x] Unit tests for the proxy-only service translation, the NAT64 ingress rules and the set contents.
- The address listener reports through `DataPlane::host_addresses_failure()`, which the reconciler files as `packet_filter_failed` with subject `host_addresses` (faults spec amended). Addresses are tracked per interface; addresses inside the proxy and resolver subnets are left to their `/120`. Each apply flushes the set itself, since `flush table` keeps set elements.

### Shell session mounts

- [ ] `crates/core/src/system/reconcile/rules.rs`: split `build_mount_rules` so one pod's mount rules can be built from a stored backend view. The reconciler keeps the backend view of its last successful apply where sessions can read it.
- [ ] `DataPlane` gains `reset_sessions()`, `set_session_mounts(session, rules)` and `remove_session_mounts(session)`. The Linux implementation keeps table `seedling_sessions` (one nat prerouting chain) and rewrites it wholesale from its registry on each change. Stub implementation records the calls.
- [ ] Startup calls `reset_sessions()` alongside the isolation install.
- [ ] `oi/shells/session.rs`: after creating the network, build and register the session's mount rules before creating its container. Deregister on every path that removes the session network. A registration failure ends startup like a network failure.
- [ ] After each successful rules apply, the reconciler rewrites every registered session's rules from the new backend view.
- [ ] Tests with the stub runtime: a session with mounts registers rules for its prefix before its container starts and deregisters on each exit path; a backend change reaches a live session's rules.

### Docs

- [ ] `docs/networking.md`: the three tables and their owners, bridge naming and the taken prefixes, the isolation chains, NAT64 marks (bits and mask) and the Jool behaviour they rely on, and the forward chain section replaced.
- [ ] Run `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`.
