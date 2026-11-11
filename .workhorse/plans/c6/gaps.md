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

## Batch reports (appended as they arrive)

### Language A (BSL BND COL CONST APP PARAM SVC)
Open:
- CONST: failed egress probe reports false, undetected time zone UTC ("could not determine"): keep+document, block evaluation, or fault+fallback (rec.).
- CONST: IDLE_CMD needs an image `sleep` accepting `infinity`: state image requirement, or runtime provides idle process.
- CONST: host facts read once at daemon start; pick up network changes without restart?
- COL: col(external_service) is empty though col(app) includes it: fix code to coerce to itself, or keep.
- COL: anonymous resources of one type compare equal in only/except: own identity, or accept.
- COL: col([a,a]) duplicates; col(app) taken early includes later declarations: state sets resolved on use, or leave unstated.
- PARAM: validators block only resource declaration: forbid all builder methods, or only declarations.
- SVC: unmapped external service has no target, so drop-or-reject routing applies: confirm.
Spec leads code:
- PARAM: previous generation unbuildable -> handler runs with empty `old` (spec: MUST NOT run with a substitute).
- PARAM: old.<type>(name) in a change handler returns the current generation's resource (spec: previous).
- PARAM: one-parameter change-handler closure registers but fails at run.
- SVC: service.port on a static service in action context throws (spec: instance method).
- BSL/VOL/SVC: exported(options) skips unknown keys (spec: throw).
- APP: ExternalVolume.description mutates a static external volume in action context.
Moves: DEFN `definition.bundle.limits` -> link BND#metadata-file; `/services/exported/list` OI endpoint has no interface spec; event non-emission of secret params belongs in SEC.
Standards: TOML, Cargo version requirement syntax, SemVer 2.0.0, RFC 6146, 6147, IANA tz.
Glossary: Script (BSL), Action context (APP), Static resource (APP).

### Language B (HTTP ING DEP JOB CTR POD VOL)
Open:
- ING: redirect() then tls() with a non-HTTPS pairing leaves a redirect on non-HTTPS: tls() throws, tls() clears, or check at end of evaluation.
- ING: two apps declaring the same (hostname, port) ungoverned: drop both + fault, first-installed wins + fault, or refuse install.
- DEP: scale(1..4) read as inclusive though Rhai `..` is exclusive; `..=` unsupported: keep+note, accept `..=` too, or only `..=`.
- CTR: no stop timeout leaves systemd's default; spec SHOULD 90 s: pin 90, or drop the number.
- CTR: container with no image: throw at end of evaluation, fault at start, or unspecified.
- CTR: trimming `user` is a coercion: keep as defined coercion, or throw on whitespace.
- POD: two mounted Service Ports with the same port collide at localmount:<port>: second throws, or specify winner.
Spec leads code:
- JOB: `deadline` never enforced.
- CTR: failed allowlist read treated as empty and files the fault (spec forbids).
- CTR: env array entries with extra keys accepted (spec: throw).
- VOL: readonly() has no effect (always mounted rw).
Moves: PRX rate-limit budget sentence should say a service-declared limit is shared by inheriting routes; ACTU hardening should link CTR for the PID default; 0640 write mode fits ACTU near storage confinement; SVC dead links to pod.bind/container.mount-service -> POD#service-mounts, POD#port-bindings.
Standards: RFC 9110, 8878 (HTTP); RFC 8446, 9147, 9110, 9112, 9113, 9114, 1035, 1123 (ING); OCI image config, capabilities(7), signal(7) (CTR).
Glossary: Ingress (ING); External volume, Declared file write (VOL).
