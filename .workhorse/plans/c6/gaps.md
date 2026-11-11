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

### Runtime C (ACTU IMG SITE PRIO UPD FLT IDN)
Open:
- ACTU: journal field names (SEEDLING_*) as contract (kept) or implementation detail; "system journal" vs "host log".
- ACTU: hold renames data before writing its record; record write failure leaves data moved but invisible: needs a rule.
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
- TPL: /templates/update with origin but no definition silently drops origin: refuse with requirements_invalid (rec.), or ignore.
- TPL/APPS: template instantiation whose script fails: code registers + files script_error; app.register says refuse and don't register.
- TLSM: issue-acme-dns precondition failures reported as internal: keep, or requirements_invalid.
- TLSM: dns-providers/upsert doesn't validate config against provider kind.
- TLSM: attempts/list limit uncapped, negative = unbounded: cap (200?), or refuse non-positive.
- TLSM: manual upload always reports key_type ecdsa_p256: restrict uploads to P-256, or detect.
- TLSM: dead `tailscale` policy strategy rendered but forbidden by schema: remove, or allow.
- TLSM: hostname view status precedence when several conditions hold: specify, or meanings only.
- RLY: canopy_unavailable listed in WIRE but never returned: drop, or use.
- RLY: offer chosen before waiting for a slot (may go to an ended offer): re-select, or accept.
- RLY: /canopy/offer outside a relay-capable connection refused as unknown method (not_found): dedicated code?
- CLI: templates update --source vs positional source elsewhere: align, or keep.
Spec leads code:
- TLSM: database failures map to not_found (spec: internal).
Moves: EVT lacks Template* events (kept in TPL); hostname pattern / primary SAN belong in TLS runtime; ACME directory (LE production) belongs in r/tls.strategy.acme-dns; hostname syntax could move to OI/WIRE; "failed read is internal, never not_found" belongs in WIRE error codes; ACME account keys never returned is not stated in TLSM.
Standards: RFC 3339 (TPL); RFC 8555, 9773, 2986, 5280, 7468, 5958 (TLSM); RFC 9110, 9112, 9000, 3339 (RLY).
Glossary: Canopy (defined in RLY for now).

### Web (WEB UI)
Open:
- WEB: interface name matching no interface: skip with warning (current spec), or fatal.
- WEB: /connected-clients/list when shells/forwards can't be read: fail with daemon_unavailable, or null member.
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
- RCK: persistently failing re-checks only log: add a fault after N, or accept.
- LCY: sole backend in start period receives traffic (code) vs not (old spec).
- LCY: ingress Ready requires a valid certificate (old spec) or just proxy config applied (code; spec follows code).
- HIST: "provenance" clashes with definition provenance: rename "operation provenance", or keep.
- HIST: external volumes get observations never made: carve out declaration-only resources, or stop deriving their state from observations.
- AUD: failure to open the log at startup disables auditing for the run: retry+fault, or fatal.
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
- TLS: Canopy report fault under `seedling`, cert_issue_failed under `_system`, neither reserved: reserve both, or a non-app fault scope. (Joins FLT/IDN items.)
- TLS: tailscale policy: specify in TLS, or leave in SITE.
- CNR: report status `uninstalling` vs OI `Deregistering`: keep (on the wire), or rename.
Spec leads code:
- TLS: ARI renewal info fetched only at issuance (RFC 9773 §4.3 requires re-fetch).
- TLS: `replaces` names the active certificate of any origin, including non-ACME (RFC 9773 §5); treat as bug.
Moves: TLSM stale claims (manual strategy, directory_url, manual issuance clears retry block, set_by auto) — check TLSM rewrite agrees; APP app.canopy-type link should point at CNR.
Standards: RFC 8555, 9773, draft-ietf-acme-profiles, RFC 9525, 5280, 2986, 7468, 6066 (TLS); RFC 9457, 4648 (TCAN).
Glossary: Managed hostname, Strategy, Policy, Runtime-managed certificate (TLS); Canopy (TCAN/RLY); Report tick (CNR).

### Interface B (ACI SHL FWD LOG FLTS EVT KEY IMGM)
Open:
- SHL: shell volume-kind params validated but never bound: bind (spec says so), or exclude volume kinds from shells.
- FWD/SHL: malformed forward_id -> not_found, malformed session_id -> requirements_invalid: align on requirements_invalid, or keep.
- LOG: journal open failure reported as server_busy: internal, or keep.
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
- OPS: start action at boot (trigger `boot`) never run: implement, or drop the row and the language promise. (Joins ACN item.)
- OPS: queued operations in memory only; restart drops queued on_change handlers and operator actions: persist, report the drop, or accept.
- OPS: install that cannot be replayed goes to NotInstalled (strands resources); spec says recovered as stuck install (Uninstalling): keep spec, or NotInstalled + later sweep.
- OPS: unreadable persisted cancel flag: replay starts cancelled, or refuse with fault.
- OPS: unreadable persisted operation at startup: fault, or keep retrying.
- OPS: `seedling-op-` generated volume prefix not reserved (shadows operator mappings): reserve, or accept.
- OPS/TRD/ACI: app phases have no defining rule: which spec owns them.
- SUSP: action log committed only at end of pass, so crash after signal/write/exec repeats it: (a) record intent before effect, (b) commit after each effect, (c) weaken language wording.
- AUTO: reconciler-issued restart of a failed unit recorded as deliberate (never counts toward crash loop): should count as recovery?
- OBS: observe_failed clears only when next seen running and healthy (code) vs next successful observation (spec): confirm.
- OBS: each (instance, kind) recorded once until restart, so later health changes are lost (contradicts LCY unhealthy->Ready): spec says don't record an unchanged fact again: confirm.
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
- WIRE: malformed actor silently replaced by a synthesised one: refuse with requirements_invalid (rec.), or keep.
- WIRE: unparseable/oversized request -> not_found: switch to requirements_invalid?
- STS: unreadable values reported as null (confirm representation).
- APPS: resource `type` externalvolume vs def.kind external_volume vs plan ExternalVolume: unify?
- APPS: container env in /apps/show and plan diffs can carry plaintext secret params: redact? (security)
Spec leads code:
- TRN: bootstrap authorized_keys fingerprints imported without normalising (uppercase/prefixed never match).
- WIRE: control request over 64 KiB with no newline loses its actor (affects pushed bundles via web); /forwards/start error responses lack trailing newline; `deregistering` code defined but unused (dropped).
- STS: active_faults falls back to 0, failed inspect read as stopped, app status falls back to degraded (spec: null).
- APPS: /apps/show defaults unreadable state to empty/null and status to Degraded (spec: internal); unknown_params returned decrypted from secret storage (security); next_fire_at null whenever no schedule rows; removal maps db failure to not_found and only logs purge failures; update/param changes read params via a function returning empty on error.
- CTL: database failures reported as script_error; PRM handlers as not_found (spec: internal).
- PRM: plan with no input omits errors (spec: empty array).
Standards: RFC 9000, 9001, 9846 (TLS 1.3, obsoletes 8446), 7250, 7301, 9221, 8259, 3339, 5280, 8410, 4648, 1952, FIPS 180-4, OCI Image, OCI Distribution, SemVer 2.0.0.
Glossary: Fingerprint (TRN).
