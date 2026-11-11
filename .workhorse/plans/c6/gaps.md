# Gaps from the rewrite

## Decided

Batches 1 and 2, folded into the specs; code changes filed on D6.

- Could-not-determine cases follow AGENTS.md: NAT64 detection and IPv6 egress undetermined (NAT64 active, forced synthesis, fault, retry), NAT64 teardown fault, unobserved exit status not success, re-check fault after 5 failures, audit log unavailable fault, unreadable operation state holds operations with a fault.
- Effects recorded before they happen: action log begin/complete entries (signal and exec throw on an incomplete entry, write repeats); held volume record committed before the move.
- Malformed requests and actors get `requirements_invalid`; journal open failure `internal`; connected-clients read failure `daemon_unavailable`; STS `null`; forward id, template origin, TLSM preconditions and DNS provider config validated as `requirements_invalid`.
- BSL throws: duplicate action, shell and install declarations; `install` reserved as an action name; `on_schedule` inside a closure; non-ready barriers on warm `Started`; whitespace in `user`; a container with no image fails evaluation.
- `seedling-op-` reserved for external volume mapping names; `_system` is not a valid app name, so needs no reservation.
- Container `env` in `/apps/show` reports values containing a secret parameter's value as `null`; plan diffs carry field names only.
- Database side files created owner-only; a missing key file with protected values present stops startup (no fault: the daemon is not running to serve one).
- Resolver healthy before workloads stays MUST.
- Start action runs after a successful install (trigger `install`) and at runtime start (`boot`); the app's desired state is held to what the start action has started until it ends.
- Scheduled runs validate an empty invocation against the schema; failure files `schedule_params_invalid`.
- `rt.stop` default deadline is the longest stop timeout of the stopped containers plus 30 s. CTR's 90 s default stays SHOULD: it is an intended platform hedge, which systemd's default satisfies on Linux.
- Queued operations are persisted and restored at startup.
- An install that cannot be replayed is recovered as a stuck install.
- A restart replacing a failed run is a recovery restart, whoever performs it.
- `observe_failed` clears on the next successful observation; a fact is recorded again whenever it changes.
- Proxy, resolver and NAT64 translator idle teardown are MUST, as the original specs and the code have it (the rewrite had softened them).
- Deployment scale ranges take Rhai's meaning: `..` exclusive, `..=` inclusive.
- A proxied route with no upstreams answers 503.
- `redirect` with a non-HTTPS termination fails evaluation once the definition has evaluated, whatever the call order.
- Two apps claiming one hostname and port: the app whose install completed first is served, the other gets `ingress_conflict`.
- Mounting two Service Ports with the same port number in one pod throws.
- An unmapped external service has no target, so its traffic is dropped or rejected (confirmed).
- Tailscale certificates are specified in TLS (`#tailscale-certificates`); SITE links there.
- Stored certificates' `key_type` names the key's algorithm; only `csr/begin` is limited to P-256 (the rewrite had widened that restriction).
- `/tls/certificates/attempts/list` limit is 1 to 500; hostname status precedence is `blocked`, `expired`, `error`, `pending`, `active`, `default`.
- The `tailscale` policy kind is removed; `canopy_unavailable` is dropped from the error codes.
- The relay selects its offer once a slot is held; `/canopy/offer` on a connection that cannot relay stays `not_found`.
- CNR keeps `uninstalling` on the wire; an ingress is Ready once the proxy configuration including it is applied.

## Open decisions

### PRX / INFRA / DNS / NAT64 / SEC
- INFRA: `container_runtime_unavailable` is never cleared in code; spec says cleared once started with both backends reachable.
- SEC: name the encryption construction (XChaCha20-Poly1305, nonce prefix) or keep "authenticated encryption".

### ACN / RT

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

## Platform hedges

The generic specs hedge on purpose for platforms other than Linux, and these hedges are not gaps.
The Linux platform spec (`platform/linux.md`, LINUX) resolves each one for Linux:

- CTR default stop timeout (systemd's 90 s default)
- AUTO restart record and pacing (systemd as the supervisor, `Restart=` pacing, exit status from the unit)
- RT unobserved exit status (unit result)
- PRIO OOM protection (OOM score adjustment), resource-control groups (cgroup v2 slices)
- ACTU journal fields (`SEEDLING_*` in the systemd journal)
- DNS host resolver at `127.0.0.54` (systemd-resolved stub)
- CONST `IDLE_CMD` (an image `sleep` accepting `infinity`)
- KEY client key path (`$XDG_STATE_HOME`)

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
- CONST: host facts read once at daemon start; pick up network changes without restart?
- COL: col(external_service) is empty though col(app) includes it: fix code to coerce to itself, or keep.
- COL: anonymous resources of one type compare equal in only/except: own identity, or accept.
- COL: col([a,a]) duplicates; col(app) taken early includes later declarations: state sets resolved on use, or leave unstated.
- PARAM: validators block only resource declaration: forbid all builder methods, or only declarations.
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
Spec leads code:
- JOB: `deadline` never enforced.
- CTR: failed allowlist read treated as empty and files the fault (spec forbids).
- CTR: env array entries with extra keys accepted (spec: throw).
- VOL: readonly() has no effect (always mounted rw).
Moves: PRX rate-limit budget sentence should say a service-declared limit is shared by inheriting routes; ACTU hardening should link CTR for the PID default; 0640 write mode fits ACTU near storage confinement; SVC dead links to pod.bind/container.mount-service -> POD#service-mounts, POD#port-bindings.
Standards: RFC 9110, 8878 (HTTP); RFC 8446, 9147, 9110, 9112, 9113, 9114, 1035, 1123 (ING); OCI image config, capabilities(7), signal(7) (CTR).
Glossary: Ingress (ING); External volume, Declared file write (VOL).

### Runtime C (ACTU IMG SITE PRIO UPD FLT IDN)
Open:
- IMG: rt.query records images during a probe in code; spec says no-op: change code or spec.
- FLT: /faults/clear rejects `_system` (invalid name) though spec lets operators clear `_system`/`seedling` faults: change endpoint, or auto-clear every `_system` kind.
- FLT: reconciler_panic has no clear path (depends on above).
- FLT: install_interrupted, audit_lag clear only by operator: keep, or clear install_interrupted on a later successful install.
Spec leads code:
- IMG: image_pull_failed filed on the first tick the image is missing (spec: after 5 consecutive failures).
- IMG: image GC treats a failed pin lookup as not pinned and removes the image.
- UPD: replace strategy does not wait for every stale instance to stop before starting replacements.
- FLT: container_runtime_unavailable never cleared; volume_create_failed/volume_remove_failed/volume observe_failed no clear path, volume observation not thresholded; cert_issue_failed not cleared when hostname stops needing a cert.
- FLT: image_pull_failed keyed by instance and cleared by description match (spec: by image reference); site_ingress_target_missing keyed by site ingress, matched by description substring (spec: by attachment); app-wide registry_error clear also clears instance-scoped ones.
- FLT: health_check_replace_failed clears on an in-memory generation change (missed across restart); tailscale_unreachable never filed when the socket is unreachable.
- IDN: app name `seedling` not reserved in code though the runtime files faults under it.
Moves: ACTU ingress serving sections (actuate.ingress.plaintext/port-scope/warm-certs) belong in PRX; TLS/TCAN/CNR/FLTS/CTR/APPS/PRM fault restatements should link FLT rows.
Standards: RFC 9000, 9110 (ACTU); RFC 1035, 3596, 6146, 6052, 9110 (SITE); RFC 9562 (IDN).
Glossary: Data directory (ACTU); Site volume, Site ingress (SITE).

### Interface C (TPL TLSM RLY CLI)
Open:
- TPL/APPS: template instantiation whose script fails: code registers + files script_error; app.register says refuse and don't register.
- CLI: templates update --source vs positional source elsewhere: align, or keep.
Spec leads code:
- TLSM: database failures map to not_found (spec: internal).
Moves: EVT lacks Template* events (kept in TPL); hostname pattern / primary SAN belong in TLS runtime; ACME directory (LE production) belongs in r/tls.strategy.acme-dns; hostname syntax could move to OI/WIRE; "failed read is internal, never not_found" belongs in WIRE error codes; ACME account keys never returned is not stated in TLSM.
Standards: RFC 3339 (TPL); RFC 8555, 9773, 2986, 5280, 7468, 5958 (TLSM); RFC 9110, 9112, 9000, 3339 (RLY).
Glossary: Canopy (defined in RLY for now).

### Web (WEB UI)
Open:
- WEB: interface name matching no interface: skip with warning (current spec), or fatal.
- WEB: auth_required "password" even when password login disabled: keep, add a none value, or unspecified.
- WEB: heartbeat returning alive:false after reap: console reconnects, or server re-records.
- UI: dangerous tier for destructive controls: MUST with fault-clearing exception named, or SHOULD.
Spec leads code:
- WEB: HTTP resolving to no address must stop startup (code runs without listener).
- WEB: session path must be exactly /wt (code: prefix match).
- WEB: --wt-port with --wt-listen uses 7893.
- WEB: debug builds trust any daemon without a fingerprint (spec: must not start).
- WEB: connected-clients returns [] on failed shells/forwards read.
Moves: daemon_unavailable/protocol_error could join WIRE error codes; WebSession* events to EVT.
Standards: WebTransport (W3C CR), draft-ietf-webtrans-http3, Secure Contexts, RFC 9110, 9111, 8246, CSP3, RFC 7034, Fetch, HTML, Permissions Policy, RFC 2047, 9106, PHC string format, FIPS 180-4, 186-5, RFC 8259, 3339, TOML 1.0.0, Tailscale Serve headers.
Glossary: Web interface (WEB), Actor (i/wire.actor), Console, Safety mode (UI).

### Runtime A (LIM GEN TRD RCK LCY HIST AUD GC RUN)
Open:
- LIM: --script-max-call-depth 0: help says unlimited, engine treats as no calls: fix help, or make 0 unlimited.
- TRD: /apps/uninstall has no interface spec; app install phases (NotInstalled/Installing/Installed/Uninstalling) have no home spec; SCHED links nonexistent r/lifecycle.app.
- LCY: sole backend in start period receives traffic (code) vs not (old spec).
- HIST: "provenance" clashes with definition provenance: rename "operation provenance", or keep.
- HIST: external volumes get observations never made: carve out declaration-only resources, or stop deriving their state from observations.
- GC: third retirement case (terminal observation present) may be obsolete and may retire a live later cycle: drop, or keep.
Spec leads code:
- GEN: failed decrypt during reconstruction treated as unset (spec: fail).
- TRD: deregistration that cannot discard state only warns (can leak external volume mappings to the next same-named app).
- LCY: service never leaves Ready when its pool empties; unreadable observation history derives Pending and emits a false state-change event.
- HIST: provenance observation ids always empty; should_back_off never called; autonomous operation runs when its log entry write fails.
- AUD: parent dir permissions reset to 0750 on every open.
- GC: unreadable history makes an instance eligible for retirement.
Moves: history.persist.partial-update belongs in INFRA database section (kept in HIST#durability); audit.log.generations field semantics better in EVT; EVT missing template, site/held volume, external mapping, priority, restart, stop/unstop events; FLT should list audit_lag, uninstall_unidentifiable_units; RT rt.lifecyle should link LCY.
Standards: JSON Lines, RFC 8259 (AUD).
Glossary: World observation history, Autonomous operations log, Action execution log, Provenance (HIST); Generation history (GEN); Resource instance (IDN).

### Runtime TLS (TLS CHAIN TCAN CNR)
Open:
Spec leads code:
- TLS: ARI renewal info fetched only at issuance (RFC 9773 §4.3 requires re-fetch).
- TLS: `replaces` names the active certificate of any origin, including non-ACME (RFC 9773 §5); treat as bug.
Moves: TLSM stale claims (manual strategy, directory_url, manual issuance clears retry block, set_by auto) — check TLSM rewrite agrees; APP app.canopy-type link should point at CNR.
Standards: RFC 8555, 9773, draft-ietf-acme-profiles, RFC 9525, 5280, 2986, 7468, 6066 (TLS); RFC 9457, 4648 (TCAN).
Glossary: Managed hostname, Strategy, Policy, Runtime-managed certificate (TLS); Canopy (TCAN/RLY); Report tick (CNR).

### Interface B (ACI SHL FWD LOG FLTS EVT KEY IMGM)
Open:
- SHL: shell volume-kind params validated but never bound: bind (spec says so), or exclude volume kinds from shells.
- IMGM: failed pull error code: not_found (code), internal, or new pull_failed.
- IMGM: pull single attempt vs runtime's retrying pull path: restore retries, or keep.
Spec leads code:
- SHL: volume-shell failures send undefined codes shell_error/internal_error (spec: internal).
- FWD: TCP relays outlive /forwards/stop and teardown; teardown only on /apps/update, not param change or uninstall; stream naming no live forward dropped silently (spec: reset).
- FLTS: settings read failure falls back to 5/1800 and writes them.
- FLTS/KEY/IMGM/TLSM: database failures return not_found instead of internal (pattern across handlers).
- EVT: FaultCleared, ShellStarted/Exited, ForwardStarted/Stopped never carry actor.
- IMGM: proposed_script failure -> not_found (spec: script_error); container list unwrap_or_default makes images look unused (non-forced remove can delete in-use image); in_use likewise; failed allowlist read files disallowed_registry against every app.
Moves: /volumes/* methods have no interface spec (volume reference belongs there); client identity key (i/key.client.*) could move to CLI.
Standards: RFC 9000, 3339 (SHL); RFC 9000, 9221, 6891 (FWD); RFC 3339 (LOG FLTS EVT); RFC 8032, 5958, 8410 (KEY); OCI Distribution, OCI Image (IMGM).
Glossary: Install phase (ACI; also asked by TRD).

### Runtime B (OPS SCHED SUSP AUTO OBS)
Open:
- OPS/TRD/ACI: app phases have no defining rule: which spec owns them.
Spec leads code:
- OPS: operation record not persisted before running (only warns); failed reconstruction of `old` -> empty app; replay ignores target generation; failed persisted-operation read -> none (stuck sweep can tear down a live install); failed cancel-flag read -> not cancelled; generation outcome never recorded (record_outcome test-only), cancelled reported as OperationFailed; EVT trigger list has `boot`, lacks `replay`.
- SCHED: queued fires never stamped, so they double-fire; unparseable last_fired_at -> never fired.
- SUSP: rt.stop deadline start located by resource equality not position; spared-resource listing failure leaks them.
- AUTO: unreadable scale stalls reconciliation (empty app list, waits for outside notify); unreadable restart settings fall back silently.
- OBS: certificate reported valid if the file exists (no expiry check); observation marked recorded before its write.
Moves: rt default deadlines to RT (already done there); OI certificate status vocabulary differs from observed statuses (TLS/TLSM to reconcile).
Glossary: Action closure, Dynamic resource, App phase (OPS); Barrier (SUSP/RT); Effective scale (AUTO).

### Interface A (OI TRN WIRE STS APPS DEFN CTL PRM)
Open:
- APPS: resource `type` externalvolume vs def.kind external_volume vs plan ExternalVolume: unify?
Spec leads code:
- TRN: bootstrap authorized_keys fingerprints imported without normalising (uppercase/prefixed never match).
- WIRE: control request over 64 KiB with no newline loses its actor (affects pushed bundles via web); /forwards/start error responses lack trailing newline; `deregistering` code defined but unused (dropped).
- STS: active_faults falls back to 0, failed inspect read as stopped, app status falls back to degraded (spec: null).
- APPS: /apps/show defaults unreadable state to empty/null and status to Degraded (spec: internal); unknown_params returned decrypted from secret storage (security); next_fire_at null whenever no schedule rows; removal maps db failure to not_found and only logs purge failures; update/param changes read params via a function returning empty on error.
- CTL: database failures reported as script_error; PRM handlers as not_found (spec: internal).
- PRM: plan with no input omits errors (spec: empty array).
Standards: RFC 9000, 9001, 9846 (TLS 1.3, obsoletes 8446), 7250, 7301, 9221, 8259, 3339, 5280, 8410, 4648, 1952, FIPS 180-4, OCI Image, OCI Distribution, SemVer 2.0.0.
Glossary: Fingerprint (TRN).
