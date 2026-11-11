# Gaps from the rewrite

## Open decisions

### PRX / INFRA / DNS / NAT64 / SEC
- PRX: status for a route with no upstreams: name 502/503, or "an error status" (current).
- PRX/DNS: proxy and resolver teardown on an idle host written as SHOULD.
- INFRA: `container_runtime_unavailable` is never cleared in code; spec says cleared once started with both backends reachable.
- INFRA: should WAL/SHM side files fall under the database-file permission rule?
- DNS: resolver healthy before any workload starts: keep MUST (spec leads code) or drop.
- DNS: host resolver `127.0.0.54` as a host requirement or a note.
- NAT64: failed `ipv4only.arpa` lookup and unreadable route/address state become definite answers (violates "could not determine"): keep defaults with a fault, or treat as unknown.
- NAT64: should a failed translator teardown file a fault (today only logs)?
- SEC: missing key file with secret rows present: refuse to start, or keep auto-create (silently orphans values).
- SEC: name the encryption construction (XChaCha20-Poly1305, nonce prefix) or keep "authenticated encryption".

### ACN / RT
- ACN: start action never run automatically in code; keep both triggers (after install, at runtime start), post-install only, or make on_start ordinary.
- ACN: duplicate on_action/on_start/on_shell/on_install names silently replace: throw, or last wins.
- ACN: on_action("install") collides with the install action at dispatch: reserve the name, or route install by flag.
- ACN: scheduled fires pass an empty param map without applying the schema: apply it (fault on required without default), or keep.
- ACN: on_schedule on a handle inside a closure: throw or leave unspecified.
- RT: rt.stop default deadline 30 s vs 90 s default stop timeout: raise, or tie to the container's stop timeout.
- RT: scheduled()/running()/terminated() on a warm Started: throw, or define.
- RT: unobserved exit status counted as success ("could not determine"): keep or count as failure.

## Spec leads or contradicts code

- INFRA: pod network prefixes required allocated and recorded; code derives an 8-bit hash.
- INFRA: database file created then chmod 0600; not created owner-only.
- INFRA/SEC: secret key file written with default mode then chmod 0600, briefly readable by others (security).
- INFRA: proxy and resolver database connections have no busy timeout.
- DNS: resolver brought up in parallel with pods each tick, not before workloads.
- NAT64: startup translator failure exits rather than faulting and holding workloads.
- PRX: proxy image requirements are a CI process; unverified.

- ACN: unknown option keys, unknown install config and param-definition keys, non-map params silently ignored (spec: throw).
- ACN: action sharing a shell's name cannot be invoked; shell sharing an action's name runs the action's closure.
- ACN: shell volume params unchecked/unbound; undeclared shell params dropped without schema; shell.error message not delivered.
- ACN: volume binding failure at dispatch only warns and runs the closure (spec: fails).
- ACN: install params skip the reserved-suffix check.
- ACN: secret params via invoke recorded in clear in SubActionInvoked and the breadcrumb (security).
- ACN: non-finite floats to invoke become null (spec: throw).
- RT: arrays as rt selections act on nothing (demo app's rt.warm_images([...]) warms nothing); other unrecognised values silent no-ops (spec: throw).
- RT: rt.signal delivery failure only logged and recorded as delivered (spec: throw); service/ingress-only selection does not throw.
- RT: rt.exec on a non-running target not checked; output goes to the daemon log, not the container's.
- RT: rt.restart not replay-protected (spec: at most once per call).

## Cross-spec moves

- CONST `l/const.host-has-ipv6` should link to `NAT64#ipv6-egress` instead of restating it.
- APPS `i/generation.history` redaction rule should match SEC (secret now or protected when recorded).

- SCHED `r/schedule.start-reject` duplicates ACN#start-action.
- OPS `r/operation.composition.cycles`, `.params` restate ACN#action-invocation throws; OPS keeps mechanism and links.
- PARAM `l/param.schema.kind` should link ACN#param-kinds (it omits `random`).
- CTR should enumerate accepted signal names (defs/container.rs:718), which RT links to.
- SUSP `r/barrier.condition` duplicates RT#barriers; SUSP links RT for the script-visible rule.

## Root external documents

RFC 9110, 9112, 9113, 9114, 6585, 3986 (PRX); RFC 4193, FIPS 180-4 (INFRA); RFC 1035 (DNS); RFC 6146, 6147, 6052, 7050, 4291 (NAT64).

POSIX crontab (Issue 8), IANA tz database, SipHash, zxcvbn (ACN).

## Root glossary candidates

- Host prefix: the IPv6 /48 unique local prefix within which the runtime assigns every address on a host (INFRA).
- Active NAT64: the runtime providing its own NAT64 translator for its host's workloads (NAT64).
- Action, Action closure (ACN#action-closure; root Lifecycle operation entry uses it).
- Barrier, Runtime handle (RT).
