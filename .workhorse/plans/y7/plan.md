# Restrict pod-to-pod traffic to mounts

## Technical notes

### Isolation rules outside the absolute-state apply

- The pod source rule, the forwarding policy and the host input rule live apart from the translation rules, in a table the absolute-state apply never flushes. The runtime installs them at startup, before reconciliation, and doesn't remove them on shutdown or in `clear_all` while any pod network container runs.
- None of them depends on any app's contribution, so none may be built from the set of pod networks known at the last apply. A network created since then (a new pod instance, or a shell session that never goes through reconciliation) must be covered from its first container.
- Open: how to match "arriving from a pod network" without listing bridges. Netavark creates a bridge when the first container on its network starts, so the runtime can't mark it after `create_network`. Options to settle in tech design:
  - Pre-create each bridge with an interface group, if netavark adopts an existing bridge of the configured name.
  - A strict reverse-path check (`fib saddr . iif oif missing drop`) on every interface except the host's uplinks, which still means telling uplinks apart.
- Confirm how workload containers come back after a reboot. If anything restarts them before the runtime starts, the isolation table must be in place first.

### Pod network sources

- The check runs ahead of every translation: before the dstnat prerouting chain, and before Jool's netfilter hook (registered after the dstnat priority). A chain at raw priority does both.
- It drops every IPv4 packet from a pod network, and every IPv6 packet whose source is outside the network's /64, except `::` and link-local sources.

### Forwarding and translation

- The forward chain keeps accepting every translated connection (`ct status dnat`). Once service translation is limited to the proxy network and the host, every DNAT the runtime performs is legitimate: ingress redirection, mounts, and service translation for the proxy or the host.
- "Arriving from the proxy network" is matched by the proxy bridge's input interface, so a pod can't qualify by forging a proxy address. The proxy bridge is fixed and the runtime creates it, so it can be named.
- Service translation in prerouting gains the same proxy bridge input-interface match. The output-chain copy (host-originated) stays as it is.

### Shell sessions

- Mount rules are built only from running app pods (`build_mount_rules`). A shell session's job needs mount translations for its own prefix in place before its container starts, and removed when the session ends. Either the session registers with the reconciler and waits for an apply that includes it, or session mount rules get a set of their own that the session writes directly. The first keeps one writer of the translation rules; the second avoids waiting on a withheld apply.

### Pod traffic to the host

- A filter chain on the input hook. Accept established/related, `ct status dnat` (from a pod, only a mount translation reaches the host), and ICMPv6 neighbour discovery (router solicitation, neighbour solicitation and advertisement) first, then drop the rest from pod networks, matched the same way as the source rule. Loopback is never a pod network: the host holds every bridge gateway, so its traffic to itself carries host-prefix sources.
- NAT64: verify which hooks Jool's translated IPv4 packets traverse (whether they re-enter prerouting or take the local output path) before writing the rules. Translated traffic whose destination is local, or within `10.89.255.0/24` or `10.89.254.0/24`, is redirected to the proxy's IPv4 address at ingress ports and dropped otherwise, except mount-translated and established connections. Telling it apart from the host's own IPv4 traffic, which shares the pool4 source address, is part of that verification.
