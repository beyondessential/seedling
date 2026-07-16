# Logic bug audit — July 2026

A whole-codebase audit for logic bugs, carried out ahead of larger-scale deployment. Every crate and each subsystem within them was reviewed: `protocol`, `core` (defs, OI server and handlers, runtime, system), `daemon`, `ctl`, and `web`. Line numbers refer to commit `e14aa263f197bd03fc516464c1ac660945a07f92` (v0.4.5).

This report only catalogues findings; nothing has been fixed. Findings are grouped by subsystem, and each is categorised along two axes: severity/impact and testability of the fix.

## Method and limitations

The audit was performed as seventeen independent subsystem reviews, each reading its scope in full and cross-checking suspicions against callers, callees, `docs/spec/`, and existing tests before reporting. Reviewers were instructed to report only logic defects — behaviour the code would actually exhibit that is wrong — and to exclude style, missing features, performance, and speculative hardening. Behaviour asserted by an existing test or mandated by the spec was treated as intentional.

Limitations to keep in mind:

- Each finding carries a **confidence** rating (certain / likely / possible) from the reviewer. The critical finding was additionally verified by hand, and three findings were independently discovered by two reviewers (noted inline). The rest have not had a second adversarial pass.
- Severity judges blast radius when the bug fires, weighted by how ordinary the trigger is. A "low" that fires in an exotic configuration can still matter for a specific deployment.
- Line numbers will drift as the code changes; titles and symbol names are the stable reference.

## How to read the categories

**Severity / impact:**

- **critical** — data loss, data corruption, security failure, or a full outage from a routine operation.
- **high** — a feature or guarantee is broken in common flows; wrong behaviour an operator will plausibly hit in normal use.
- **medium** — wrong behaviour in edge cases, or wrong operator-visible state that misleads without directly breaking workloads.
- **low** — minor or cosmetic wrongness, latent bugs with no reachable production path today, or defects that fail safe.

**Fix testability:**

- **easy** — the fix is exercisable by a pure unit test (pure function, in-memory `Db`, or a BSL eval test).
- **moderate** — needs an existing harness: the stub `System` implementation, the OI `test_support`/TestOi harness, or a small purpose-built stub (fake DNS server, loopback sockets, fault-injecting DB handle).
- **hard** — needs real podman, systemd, journald, btrfs, jool, or network integration to observe the fixed behaviour.

Testability rates the *fix*, not the discovery: several hard-to-reproduce bugs have easy-to-test fixes once the decision logic is extracted into a function.

## Summary

138 findings across 17 subsystem reviews.

| Severity | Easy fix test | Moderate | Hard | Total |
|---|---|---|---|---|
| Critical | 0 | 1 | 0 | **1** |
| High | 8 | 13 | 1 | **22** |
| Medium | 24 | 27 | 3 | **54** |
| Low | 40 | 17 | 4 | **61** |
| **Total** | **72** | **58** | **8** | **138** |

Nearly all fixes (94%) are testable with unit tests or existing in-repo harnesses; only 8 need real system integration.

### Priority list: critical and high findings

The one critical finding:

| # | Finding | Subsystem | Testability |
|---|---|---|---|
| C1 | Failed script evaluation in `/apps/update` still triggers destructive post-reload actions: live volume data is held (relocated), scaling decisions wiped, forwards torn down — all from a typo in the script | OI app handlers (§4) | moderate |

The 22 high findings, roughly ordered by (blast radius × likelihood):

| # | Finding | Subsystem | Testability |
|---|---|---|---|
| H1 | Transient observe failure kills a running Job permanently (failed observation treated as observation of absence) | Reconcile (§12) | moderate |
| H2 | Uninstall unit-prefix match stops sibling apps whose names extend the uninstalling app's name (`app` matches `app-db`); uninstall also never completes | Reconcile (§12) | moderate |
| H3 | Image pull retries have no back-off, and after 5 failures the image is marked exhausted forever — a 30-second registry blip permanently disables actuation of the workload | Actuator (§13) | moderate |
| H4 | Observer can never detect systemd's start-limit-hit (`SubState` vs `Result` property), so the spec-mandated `crash_loop` hard fault is dead code and broken containers restart forever | Actuator (§13) | hard |
| H5 | `rt.stop`/`rt.start`/`rt.query` on a named Deployment use the declared lower bound, not the effective scale — maintenance actions stop 1 of N replicas while the script believes the deployment is down | Barrier (§8) | moderate |
| H6 | Second and later chained barriers (`.running().ready(30)`) never record `started_at`, so their deadlines are never enforced and wedged operations suspend forever | Barrier (§8) | easy |
| H7 | `rt.signal` replay dedup is value-based, not positional: a second identical signal call is silently swallowed; changed instance sets re-deliver on replay | Barrier (§8) | easy |
| H8 | Later volume success erases earlier volume's `backup_failed` faults in the same strategy run — permanent backup failures are invisible depending on volume declaration order | Backups (§6) | moderate |
| H9 | Exact-hostname fast path serves stale/expired certs over newer valid covering certs, with no `not_after` check anywhere on the serve path | Runtime TLS (§9) | easy |
| H10 | Tailscale issuance bypasses retry blocks and failure debounce, hammering tailscaled every 5 s tick; the attempt-row flood also evicts other hostnames' debounce state, unleashing ACME retry storms | Runtime TLS (§9) | moderate |
| H11 | Tailscale provider never marks the discovered ingress stale when tailscaled is unreachable — the most common outage mode, explicitly required by the spec | Runtime networking (§11) | moderate |
| H12 | Pod /64 network prefixes derive from 8 bits of instance entropy: all static Jobs collide on one prefix, and 20 replicas have better-than-even odds of a collision that permanently fails pod startup | System networking (§14) | easy |
| H13 | Resolver blue/green upgrade starts the new slot on the IP the old slot still holds, and health-checks the old container — the upgrade can never succeed and stalls every tick | System networking (§14) | moderate |
| H14 | Daemon starts with zero OI listeners when `--interface` fails to resolve (documented as fatal) — silent management-plane outage on a boot-time race | Daemon (§16) | easy |
| H15 | UDP port forward terminates permanently on a single oversized datagram (EDNS, QUIC); the spec's drop-and-report model and `max_udp_payload` are ignored | ctl (§16) | moderate |
| H16 | `subscribe_events` swallows error responses then blocks forever in `accept_uni()` — a transient `server_busy` permanently and silently kills the web UI's event pipeline | Protocol (§1) | moderate |
| H17 | `/logs/stream` in the web gateway hangs forever when the daemon returns an error response (same shape as H16) | Web (§17) | moderate |
| H18 | Advertised WebTransport port is hard-coded to 7893 whenever explicit listen addresses are used — documented flag combinations break all WT connectivity | Web (§17) | easy |
| H19 | WT certificate rotation swaps only *after* expiry on an hourly timer — recurring window of up to an hour where all new web sessions fail TLS | Web (§17) | easy |
| H20 | Inverted scale ranges (`scale(5..2)`) are accepted at eval, then `Ord::clamp` panics the daemon on the first scale request | core/defs (§2) | easy |
| H21 | UDP relay task dies permanently on a transient socket error (ICMP port-unreachable) or zero-length datagram; the forward stays listed as healthy | OI forwards (§3) | moderate |
| H22 | `/apps/uninstall` has no operation-in-progress gate: uninstall races a running action closure and tears down resources mid-operation | OI app handlers (§4) | moderate |

## Cross-cutting themes

Several failure patterns recur across unrelated subsystems. Fixing the pattern, not just the instance, is likely worthwhile:

1. **Error responses on bidi streams are discarded by clients.** `subscribe_events` (protocol), the web gateway's `/logs/stream`, and `ctl events`' exit code all read the first response line and ignore or mishandle the error case; two of the three then block forever on a uni stream that will never open. The `ctl` subscribe path does this correctly — the pattern should be shared, not re-implemented per caller. (§1, §16, §17)

2. **Failure paths that keep partial state instead of the previous good state.** `AppRegistry::reload` swaps in a partially-evaluated AppDef on script error (the critical finding); a compute error for one app drops it from the reconcile tick so the applied absolute state tears down its data plane (an all-apps failure triggers full idle teardown); `register_app` leaves an in-memory entry when the DB write fails. The invariant "on failure, observable state is unchanged" needs enforcing at each boundary. (§10/§4, §12)

3. **Absence of observation conflated with observation of absence.** A failed podman/systemd query yields an `ObservedInstance` with every flag false, which the Job terminal-detection reads as "naturally terminated" and kills the Job; `ContainerStatus::Unknown` (podman's `stopping`, `removing`, `initialized`) maps to `ContainerMissing`/`container_removed`. Observation failures need to be first-class ("unknown"), not defaulted to "gone". (§12, §13, §15)

4. **Fault lifecycle asymmetries.** Faults are filed without dedup (`audit_lag`), cleared too broadly (backup success wipes all `backup_failed` faults for the app; `stop_sent` clears the same tick's `stop_failed`), never cleared (`ingress_conflict` after a restart, because the prior-state set is in-memory only; `disallowed_registry` after `registries/add`), or promised but never filed (`tailscale_unreachable`). A shared file/clear discipline — dedup on file, sweep against current state rather than in-memory diffs on clear — would fix a whole class. (§6, §7, §11, §12, §5)

5. **Name/prefix matching without reserved namespaces.** Uninstall matches units by `starts_with("seedling-{app}-")`, so hyphenated sibling app names collide; the `backup-snap-` site-volume prefix is deleted at startup but nothing stops operators creating volumes with that prefix; the discovered ingress named `tailscale` collides with operator-created manual ingresses; pod /64 prefixes derive from one UUID byte. Either reserve the namespaces at creation time or match on exact identity. (§12, §6, §11, §14)

6. **Silent coercion in the BSL defs layer.** `into_string().unwrap_or_default()`, `try_cast`/`filter_map`, and unchecked `as` casts turn script type errors into empty strings, dropped criteria (a malformed `select` matches *everything*), truncated integers (`pids_limit` wrapping to 1), and defaulted kinds — all surfacing far from the cause, sometimes as workload-control operations on the wrong resource set. The defs layer should throw on malformed input, matching its own validation style elsewhere. (§2)

7. **Retry logic that either hammers or gives up forever.** No back-off with permanent exhaustion (image pulls), no back-off at all (Tailscale issuance every tick), single-error permanent death (UDP relay, UDP forward, event broker consumers). Each retry loop needs both a back-off and a recovery path. (§13, §9, §3, §16)

8. **Restart/replay correctness around persisted state.** `save_current_operation` resets the persisted cancel flag; queued schedule fires are stamped `last_fired_at` but lost on restart; replay-abandoned dynamic resources are never torn down; barrier replay dedup matches by value rather than position. Anything persisted for crash-recovery deserves a restart-shaped test. (§7, §6, §16, §8)

## Suggested sequencing

1. **C1** plus the barrier trio (**H5, H6, H7**) — these undermine the core "operations are safe and replayable" contract that everything else builds on, and the barrier fixes are unit-testable.
2. The self-healing/recovery set (**H1–H4, H8**) — at deployment scale, transient errors are routine; these turn transients into permanent, silent failures of exactly the machinery that is supposed to absorb them.
3. The connectivity set (**H11–H19**) — TLS serving, resolver upgrade, pod network prefixes, WT/event plumbing.
4. **H20–H22** and the medium tier, prioritising the easy-testability column (24 medium findings have pure-unit-test fixes).

---


## Findings by subsystem


### 1. Protocol crate (crates/protocol)

#### `subscribe_events` swallows error responses and then blocks forever waiting for a uni stream that will never arrive

- **Location**: `crates/protocol/src/client.rs:342-370`
- **Bug**: `subscribe_events()` reads the first response line of `/events/subscribe` and unconditionally discards it, assuming it is `{"result":{}}`. The daemon can instead answer with an error on that bidi stream — e.g. `crates/core/src/oi/server.rs:286-306` writes `{"error":{"code":"server_busy",...}}` when the stream-concurrency semaphore is exhausted — and in that case it never opens the server-initiated uni stream. The client then parks in `accept_uni()` indefinitely: the daemon sends QUIC keep-alive PINGs every 10 s (`server.rs:147`), so the otherwise-idle connection never hits the 30 s idle timeout and `accept_uni()` never resolves. Notably, `crates/ctl/src/subscribe.rs:93-98` checks the response for `"error"` before accepting the uni stream — the protocol-crate helper skips exactly that check.
- **Failure scenario**: Daemon momentarily at `--max-streams` when the web UI's event broker (`crates/web/src/event_broker.rs:103`) calls `subscribe_events()` → the `server_busy` error is silently discarded → `stream_events` hangs forever inside `subscribe_events()`, `run_event_broker`'s reconnect loop never runs again, and the web UI silently receives no events until the web process is restarted. The same happens if the server drops the request line and finishes the stream empty (`read_line` returning 0 is also ignored).
- **Severity**: high — a transient, expected server condition permanently and silently disables the entire event pipeline of a long-running consumer; the reconnect/backoff logic built around it is defeated.
- **Fix testability**: moderate — needs a stub QUIC server (repo already has an in-process OI server harness) that responds with `{"error":...}` to `/events/subscribe` and asserts `subscribe_events()` returns `Err(ClientError::Api{..})` promptly instead of hanging.
- **Confidence**: certain

#### `EnvVar` deserialisation bypasses the null-byte check on the value (serde asymmetry)

- **Location**: `crates/protocol/src/env.rs:175-179`
- **Bug**: `EnvVar::new` rejects values containing `\0` (`env.rs:188-190`), and the doc comment states the value is "subject only to the POSIX 'no null byte' constraint", but `EnvVar` derives `Deserialize` with a plain `String` value, so deserialising a value containing a NUL byte succeeds. The name side is protected (via `EnvironmentVarName`'s custom `Deserialize`), the value side is not — the type's advertised invariant does not hold for wire-decoded instances.
- **Failure scenario**: Any future/wire consumer that decodes an `EnvVar` (the type is explicitly fuzzed as a wire decoder in `crates/protocol/fuzz/fuzz_targets/wire_decode.rs:18`) obtains a value with an embedded NUL, which later fails or truncates when handed to podman/execve-style consumers instead of being rejected at the validation boundary. Currently no in-tree production path deserialises `EnvVar`, which limits impact.
- **Severity**: low — invariant violation only reachable through deserialisation, and no in-tree production caller deserialises `EnvVar` today.
- **Fix testability**: easy — pure unit test: deserialise a value containing a NUL and assert it errors (mirror the existing `deserialize_rejects_invalid_name` test with a custom `Deserialize` impl).
- **Confidence**: certain

#### Server fingerprint pinning is case-sensitive, rejecting valid uppercase-hex fingerprints

- **Location**: `crates/protocol/src/client.rs:120-127`
- **Bug**: `FingerprintVerifier` compares the computed lowercase hex digest byte-for-byte (`ct_eq`) against the caller-supplied `expected` string with no case normalisation. Fingerprints reach this path directly from user input — `seedling-ctl --fingerprint` (`crates/ctl/src/main.rs:286`) and the web binary's config (`crates/web/src/main.rs:139`) — and no caller lowercases them.
- **Failure scenario**: Operator pastes a fingerprint in uppercase (or mixed-case, as many tools display SHA-256 fingerprints) → `verify_server_cert` fails with `ApplicationVerificationFailure` even though the fingerprint is correct → connection to a genuine server is refused with a misleading "connection failed" error.
- **Severity**: low — correct pins in canonical lowercase work; only non-canonical user input misbehaves, and it fails closed.
- **Fix testability**: easy — unit test `FingerprintVerifier` directly with an uppercase `expected` against a known SPKI (the verifier struct and `hex_digest` are pure).
- **Confidence**: likely

#### `InvalidName::Malformed` error message states the wrong maximum length

- **Location**: `crates/protocol/src/names.rs:23-26` (vs validator at `names.rs:38-44`)
- **Bug**: The rejection message claims names must match `^[a-zA-Z][a-zA-Z0-9-]{1,60}[a-zA-Z0-9]$`, which caps total length at 62; the validator (and the spec, `docs/spec/language.md:39`: "between 3 and 63 characters") accepts up to 63, and the test `app_new_longest_accepted` asserts a 63-char name is valid. The regex in the message understates the limit by one.
- **Failure scenario**: A user gets a `Malformed` rejection (e.g. for a 64-char name), reads the message, and concludes the limit is 62; conversely a 63-char name the message's regex says is invalid is actually accepted. The message never matches the implemented rule for boundary inputs.
- **Severity**: low — wrong diagnostic text only; validation behaviour itself matches the spec.
- **Fix testability**: easy — unit test asserting the Display output quotes `{1,61}` (or simply "3 to 63 characters"), alongside the existing boundary tests.
- **Confidence**: certain


### 2. core/defs (BSL app-definition layer)

#### Inverted scale ranges accepted; later panics the daemon
- **Location**: `crates/core/src/defs/deployment.rs:88-102`
- **Bug**: The `scale(range)` overload validates the lower bound (`<= 10`, non-negative) and the upper bound (non-zero, `<= u16::MAX`) but never checks `min <= max`, so `scale(5..2)` is stored as `5..2`. Spec `l[deployment.scale]` defines the range as lower/upper bounds, which is only coherent when lower ≤ upper.
- **Failure scenario**: A script declares `app.deployment("web").scale(5..2)` — accepted. `rt.start` schedules `scale.start.max(1) = 5` instances against a declared upper bound of 2; worse, `crates/core/src/runtime/scaling.rs:79`/`:100` call `stored.clamp(low, high)`, and `Ord::clamp` panics with `min > max`, so the first `/apps/scale` request (or `clamp_scaling_decisions` re-run with a stored decision) panics the daemon.
- **Severity**: high — a script-authorable value leads to a reachable panic (outage) plus contradictory scheduling in normal flows.
- **Fix testability**: easy — BSL eval unit test asserting `scale(5..2)` throws (harness `run_test_script_app` exists).
- **Confidence**: certain

#### Explicit `secret(false)` cannot override password kinds
- **Location**: `crates/core/src/defs/install.rs:23-27` (with `crates/core/src/defs/param.rs:155-165`, `crates/core/src/defs/app/install.rs:85-88`)
- **Bug**: Spec `l[param.schema.secret-from-kind]` and `l[action.install.requirements]` say password/weak-password kinds imply secret "unless explicitly overridden with `param.secret(false)`" / "unless explicitly set to `false`". `ParamDef.secret` is a plain `bool` (no unset/tri-state) and `is_secret()` returns `self.secret || matches!(kind, Password | WeakPassword)`, so an explicit `false` is indistinguishable from unset and the OR makes the override impossible.
- **Failure scenario**: `app.param("db-pass").kind("weak-password").secret(false);` → `is_secret()` still returns `true`; the value is treated as confidential (never returned to API clients, redacted in UI) despite the operator's explicit opt-out. Same for `#{ kind: "password", secret: false }` in install/action schemas.
- **Severity**: medium — wrong behaviour in a documented flow, though it fails in the safe direction.
- **Fix testability**: easy — pure unit test on `ParamDef::is_secret` / BSL eval test (`tests/param.rs` already covers the other direction).
- **Confidence**: certain

#### `col(action)` coerces an Action into a Collection, contradicting the spec
- **Location**: `crates/core/src/defs/collection.rs:162-171`
- **Bug**: Spec `l[collection.col]` says "Action handles are not coercible to a Collection" (and `l[action.type]`: an Action "cannot be passed to `rt.start`, `rt.stop`, or any other resource-scheduling method"), i.e. `col(action)` must fall through to the empty-Collection case. The code has an explicit branch turning an `Action` into a one-item `ItemBag` collection with `ResourceKind::Action` — a leftover from the actions-as-resources era (the comments in `resource.rs:52-58` and `collection/bag.rs:13-17` describe the new model, but this branch was never removed).
- **Failure scenario**: `rt.start(col(app.action("backup")))` or `col([dep, some_action])` yields a collection containing an Action handle; resource-scheduling paths then receive a `ResourceKind::Action` entry instead of the spec-mandated empty/ignored result.
- **Severity**: medium — spec-violating behaviour on a real API surface; consequences depend on downstream handling of the bogus handle.
- **Fix testability**: easy — unit test that `col(Dynamic::from(Action::new(...)))` resolves to zero handles.
- **Confidence**: certain

#### `pids_limit` and `stop_timeout` silently truncate i64 → u32
- **Location**: `crates/core/src/defs/container.rs:651` and `crates/core/src/defs/container.rs:690`
- **Bug**: Both builders check only `<= 0` then do `as u32`. Values above `u32::MAX` wrap instead of erroring (unlike `take_retries` at line 251, which checks the bound).
- **Failure scenario**: `pids_limit(4294967297)` passes validation and stores `1` — the container is limited to a single PID and fails to start any workload. `stop_timeout(4294967296)` stores `0` — `TimeoutStopSec=0`, i.e. no/immediate timeout instead of the huge value requested.
- **Severity**: medium — wrong behaviour only for extreme inputs, but the resulting values are dangerous (effectively opposite of intent) and silent.
- **Fix testability**: easy — BSL eval unit test asserting out-of-range values throw.
- **Confidence**: certain

#### Redirect status code unvalidated and wrapped i64 → u16
- **Location**: `crates/core/src/defs/ingress.rs:190-201`
- **Bug**: The `redirect(port, code)` overload validates `port` via `Port::new` but stores `code as u16` with no validation at all: negative and > 65535 values wrap, and non-redirect codes (e.g. `0`, `42`, `200`) are accepted.
- **Failure scenario**: `ingress.redirect(80, 99301)` silently stores code `33765`; `redirect(80, -1)` stores `65535`. The edge proxy is later configured with a nonsensical HTTP status instead of the script throwing at evaluation time.
- **Severity**: medium — silent generation of an invalid ingress config from a plausible typo; edge-case inputs.
- **Fix testability**: easy — BSL eval unit test (existing `tests/ingress.rs` redirect tests to extend).
- **Confidence**: certain

#### Malformed `select` criteria silently invert to select-everything (or nothing)
- **Location**: `crates/core/src/defs/collection/selector.rs:18-51`
- **Bug**: `Selector::from_map` uses `try_cast`/`filter_map` throughout: if `types`/`names`/`name_patterns` is present but not an array, the whole criterion is dropped (`None`) and the filter matches **all** resources; if it is an array whose elements have the wrong type (e.g. strings instead of `ResourceType` values), the elements are dropped, producing `Some(vec![])`, which matches **nothing**. Unknown criterion keys are ignored too. Spec `l[collection.select.*]` gives no licence for silently ignoring malformed criteria.
- **Failure scenario**: `app.select(#{ types: ResourceType.Service })` (forgot the array brackets) returns every resource in the app; a follow-up `rt.stop(...)` on that collection then stops all workloads instead of the services. Conversely `#{ types: ["service"] }` silently selects nothing.
- **Severity**: medium — a one-character script mistake silently broadens a selection that drives workload control operations.
- **Fix testability**: easy — pure unit tests on `Selector::from_map`/`matches`.
- **Confidence**: certain (behaviour); the intended fix is to throw on malformed criteria

#### `external_service` bypasses action-context reference semantics
- **Location**: `crates/core/src/defs/app/service.rs:71-96` (contrast with the named-resource branch at lines 19-36; also `crates/core/src/defs/service.rs:372-375`)
- **Bug**: Unlike `app.service`/`deployment`/`job`/`volume`, `app.external_service(name)` has no `is_in_action_closure()` branch: inside an action closure it registers a brand-new ExternalService instead of returning a frozen reference to an existing static one, and never errors for a name with no static declaration (spec `l[app.resources.context.named]`: "If no static resource with that name exists, it is a script error"). External *volumes* are explicitly exempted by `l[volume.external.dynamic]`; external services have no such exemption. Additionally `ExternalService::description` has no freeze check, so static external services are mutable inside closures, violating `l[app.resources.context.immutable]`.
- **Failure scenario**: An action closure calls `app.external_service("postgers")` (typo). Instead of throwing, a fresh unmapped slot is returned, the closure mounts it, and the failure surfaces much later as an unresolvable mapping at reconcile time rather than as a script error.
- **Severity**: medium — silently swallows a class of script errors the spec requires to throw.
- **Fix testability**: moderate — needs the action-closure test harness (`tests/action.rs` pattern with `set_appdef_holder`).
- **Confidence**: likely

#### Non-string `kind` and non-map entries in param schemas silently accepted
- **Location**: `crates/core/src/defs/app/install.rs:52-61`
- **Bug**: `parse_param_defs` reads `kind` via `into_string().ok()`, so a `kind` that is present but not a string (e.g. `kind: 42`) silently falls back to `"text"` — spec `l[action.install.requirements.kind-unknown]` / `l[action.option-params]` require a throw whenever a provided `kind` doesn't match a defined kind. Entire param definitions that aren't maps (`params: #{ foo: "text" }`) are silently skipped rather than rejected.
- **Failure scenario**: `app.on_install(f, #{ params: #{ pw: #{ kind: true } } })` installs with `pw` as plain text (no password validation, not implicitly secret); `params: #{ pw: "password" }` declares no params at all — required-field validation never happens.
- **Severity**: low — wrong behaviour for malformed schemas only, but it downgrades password params silently.
- **Fix testability**: easy — pure unit test on `parse_param_defs`.
- **Confidence**: likely

#### `volume.write` rejects non-escaping `..` paths the spec allows
- **Location**: `crates/core/src/defs/volume.rs:18-24`
- **Bug**: Spec `l[volume.write.validation]` forbids paths that "escape the volume root **after canonicalisation** (resolving `.` and `..` segments)". The implementation rejects any path containing a `..` component outright, including ones that canonicalise safely inside the root.
- **Failure scenario**: `volume.write("/etc/app/../app.conf", data)` — canonicalises to `/etc/app.conf` (inside the volume) — throws instead of succeeding.
- **Severity**: low — rejects valid input in an unusual edge case; fails safe.
- **Fix testability**: easy — pure unit test on `validate_volume_write_path`.
- **Confidence**: likely (could be intentional strictness, but the spec text specifies canonicalise-then-check)

#### Ingress on an anonymous service is silently inert instead of throwing
- **Location**: `crates/core/src/defs/service.rs:158-192` (the `if let Some(arc) = service.app_def...` guard) with `crates/core/src/defs/ingress.rs:47-52`
- **Bug**: For an anonymous service created inside an action closure (`app_def: None`, mutable), `declare_ingress` succeeds but skips both the conflict check and registration into `AppDef.resources`, returning a dangling Ingress; every subsequent builder call on it (`.tls`, `.redirect`) then fails because `Ingress::is_frozen` is unconditionally true inside closures. Spec `l[app.resources.context.anonymous]` says Ingress "has no anonymous form in any context", so the creation itself should throw.
- **Failure scenario**: Inside an action: `let s = app.service(); s.ingress("example.com", 443);` — no error, but no ingress is ever registered or routable; the script author gets silent no-op behaviour (or a confusing frozen error only when chaining `.tls`).
- **Severity**: low — silent no-op in an unsupported edge combination.
- **Fix testability**: moderate — needs an action-closure harness to exercise the anonymous-service path.
- **Confidence**: likely

#### Non-string array elements silently become empty strings in command/arg/env/healthcheck cmd
- **Location**: `crates/core/src/defs/container.rs:463-473`, `:489-501`, `:283-291`, `:521-550`
- **Bug**: All array-taking builders use `into_string().unwrap_or_default()`, so non-string elements are silently converted to `""` (and non-map `env` array items are skipped entirely) instead of throwing. `take_command_cmd` only rejects when *all* elements are empty.
- **Failure scenario**: `container.command(["nginx", 8080])` stores `["nginx", ""]`; the container launches with an empty argv element — a confusing runtime failure with no script-time diagnostic. `healthcheck(#{kind:"command", cmd:["curl", 80]})` similarly produces `["curl", ""]`.
- **Severity**: low — requires a type mistake in the script, but the corruption is silent and surfaces far from the cause.
- **Fix testability**: easy — BSL eval unit tests.
- **Confidence**: likely (silent coercion is clearly not asserted anywhere; throwing matches the surrounding validation style)

#### Digest "hex" validation accepts any lowercase letters
- **Location**: `crates/core/src/defs/container.rs:69-78`
- **Bug**: The digest check uses `is_ascii_lowercase() || is_ascii_digit()`, which accepts `g`–`z`, while the error message and OCI digest grammar require hex. `sha256:zzzz…` (≥32 chars) validates.
- **Failure scenario**: `image("docker.io/lib/app@sha256:" + "zx".repeat(32))` passes BSL validation and only fails later when podman attempts the pull.
- **Severity**: low — accepts invalid input; failure is deferred, not incorrect execution.
- **Fix testability**: easy — pure unit test on `validate_image_ref`.
- **Confidence**: certain (behaviour contradicts its own error message)

#### Cron `H` hash uses `DefaultHasher`, which is not stable across Rust releases
- **Location**: `crates/core/src/defs/action.rs:21-26`
- **Bug**: `schedule_hash` feeds cronexpr's `H` extension from `std::collections::hash_map::DefaultHasher`, whose algorithm is explicitly unspecified across std releases. Spec `l[action.schedule]` promises a "stable hash-derived value" (e.g. "fires once daily at a stable minute"); a toolchain upgrade can silently move every `H`-scheduled action to a different minute/hour.
- **Failure scenario**: Daemon rebuilt with a newer Rust that changes `DefaultHasher` → all `H 2 * * *` schedules shift, breaking operator expectations of stable firing times (and any coordination based on them).
- **Severity**: low — behaviour is correct within any single build; drift only across toolchain upgrades.
- **Fix testability**: easy — switch to an explicitly seeded stable hasher and pin with a unit test asserting a known hash value.
- **Confidence**: possible


### 3. OI server, auth, forwards, shells

#### UDP relay task dies permanently on transient socket error or zero-length datagram
- **Location**: `crates/core/src/oi/forwards/session.rs:381-412`
- **Bug**: In `udp_relay_task`, the `socket.recv` arm matches `Ok(n) if n > 0` and treats everything else — `Ok(0)` (a legal zero-length UDP datagram) and `Err(_)` — as `break`, exiting the relay task forever. On Linux a connected UDP socket surfaces ICMP port-unreachable as `ECONNREFUSED` on the next `recv`, so a single datagram sent while the target service is down/restarting kills the relay. The forward stays registered, the control stream stays open, `/forwards/list` still shows it, and subsequent client datagrams hit `TrySendError::Closed` in `server.rs:252` and are silently dropped. No `forward.status` message is emitted (the relay's `status_tx` clone is just dropped; the session's own copy keeps `status_rx` pending forever).
- **Failure scenario**: Operator starts a UDP forward (allowed even before the service instance runs, since target resolution is `get_or_create_singleton`), sends one DNS query while the container is briefly down → ICMP refused → relay task breaks. Service comes back; forward is permanently dead but appears healthy.
- **Severity**: high — a routine, transient condition (target briefly not listening) silently and irrecoverably bricks a forward in a common flow.
- **Fix testability**: moderate — unit test spawning `udp_relay_task` against a closed local UDP port (real sockets, no podman) asserting the task survives and reports status.
- **Confidence**: certain

#### Shell session is stoppable/resizable only after container start, but `session_id` is issued at handshake
- **Location**: `crates/core/src/oi/shells/session.rs:185-205` (handshake) vs `:488-497` (registry insert); same pattern in `volume_session.rs:283-302` vs `:387-396`
- **Bug**: The handshake response carrying `session_id` is written before the shell closure runs (`spawn_blocking`, including a `Suspended` retry loop sleeping 2 s per iteration), before image pull (or `podman build` for volume shells), network creation, and `exec`. The session is inserted into `ShellRegistry` only after all of that. During this potentially minutes-long window, `/shells/stop` and `/shells/resize` with the valid, already-issued `session_id` return `not_found`, the session is absent from `/shells/list`, and the control stream (`recv`) is not polled, so a stop request is simply lost and the container starts anyway.
- **Failure scenario**: Client opens a shell, image pull takes 60 s, user aborts; client calls `/shells/stop { session_id }` → `not_found`. Container subsequently starts and runs; likewise a terminal resize during startup errors and the size change is lost.
- **Severity**: medium — wrong behaviour (lost stop, spurious not_found) in the startup window; spec (`i[shell.stop]`/`i[shell.resize]`) says `not_found` means the session does not exist, but the server itself told the client it does.
- **Fix testability**: moderate — needs the stub System harness plus a slow/blocking stubbed `pull_image` to widen the window; assert stop before registration takes effect.
- **Confidence**: certain

#### Relay loops serialise the two data directions and can deadlock under mutual backpressure
- **Location**: `crates/core/src/oi/forwards/session.rs:314-337` (TCP relay); `crates/core/src/oi/shells/session.rs:520-567` and `volume_session.rs:415-460` (shell stdin/stdout)
- **Bug**: Both relays use a single `select!` loop where the winning branch `await`s a `write_all` to the opposite peer inside the branch body. While blocked writing (peer not reading, buffers full), the other direction is not drained, and in the shell loop `child.wait()`/`stop_rx` are not polled either — so `/shells/stop` cannot terminate the session (the oneshot fires but is never observed) and no SIGTERM is sent.
- **Failure scenario**: TCP forward to an HTTP server that stops reading the request body while emitting a large error response: relay blocks in `tcp_send.write_all`, target blocks writing its response, relay never reads `tcp_recv` → classic proxy deadlock; the stream hangs until one side's TCP stack gives up, if ever. Shell variant: client floods stdin while the job floods stdout without reading stdin → PTY buffers fill both ways → session wedged and un-stoppable.
- **Severity**: medium — requires simultaneous bidirectional saturation, but the outcome is a hang that even the stop path cannot break.
- **Fix testability**: moderate — two local sockets with a peer that writes without reading; assert relay progress/stop responsiveness.
- **Confidence**: likely

#### TCP forward tears down both directions on client half-close, truncating in-flight responses
- **Location**: `crates/core/src/oi/forwards/session.rs:316-324`
- **Bug**: In `handle_forward_stream`, a QUIC-side FIN (`recv.read` → `Ok(None)`) breaks the whole relay loop and finishes the stream, discarding any response data still flowing from the TCP target. Protocols that use `shutdown(SHUT_WR)` after sending a request lose the tail of the response. (The ctl client mirrors this at `crates/ctl/src/forward.rs:193-202`, so the truncation is end-to-end; spec `i[forward.tunnel.tcp]` says "until either end closes", which arguably licenses this — but half-close is not close, and the result is silent data truncation.)
- **Failure scenario**: A forwarded client sends a request, half-closes, and waits for the response (common for one-shot RPC/netcat-style tools) → connection closed mid-response, payload truncated with no error.
- **Severity**: low — only affects protocols relying on TCP half-close semantics, and the peer client currently behaves the same way.
- **Fix testability**: moderate — TCP echo target plus a stubbed stream pair; assert bytes written after client-side FIN still arrive.
- **Confidence**: likely

#### Forward-key wraparound: removing a TCP forward can delete a live UDP forward's datagram route
- **Location**: `crates/core/src/oi/forwards/registry.rs:105-126` (`insert`/`remove`), `:90-103` (`alloc_key`)
- **Bug**: `alloc_key` allocates keys for TCP and UDP forwards alike, but `insert` only records the key in `conn_key_to_id` for UDP entries, while `remove`/`remove_stale_for_app` unconditionally remove `(conn_id, forward_key)` from that map. `alloc_key` also only treats UDP keys as "in use". After the per-connection counter wraps (65 536 allocations over a connection's lifetime), a UDP forward can be issued a key currently held by a live TCP forward; when that TCP forward later closes, its removal deletes the UDP forward's `conn_key_to_id` entry, and all subsequent datagrams for the UDP forward are dropped as "datagram for unknown forward key".
- **Failure scenario**: Long-lived connection cycles >65 536 forwards (e.g. scripted tooling), then a TCP forward stop silently kills an unrelated active UDP forward's traffic.
- **Severity**: low — mechanism is certain but requires counter wraparound on one connection.
- **Fix testability**: easy — pure unit test on `ForwardRegistry`: pre-set `key_counters`, insert TCP entry with key K, wrap, insert UDP with key K, remove TCP, assert `get_udp_sender` still resolves.
- **Confidence**: certain

#### Spec-required forward status messages for relay failures and backpressure are never sent
- **Location**: `crates/core/src/oi/forwards/session.rs:354-364` (bind/connect failures); `crates/core/src/oi/server.rs:250-257` (`TrySendError::Full` silently ignored)
- **Bug**: `i[forward.status]` requires "relay task failures" and "datagram backpressure" to be reported on the control stream; only the oversized-datagram case is implemented. UDP relay bind/connect failures log a server-side warning and return, leaving the client with a successfully-established, silently non-functional forward; datagrams dropped because the 64-slot relay channel is full (`TrySendError::Full` falls through the `if let Closed` pattern) are not reported either.
- **Failure scenario**: `UdpSocket::bind`/`connect` fails (e.g. ephemeral port exhaustion) right after `/forwards/start` returned success → every datagram vanishes with no status message and no event.
- **Severity**: low — failures still fail, but invisibly, contradicting the spec's operator-feedback contract.
- **Fix testability**: moderate — drive `udp_relay_task` with an induced bind failure and assert a status message arrives on `status_rx`.
- **Confidence**: certain

#### `/forwards/start` does not validate `port` against the service's declared ports
- **Location**: `crates/core/src/oi/forwards/session.rs:90-116`
- **Bug**: The lookup verifies the app exists, is installed, and that a Service with the given name exists, but never checks `params.port` against the ports declared via `service.port()`. Spec `i[forward.request]` defines `port` as "a port number on that Service as defined by `service.port()`", and `i[forward.script-update]` requires teardown when the *port* is no longer declared — implying declared-port membership is part of the contract. A typo'd port returns success and a dead forward.
- **Failure scenario**: `/forwards/start { app, service, port: 5433 }` against a service declaring only 5432 → success response, forward listed, every tunneled connection just fails to connect.
- **Severity**: low — misconfiguration surfaces as connection failures instead of `requirements_invalid`; no state corruption.
- **Fix testability**: moderate — needs the stubbed OI + a quinn loopback (or refactoring the lookup into a testable function).
- **Confidence**: likely

#### App-shell early stdin-failure path leaks the pod network
- **Location**: `crates/core/src/oi/shells/session.rs:508-516` (compare `volume_session.rs:408-413`)
- **Bug**: If the initial `leftover_stdin` write to the PTY fails right after `exec`, the app-shell path removes the session and emits `ShellExited` but never SIGTERMs/waits the child and never calls `remove_network(&net_name)` — the volume-shell equivalent does remove the network, confirming the omission. The podman network `seedling-<container>` persists indefinitely (the container itself likely dies via PTY-master-close SIGHUP, but the network object leaks until manual cleanup).
- **Failure scenario**: PTY write error immediately after container start (client sent stdin bytes with the request and the exec's stdin pipe failed) → orphaned podman network accumulates.
- **Severity**: low — rare path; leaks a network object rather than corrupting state.
- **Fix testability**: hard — needs a real or elaborately stubbed exec handle whose stdin write fails; simplest as code-inspection parity fix with the volume path.
- **Confidence**: certain

#### Shell error/exit frames on failure paths violate the newline-delimited framing
- **Location**: `crates/core/src/oi/shells/session.rs:311, 322-328, 335-341, 375-379, 395-398, 423-427, 468-471, 512-514`; `volume_session.rs:91-99` (`fail!` macro)
- **Bug**: `i[stream.shell.framing]` specifies the server-to-client direction of the session stream as newline-delimited JSON (handshake, then exit frame). Every post-handshake failure path writes `{"exit_code":-1}` or `{"error":...}` **without** the trailing `\n` (the success paths append it), and some emit an `{"error":...}` object where the spec only defines an exit frame. A strict NDJSON reader waiting for a newline only recovers via stream EOF, and may not recognise the error object at all.
- **Failure scenario**: Shell script compile failure or image pull failure after handshake → client's line-based frame parser stalls until EOF or misparses the terminal frame.
- **Severity**: low — most readers recover at EOF; inconsistency rather than data loss.
- **Fix testability**: moderate — stubbed runtime driving `open_shell_session` over a quinn loopback, assert every terminal frame is newline-terminated.
- **Confidence**: certain


### 4. OI handlers: apps, actions, params, templates, status, faults

#### Failed script evaluation in `/apps/update` still triggers destructive post-reload actions (volume hold, scaling wipe, forward teardown)

- **Location**: `crates/core/src/oi/handler/apps.rs:1561` (reload) and the dependent blocks at `apps.rs:1573`, `apps.rs:1684`, `apps.rs:1754`, `apps.rs:1781`
- **Bug**: Spec `i[app.update]` says a script that fails to evaluate files a `script_error` fault and "the existing AppDef continues running". But `AppRegistry::reload` (`runtime/apps.rs:156-169`) unconditionally swaps in the partially-evaluated `App` even on failure (contradicting its own doc comment), and `update_app` then runs every post-reload step against that partial def without checking `entry.script_error`: the named-volume diff (`r[impl actuate.volume.hold]`), scaling clamp, stale-forward teardown, and schedule prune.
- **Failure scenario**: Installed, running app with a named volume `data`, a scaled deployment, and scheduled actions. Operator submits `/apps/update` with a script containing a parse error (partial def = zero resources). Result: `data`'s on-disk directory is `fs::rename`d into the held-volumes area and its `resource_instances` rows deleted (apps.rs:1591-1674), all scaling decisions are deleted (`clamp_scaling_decisions` with an empty bounds map, apps.rs:1684-1712), all port forwards for the app are stopped (apps.rs:1754-1778), and all schedule rows (including `last_fired_at`) are pruned (apps.rs:1781) — all from a typo, while the request "succeeds" per spec. Fixing the typo does not bring the volume data back; a fresh empty volume is created.
- **Severity**: critical — a routine typo in a script update detaches live volume data and wipes operator scaling/schedule state on a running app.
- **Fix testability**: moderate — TestOi harness: register+install app with a volume, update with `throw "boom"`, assert volume not held / scaling decisions intact (volume hold path needs the stub volume store). The root-cause fix site (`AppRegistry::reload` keeping the old def on failure) is unit-testable on its own: register a valid script, reload with an erroring script, assert the entry still contains the original resources.
- **Confidence**: certain — independently found by two auditors (via the handler flow and via `AppRegistry::reload` directly) and manually verified against the source. The same swap also happens on `set_param`/`unset_param` reload failures via `reload_and_persist_apperror`, so those paths need the same guard.

#### `/apps/uninstall` has no operation-in-progress gate

- **Location**: `crates/core/src/oi/handler/apps.rs:1405-1464`
- **Bug**: Every other desired-state mutation (`update_app`, `deregister_app`, `stop_resource`, `unstop_resource`, `unstop_all_resources`, `set_param`/`unset_param`) rejects with `operation_in_progress` when `scheduler.has_operation_for(app)` is true — per `i[resource.stop.no-active-op]`: "desired-state mutations must not race with a running action closure". `uninstall_app` only checks the phase (which stays `Installed` during a non-install operation), so an uninstall is accepted mid-operation. It neither rejects nor cancels the op.
- **Failure scenario**: A long action (e.g. scheduled backup) is running; operator calls `/apps/uninstall`. Phase flips to `Uninstalling`, the reconciler switches to `compute_uninstalling` and tears down all resources while the action closure is mid-flight (possibly mid-write inside a job container, which gets killed). The closure's barriers can never be satisfied; the scheduler slot stays occupied until the op times out/fails, during which the app has already been torn down to `NotInstalled` and its `resource_instances` rows deleted under the still-"active" operation.
- **Severity**: high — silently races running operations in a normal operator flow; can corrupt in-progress action work (backups, migrations).
- **Fix testability**: moderate — TestOi + stubbed scheduler: occupy the scheduler slot for the app, call `/apps/uninstall`, assert `operation_in_progress`.
- **Confidence**: likely (no test or spec blesses uninstall-during-op; all sibling endpoints gate on it)

#### `/server/status` hardcodes `active_operations` to 0

- **Location**: `crates/core/src/oi/handler/status.rs:60`
- **Bug**: Spec `i[status.get]` defines `active_operations` as "number of lifecycle operations currently in progress", but the handler emits the literal `0` regardless of scheduler state (`state.scheduler` is available and used by other handlers).
- **Failure scenario**: An install or action operation is running; `/server/status` reports `active_operations: 0`, misleading operators and automation that poll status before maintenance.
- **Severity**: medium — always-wrong field in a common status call, though describe/list expose the operation elsewhere.
- **Fix testability**: easy — TestOi: occupy the scheduler, call `/server/status`, assert count = 1 (plus queued).
- **Confidence**: certain

#### `effective_app_status` reports Degraded for apps whose stopped resources are at their desired state (and for kinds that can never derive Ready)

- **Location**: `crates/core/src/oi/handler/apps.rs:456-513` (the `all_ready` loop at 479-503)
- **Bug**: The status refinement requires every resource group in `def.resources` to have at least one `Ready` instance (`has_ready`). It ignores the stopped set, even though a stopped resource's desired state is `Unscheduled` (`desired.rs` `compute_steady`, annotated `i[impl resource.stop]`) — spec `i[app.status]` says Running means "all resources are at their desired lifecycle states" and explicitly excludes torn-down instances from Degraded. Separately, resources of kind `ExternalService` land in `def.resources` (`defs/app/service.rs:71-96`) and get instances (`reconcile/routes.rs:106`), but `derive_lifecycle_with_ms` (`oracle.rs:365-374`) has no arm for `ExternalService` and derives `Pending` forever despite `backend_healthy` observations being recorded — so the loop's `return false` fires.
- **Failure scenario**: (a) Operator stops a deployment via `/apps/resource/stop`; all its instances reach `Unscheduled` (their desired state); the app shows `degraded` permanently until unstopped. (b) Any app declaring `app.external_service(...)` never reports `running`, even with healthy mapped backends. (The ExternalService leg's root cause is the missing oracle arm in `runtime/barrier/oracle.rs`, outside this scope, but `effective_app_status` is where the wrong status is produced.)
- **Severity**: medium — permanently wrong app status in legitimate configurations; misleads monitoring.
- **Fix testability**: moderate — TestOi/stub DB: seed instances with `container_removed` observations for a stopped resource and assert `running`; unit-test the loop with a stopped set.
- **Confidence**: likely (stopped leg), likely (external-service leg)

#### `set_param`/`unset_param` persist the change, then return an error and skip the event when a concurrent operation appears

- **Location**: `crates/core/src/oi/handler/params.rs:322` and `params.rs:412` (with the gate at `params.rs:67-85` and persist at `params.rs:279-315`)
- **Bug**: The operation-in-progress gate runs at the top of the handler, but the cron schedule ticker (`runtime/schedules.rs:96`) can grab the scheduler slot for the same app between the gate and `schedule_on_change`. `schedule_on_change` then returns `Rejected(...)` which propagates as an `operation_in_progress` error — *after* the param value has been durably written, the generation bumped, and the def reloaded. The `param_change` event (emitted only after `schedule_on_change`) is also skipped, and the `on_change` handler never fires for that change.
- **Failure scenario**: Cron schedule for app `x` fires while an operator's `/apps/params/set` for `x` is between the gate and scheduling. Operator receives `operation_in_progress` and believes the set failed; in fact the value changed durably (visible in `/apps/show` and generation history), event-feed consumers never see the change, and the registered `on_change` handler is never invoked for it. The now-running operation also sees the param mutate mid-flight — the exact hazard the gate exists to prevent.
- **Severity**: medium — race window is small but real on systems with scheduled actions; leaves committed state with a failure response and a lost event/handler.
- **Fix testability**: moderate — unit-level: occupy the scheduler after the gate (call `schedule_on_change` directly with a busy scheduler) and assert the handler either rolls back or reports success-without-schedule.
- **Confidence**: likely

#### `/apps/show` fabricates an empty `current_operation` for Uninstalling apps

- **Location**: `crates/core/src/oi/handler/apps.rs:827-893`
- **Bug**: The `current_operation` block runs for `Uninstalling` on the claim that uninstall "runs an action closure with the same observable surface", but uninstall is reconciler-driven with no scheduler operation (`reconcile.rs:1468-1471`: "Uninstall is reconciler-driven and therefore emits no OperationCompleted event"). `scheduler.active().filter(|a| a.app == name)` is always `None`, so the `unwrap_or_else` fallback emits `current_operation` with `action_name: ""` (the `ActionName::default()` placeholder that per `names.rs:211-214` "must be overwritten before anything inspects it"), `source_generation: 0`, `target_generation: 0`, `barrier: null`.
- **Failure scenario**: Describe any app during uninstall → response contains a bogus operation object with an empty action name and zeroed generations; clients render a phantom operation. The same fallback also fires if another app's operation is active while this app is Operating-adjacent.
- **Severity**: low — misleading output only, during every uninstall.
- **Fix testability**: easy — TestOi: install, uninstall, `/apps/show`, assert no `current_operation` (or meaningful contents).
- **Confidence**: certain

#### `/apps/list` always reports an empty `action_name` for Operating apps

- **Location**: `crates/core/src/oi/handler/apps.rs:443-445`
- **Bug**: `derive_status` (`runtime/apps.rs:354-357`) constructs `AppStatus::Operating { action_name: ActionName::default() }` — an empty placeholder the type docs say must be overwritten before inspection. `list_apps` inspects it directly and emits `"action_name": ""` without consulting `scheduler.active()` for the real action name (as `describe_app` does).
- **Failure scenario**: Any action running → `/apps/list` shows `status: "operating", action_name: ""` for that app; UIs cannot show which action is running from the list view.
- **Severity**: low — spec `i[app.status]` says Operating "includes the field action_name"; the value is always wrong but only informational.
- **Fix testability**: easy — TestOi: start an operation, call `/apps/list`, assert `action_name` equals the action.
- **Confidence**: certain

#### `register_app` leaves the app registered in memory when DB persistence fails

- **Location**: `crates/core/src/oi/handler/apps.rs:1228-1293`
- **Bug**: The app is added to the in-memory registry first; if the subsequent `persist_app_fields` or `bump_register` DB call fails, the handler returns an error without removing the in-memory entry. The client sees a failure, but the app is half-registered.
- **Failure scenario**: DB write fails (disk full, I/O error) during `/apps/create` → error returned, yet `/apps/list` shows the app; a retry of `/apps/create` is rejected with "app already registered" even though nothing durable exists (a daemon restart silently drops the app, since `load_from_db` skips rows with generation 0). The operator must `/apps/remove` a "phantom" app to retry.
- **Severity**: low — requires a DB failure, but the check-retry contract is broken and state diverges from disk.
- **Fix testability**: moderate — needs a fault-injecting DB handle to make `bump_register` fail, then assert the registry no longer contains the app.
- **Confidence**: likely


### 5. OI handlers: tls, services, ingresses, images, registries, key_mgmt

#### CSR cert upload never checks SAN coverage of the target hostname
- **Location**: `crates/core/src/oi/handler/tls.rs:556-593` (`csr_upload_cert`)
- **Bug**: `validate::validate_upload` only checks SAN-list-non-empty, expiry, and SPKI match — it takes no hostname. The code comment ("SAN coverage for the originally-requested hostname is enforced as part of the SPKI match") is wrong: an SPKI match proves the cert was issued for the CSR's keypair, not that its SAN set still contains the requested hostname. Spec `r[tls.cert.validation.san-coverage]` and `r[tls.csr.flow]` (docs/spec/runtime.md:1490, 1516-1518) explicitly require the SAN-coverage check on CSR cert upload and require failing uploads to "not alter any existing policy or certificate". A `parse::san_covers(&validated.parsed.san_dns_names, &cert_row.hostname)` check is missing.
- **Failure scenario**: Operator runs `csr/begin` for `www.example.com`; the CA rewrites/renames the SAN set when signing (e.g. issues for `example.com` only, or the operator's CA workflow substitutes names). Upload is accepted, the row goes `active` with `hostname = www.example.com`, and `supersede_other_active_for_hostname` retires the previously-serving valid cert. `find_active_for_hostname`'s exact-match fast path (store.rs:356-370) then serves the non-covering cert for `www.example.com` → TLS name-mismatch outage, and the good cert is gone.
- **Severity**: medium — needs a CA that alters the SAN set (edge case), but the outcome is an outage plus loss of the previously active cert, and it is a direct spec violation.
- **Fix testability**: moderate — TestOi/in-memory DB: begin a CSR, build a cert carrying the CSR's public key but a different SAN, upload, assert `requirements_invalid` and prior cert untouched.
- **Confidence**: certain (check is absent and spec requires it); impact scenario likely. Independently found by the runtime-TLS auditor as well.

#### Uploading a not-yet-valid cert immediately supersedes the currently-serving cert
- **Location**: `crates/core/src/oi/handler/tls.rs:335-359` (`upload_manual`; same pattern at 574-587 in `csr_upload_cert`)
- **Bug**: A cert whose `notBefore` is in the future is accepted with only a `not_yet_valid` warning (validate.rs documents this as "so the operator can stage uploads ahead of cutover"), yet the handler inserts it as `Active` and supersedes the prior active cert for the same primary SAN. Serving (`serve.rs::lookup` → `find_active_for_hostname`, newest active wins, no validity-window filter) immediately hands the not-yet-valid cert to Caddy.
- **Failure scenario**: Hostname `shop.example.com` is serving a valid manual cert. Operator "stages" next month's cert (notBefore = +7 days) via `/tls/certificates/upload-manual`. The old cert is marked superseded, the future-dated cert is served at the next handshake → every client rejects the handshake until `notBefore` passes. The stated staging use-case is exactly the input that causes the outage.
- **Failure scenario**: as above; also reproducible through the CSR upload path.
- **Severity**: medium — edge-case input, but produces an immediate outage on a serving hostname and defeats the documented purpose of accepting such certs.
- **Fix testability**: moderate — TestOi + in-memory DB: upload valid cert, upload future-dated cert, assert which cert `serve::lookup` returns and prior cert's state.
- **Confidence**: likely (behaviour is certain; a maintainer could argue immediate-activation is intended, but it contradicts the staging rationale written next to the acceptance rule).

#### CSR upload/cancel check-then-act races can supersede or delete the wrong cert
- **Location**: `crates/core/src/oi/handler/tls.rs:530-593` (`csr_upload_cert`), `tls.rs:601-622` (`csr_cancel`)
- **Bug**: Both handlers read the row state in one `db.call` and mutate in a later `db.call`; OI requests run concurrently (one task per QUIC stream) and only individual closures are serialised on the DB thread. Additionally, `csr_upload_cert`'s mutating closure runs `update_certificate` (which doesn't report 0-rows-matched) and then unconditionally `supersede_other_active_for_hostname`, without a transaction or a re-check of state.
- **Failure scenario**: (a) `csr_cancel` passes its `csr_pending` check, a concurrent `csr_upload_cert` completes and activates the row, then cancel's second `db.call` deletes the now-active serving cert — exactly what the cancel guard is documented to prevent. (b) Cancel deletes the row between upload's read and its mutation: `update_certificate` matches 0 rows, but `supersede_other_active_for_hostname` still retires the hostname's existing active cert, and the handler returns success — hostname left with no active cert.
- **Severity**: low — requires two operator requests racing on the same CSR id; consequence (lost active cert) is serious but the window is small.
- **Fix testability**: moderate — fix is a single transaction with a state-conditioned `UPDATE ... WHERE state='csr_pending'`; unit-testable against the in-memory DB by simulating the interleaving (delete row, then run the mutation closure, assert no supersede).
- **Confidence**: certain (interleaving is possible by construction), likely as a practical defect.

#### Adding a registry to the allowlist leaves stale `disallowed_registry` faults
- **Location**: `crates/core/src/oi/handler/registries.rs:27-34` (`add_registry`) vs `registries.rs:37-53` (`remove_registry`)
- **Bug**: `remove_registry` re-evaluates all apps and re-syncs `disallowed_registry` faults; `add_registry` does not. Fault sync only ever runs on app create/update (`apps.rs:207`) and registry removal — nothing re-checks after an add, and no reconciler tick re-syncs these faults (`sync_registry_faults` has no production callers). The natural remediation path for the fault therefore doesn't clear the fault.
- **Failure scenario**: Operator registers an app using `quay.io/...` → `disallowed_registry` fault filed. Operator fixes it the obvious way: `/registries/add {"registry":"quay.io"}`. The fault remains indefinitely (until some unrelated app update or registry removal), telling the operator the registry is still disallowed even though the check would now pass.
- **Severity**: medium — wrong operator-visible state in the primary remediation flow for this fault; spec (interface.md `i[registry.remove]`) only mandates re-evaluation on remove, but language.md says the fault "is cleared when the app is re-evaluated and all image registries pass", and add never triggers that re-evaluation.
- **Fix testability**: easy — TestOi: create app with quay.io image, assert fault, call `/registries/add quay.io`, assert fault cleared (currently fails).
- **Confidence**: likely (behaviour certain; small chance the asymmetry is deliberate, but nothing in spec/tests asserts the stale-fault behaviour).

#### Revoking a client key does not affect its already-open connections
- **Location**: `crates/core/src/oi/handler/key_mgmt.rs:57-73` (`revoke_key`), enforcement in `crates/core/src/oi/auth.rs:213-231`
- **Bug**: The trusted-key set is consulted only in `verify_client_cert` at TLS handshake time. `revoke_key` removes the fingerprint from the DB and the in-memory set but nothing closes or re-validates existing QUIC connections, which are long-lived (event streams, shells, forwards).
- **Failure scenario**: Operator revokes a lost/compromised laptop's key via `/keys/revoke`. A session already connected with that key keeps full OI access (including re-authorising keys) until it disconnects on its own — revocation silently does not achieve its purpose.
- **Severity**: medium — security-relevant gap in the one operation whose purpose is removing access; only matters when the revoked key has a live connection, but that is precisely the compromise scenario.
- **Fix testability**: hard — needs the real QUIC endpoint (the in-repo `spawn_oi_server` test harness in `oi/server.rs` could be extended: connect, revoke, assert subsequent requests on the old connection fail).
- **Confidence**: certain about the behaviour; likely as a defect (spec is silent on active-connection semantics).

#### Hostname rollup labels Tailscale issuances as `acme_dns`
- **Location**: `crates/core/src/oi/handler/tls.rs:924-936` (`build_last_issuance`)
- **Bug**: The `last_success` branch hardcodes `"kind": "acme_dns"`. Tailscale issuance also logs attempts (`issuance.rs::run_tailscale` inserts/finalises attempt rows), so a Tailscale-policy hostname with a successful fetch reports its last issuance as `acme_dns` with `provider: null`.
- **Failure scenario**: Tailscale-discovered ingress `host.tailnet.ts.net` gets a cert from tailscaled; `/tls/hostnames/list` shows `last_issuance.kind = "acme_dns"`, contradicting the `tailscale` policy/origin shown alongside it.
- **Severity**: low — display-only mislabel in the operator rollup; no behavioural consequence.
- **Fix testability**: moderate — needs a managed ingress in the registry plus an attempt row (stubbed OiState/test_support), then assert the rollup JSON.
- **Confidence**: certain.

#### Unknown DNS provider in set-acme-dns surfaces as `not_found: db error: FOREIGN KEY constraint failed`
- **Location**: `crates/core/src/oi/handler/tls.rs:170-193` (`set_policy_acme_dns`), `tls.rs:1102-1104` (`db_error`)
- **Bug**: The handler never validates that `dns_provider` exists; the `tls_policies.dns_provider → tls_dns_providers` FK rejects the insert and `db_error` maps every DB error to `ErrorCode::NotFound` with a raw "db error: FOREIGN KEY constraint failed" message. The delete paths in the same file special-case FK errors into clear `requirements_invalid` messages; this path doesn't. The handler also accepts an empty/garbage `hostname` pattern, silently creating a policy row that can never match.
- **Failure scenario**: Operator typos `/tls/policies/set-acme-dns { hostname: "a.example.com", dns_provider: "aws-prd" }` → gets `not_found: db error: FOREIGN KEY constraint failed` with no mention of the provider; a CLI/web client keying off the error code misreports the failure.
- **Severity**: low — the invalid input is rejected, but with the wrong error code and an unactionable message, inconsistent with the file's own FK handling elsewhere.
- **Fix testability**: easy — TestOi: set policy with unknown provider, assert `requirements_invalid` and a message naming the provider.
- **Confidence**: certain.

#### Key authorisation accepts fingerprints that can never authenticate
- **Location**: `crates/core/src/oi/handler/key_mgmt.rs:44-54` (`authorize_key`)
- **Bug**: Fingerprints are 64-char lowercase-hex SHA-256 of the client SPKI (`keys::fingerprint`), and the verifier compares byte-for-byte (`auth.rs:222`). The handler performs no validation or normalisation, so uppercase hex, surrounding whitespace, `sha256:`-prefixed, or wrong-length strings are stored and listed as authorised but will never match a real client.
- **Failure scenario**: Operator pastes `SHA256:AB12...` or uppercase hex from another tool into `/keys/authorise`; the call succeeds and `/keys/list` shows the key, but the client is rejected at every handshake with no hint why.
- **Severity**: low — silent misconfiguration trap; no security impact, but a confusing lockout in a first-time-setup flow.
- **Fix testability**: easy — pure unit test on the handler: reject/normalise non-canonical fingerprints.
- **Confidence**: certain about behaviour; likely as a defect (tests use short lowercase strings for convenience, but nothing asserts acceptance of non-canonical forms).

#### Port 0 accepted for site-service endpoints and ingress attachments
- **Location**: `crates/core/src/oi/handler/services.rs:76-93` (`EndpointParams`), `crates/core/src/oi/handler/ingresses.rs:401-435` (attachment params)
- **Bug**: `service_port`, `remote_port`, and attachment `port` are plain `u16` with no range check, so 0 is accepted. Port 0 is not a routable listener/backend port; it flows into the site-proxy config (`listen :0` semantics / dial `:0`) instead of being rejected at the interface.
- **Failure scenario**: `/ingresses/site/attach/forward { port: 0, protocol: "http", ... }` succeeds; the reconciler renders a proxy listener for port 0, yielding an arbitrary-port listener or proxy config error rather than a `requirements_invalid` at the API.
- **Severity**: low — requires nonsensical operator input; consequence is confusing downstream failure instead of upfront rejection.
- **Fix testability**: easy — TestOi calls with port 0, assert `requirements_invalid`.
- **Confidence**: certain that 0 is accepted; possible on downstream impact (depends on how the proxy renders it).


### 6. Backups, volumes, scheduling

#### Later volume success erases earlier volume's backup failure faults
- **Location**: `crates/core/src/oi/handler/backups.rs:573-579`
- **Bug**: On a successful `save-snapshot` for one volume, `run_volume_backup` clears **all** active `backup_failed` and `backup_source_unavailable` faults for the backing backup app. Faults are keyed only by (app, kind), so a success for volume B wipes the fault just filed for volume A in the same strategy run (volumes are processed serially in declaration order), and also wipes faults from other strategies sharing the same backup app.
- **Failure scenario**: Strategy `nightly` has volumes `["broken/data", "ok/data"]`. `broken/data` fails both attempts → `backup_failed` fault filed (backups.rs:603-613). Seconds later `ok/data` succeeds → `clear_faults_by_kind(db, backup_app, "backup_failed")` clears it. The operator never sees any active fault even though `broken/data` is never backed up; whether a persistent failure is visible depends entirely on volume declaration order.
- **Severity**: high — silently masks permanent backup failures in the documented multi-volume use case; for a deployment relying on backups this is failure-invisibility for critical data. (Note: the spec's `r[backup.execution]` "On success, any existing backup_failed... faults for the backup app are cleared" is implemented literally, but the order-dependent same-run masking it produces cannot be the intent of a per-volume fault mechanism.)
- **Fix testability**: moderate — TestOi/stub harness with a two-volume strategy where the first volume's path is missing and the second succeeds; assert the fault survives.
- **Confidence**: likely (behaviour itself is certain; classified as bug vs spec wording)

#### Manual backup retry sleeps a random scheduled-run delay (up to 2.4 h)
- **Location**: `crates/core/src/oi/handler/backups.rs:515-518`
- **Bug**: The snapshot-creation retry path sleeps `random_delay_secs(&strategy.schedule)` unconditionally, without the `is_manual` check that the save-snapshot retry path has (backups.rs:591-594). Spec `r[backup.schedule.delay]` and `i[backup.run]` both state manual `/backups/run` applies no delay.
- **Failure scenario**: Operator runs `/backups/run` on an "every day" strategy; the first `snapshot_site` call fails transiently (e.g. btrfs momentarily busy). The retry task silently sleeps up to 8640 s before retrying; the manual backup appears hung for hours.
- **Severity**: medium — wrong behaviour only on the snapshot-failure retry path, but directly contradicts the spec'd no-delay contract for manual runs.
- **Fix testability**: moderate — needs a stubbed volume store whose `snapshot_site` fails once; assert retry happens immediately for `is_manual == true` (or unit-extract the delay decision).
- **Confidence**: certain

#### Snapshot/promote to an existing site-volume name nests a subvolume inside a live volume
- **Location**: `crates/core/src/oi/handler/volumes.rs:500-548` (snapshot) and `crates/core/src/oi/handler/volumes.rs:559-633` (promote)
- **Bug**: Neither handler checks whether the target `name` already exists (in `site_volumes` or on disk) before invoking `btrfs subvolume snapshot [-r] src dest`. When `dest` (`site-<name>`) already exists as a directory, btrfs creates the snapshot **inside** it (`dest/<src-basename>`) and reports success; the subsequent DB insert then fails on the `name` PRIMARY KEY, leaving a (read-only, for snapshot) subvolume nested inside an unrelated live site volume. `restore_held` demonstrates the intended pattern — it rejects target-name collisions both in DB and on disk (`volume_store.rs:261-267`).
- **Failure scenario**: Operator runs `/volumes/site/snapshot { name: "data", source: "app/vol" }` where managed site volume `data` already exists → request errors with "failed to store snapshot site volume", but `site-data/site-<src>` now contains a read-only nested subvolume polluting the live volume and blocking its later deletion (`remove_dir_all`/subvolume delete fails on nested read-only subvolumes).
- **Severity**: medium — edge-case trigger (name reuse) but corrupts an unrelated live volume and leaves it undeletable.
- **Fix testability**: hard for the btrfs nesting itself (needs real btrfs); easy for the fix (existence pre-check testable via TestOi: snapshot onto an existing name must be rejected with `requirements_invalid`).
- **Confidence**: likely (btrfs nesting semantics are standard; not executed here)

#### Operator site volumes named `backup-snap-*` are destroyed by startup cleanup
- **Location**: `crates/core/src/runtime/backup_execution.rs:14` (prefix contract) with `crates/core/src/oi/handler/volumes.rs:260-337` (`create_site_volume`) and `volumes.rs:83-181` (`restore_held`)
- **Bug**: The startup orphan cleanup deletes every on-disk site volume whose name starts with `SNAPSHOT_NAME_PREFIX` (`"backup-snap-"`), assuming the prefix uniquely identifies backup-execution temporaries. But nothing reserves that prefix: `create_site_volume` and `restore_held` accept any valid `SiteVolumeName`, including `backup-snap-foo`, and register it in `site_volumes`. The cleanup does not cross-check the DB.
- **Failure scenario**: Operator creates managed site volume `backup-snap-archive` and fills it with data. On the next daemon restart, the cleanup removes its backing storage while the DB row remains — silent data loss, plus a phantom registry entry (which is then also undeletable, see next finding).
- **Severity**: medium — requires an unlucky name choice, but the consequence is unrecoverable data loss that bypasses the held-volume safety mechanism entirely.
- **Fix testability**: easy — reject the reserved prefix in `create_site_volume`/`restore_held` (pure handler test via TestOi), or have cleanup skip DB-registered names (stubbed db test).
- **Confidence**: certain

#### Managed site volume with missing backing storage can never be deleted
- **Location**: `crates/core/src/oi/handler/volumes.rs:417-434`
- **Bug**: `delete_site_volume` for `Managed` kind requires `hold_site` to succeed before removing the DB row, and `hold_inner` errors `NotFound` when the on-disk path is missing (`volume_store.rs:158-163`). The handler propagates the error before reaching the DB delete, so a row whose directory is gone (manual removal, the `backup-snap-` cleanup above, disk recovery) is permanently stuck: every delete attempt fails with an internal error.
- **Failure scenario**: `site-mydata` directory is lost; `/volumes/site/delete { name: "mydata" }` → "failed to hold site volume for review: volume site-mydata does not exist". The stale row remains in listings forever with no API path to remove it.
- **Severity**: low — edge-case state, no data at risk, but leaves an unremovable registry entry.
- **Fix testability**: easy — TestOi: create managed volume, delete its backing dir on disk, call delete; assert the row is removed (treat missing storage as nothing-to-hold).
- **Confidence**: certain

#### Queued schedule fire is stamped as fired but lost on daemon restart
- **Location**: `crates/core/src/runtime/schedules.rs:105-121`
- **Bug**: `check_due_schedules` updates `last_fired_at = now` for both `Accepted` and `Queued` results. A `Queued` fire only exists in the in-memory scheduler queue (`scheduler.rs:220-229`); if the daemon restarts before the queued operation is promoted and run, the fire is recorded in the DB as having happened but never executes, and the catch-up logic (which keys off `last_fired_at`) will not re-fire it until the next cron boundary.
- **Failure scenario**: Daily `backup` action for app A becomes due while app B has a long operation active → A's fire is queued and `last_fired_at` stamped. Daemon restarts (deploy, crash) before B finishes → A's scheduled run for that day silently never happens, defeating the `r[schedule.catch-up]` guarantee for a window the daemon was actually up for.
- **Severity**: low — needs a restart inside the queued window; consequence is one silently skipped scheduled run.
- **Fix testability**: easy — unit test against `check_due_schedules` + a fresh `Scheduler`: fire into a busy scheduler (Queued), rebuild the scheduler (simulating restart), assert the schedule re-fires (currently it does not).
- **Confidence**: likely (loss mechanism is certain; stamping-on-Queued is deliberate anti-double-fire, so the restart gap is the unhandled part)


### 7. Runtime persistence: db, generations, history, audit, faults, gc

#### `save_current_operation` silently resets a persisted cancel request
- **Location**: `crates/core/src/runtime/history.rs:495-522` (with `set_cancel_requested` at `history.rs:625`)
- **Bug**: `save_current_operation` uses `INSERT OR REPLACE` without listing `cancel_requested`, so the replaced row reverts to the column default `0`. On replay after a crash, `daemon/main.rs:1460` reads the persisted cancel flag and then `run_lifecycle_operation` (`oi/handler/actions/lifecycle.rs:346`) re-saves the row — wiping the durable flag while the cancelled operation is still in flight. Additionally, a cancel issued in the window between scheduler acceptance and the first `save_current_operation` persists nothing (`set_cancel_requested` matches no row; its `false` return is ignored by `cancel_action`).
- **Failure scenario**: operator cancels op → daemon crashes → restart replays into pre-cancelled state but re-save resets `cancel_requested=0` → daemon crashes again before the cancel unwind clears the row → second replay re-executes the whole operation the operator cancelled, violating `r[operation.cancel.persistence]`.
- **Severity**: medium — durable cancel state is destroyed in a common code path; observable harm needs a double-crash or a tight race, so edge-case frequency.
- **Fix testability**: easy — in-memory `Db`: save op, `set_cancel_requested`, save same op again, assert `load_cancel_requested` is still true.
- **Confidence**: certain (flag reset), likely (harm scenario)

#### `gc_unscheduled_instances` ignores the spec's "never delete active desired state instances" exclusion
- **Location**: `crates/core/src/runtime/gc.rs:128-198`
- **Bug**: `r[gc.instances]` (docs/spec/runtime.md:380) requires that instances in the active desired state (keep-set members / singletons) "must never be deleted regardless of their lifecycle state". The GC selects victims purely by "latest observation is a terminal kind and older than 10 min" with no desired-state check, and its transaction also deletes all `faults` rows for the instance — including active inhibitor faults.
- **Failure scenario**: a crash-looping deployment: reconciler files `crash_loop` against the instance and (per `r[fault.crash-loop]`) must not auto-restart while the fault is active; the container is gone so the newest observation can be a terminal kind (`container_removed`). Because observations are deduped per `(instance, kind)` no fresher row appears; after 10 minutes GC deletes the instance row *and its active `crash_loop` fault*, erasing the operator-visible fault, re-enabling auto-restart, and minting a new instance ID — the loop then repeats indefinitely with flapping faults and identity churn.
- **Severity**: medium — undoes a spec-mandated safety inhibition and destroys active fault records, but only in the stuck/faulted edge cases where a desired-active instance sits on a stale terminal observation.
- **Fix testability**: easy for the fault-erasure/selection logic (in-memory `Db` + backdated observations, as existing `gc.rs` tests do); moderate to exercise the desired-state exclusion end to end (needs the stub System reconciler harness).
- **Confidence**: likely

#### Orphaned and never-completed `autonomous_operations` rows leak forever
- **Location**: `crates/core/src/runtime/history.rs:126-137` (`delete_instance`) and `crates/core/src/runtime/gc.rs:111-117` (`gc_completed_operations`)
- **Bug**: `delete_instance` (and the GC instance sweep) removes `world_observations`, `faults`, and the registry row but never `autonomous_operations` rows keyed by the same `instance_id`; there is also no orphan sweep for that table (unlike `gc_orphaned_observations`). Rows with `completed_at IS NULL` (crash between `insert_autonomous_operation` and `complete_autonomous_operation`) are excluded from `gc_completed_operations` and are therefore never deleted by anything.
- **Failure scenario**: daemon crashes mid-autonomous-op, or an instance with pending autonomous ops is retired → rows referencing a dead `instance_id` (or with NULL `completed_at`) accumulate unboundedly across the deployment's lifetime.
- **Severity**: low — storage/orphan leak only; no functional misbehaviour since queries are keyed by live instance ids.
- **Fix testability**: easy — in-memory `Db`: insert op without completing, delete instance, run GC, assert row count.
- **Confidence**: certain

#### `parse_resource_kind` cannot parse `ExternalService` rows that the runtime persists
- **Location**: `crates/core/src/runtime/history.rs:668-685`
- **Bug**: the reconciler persists `resource_instances` rows with `kind = "ExternalService"` (`system/reconcile/rules.rs:260`, `system/reconcile/routes.rs:108`, asserted by `tests/service.rs:163`), but `parse_resource_kind` — used by `find_instance` — has no `"ExternalService"` arm, so `find_instance` on such a row returns `Err(FromSqlConversionFailure)` instead of the instance. `stopped.rs`'s own kind mapping handles `ExternalService`, showing the omission is an oversight.
- **Failure scenario**: any caller of the public `find_instance` API given an external-service instance id gets a hard DB-conversion error. Currently only tests call `find_instance`, so the defect is latent, but the data guaranteeing the failure already exists in production databases.
- **Severity**: low — real code/data mismatch, but no production call path hits it today.
- **Fix testability**: easy — insert an ExternalService instance, `find_instance`, assert `Ok(Some(_))`.
- **Confidence**: certain (mismatch), latent impact

#### Generation bumps are not atomic across `generations` insert and `current_generation` update
- **Location**: `crates/core/src/runtime/generations.rs:197-215, 245-334` (`insert_register_or_update`, `bump_param_set`, `bump_param_unset`)
- **Bug**: each bump executes an `INSERT INTO generations` followed by a separate autocommit `UPDATE registered_apps SET current_generation` with no transaction (contrast `delete_instance` and `get_or_create_singleton`, which do wrap multi-statement sequences). A crash or I/O error between the two statements leaves `registered_apps.current_generation` pointing at generation N while generation N+1 exists; the callers' surrounding statements (param upsert in `oi/handler/params.rs:281-314`) are likewise uncommitted as a unit, so a param value can change with no matching history entry.
- **Failure scenario**: power loss between the two statements of `bump_param_set` → `generations::current()` reports the old generation forever (the orphan generation is never "current"), reconstruction/`script_hash_at` and the events/audit stream disagree about the app's current definition.
- **Severity**: low — requires a crash at an exact instant; the DB actor thread removes all concurrency interleavings.
- **Fix testability**: easy to fix (wrap in `unchecked_transaction`); hard to unit-test the crash itself, but a test can assert the statements run inside one transaction via a failure injection on the second statement.
- **Confidence**: certain (non-atomicity), possible (impact)

#### `apply_schema` silently discards non-string submitted param values
- **Location**: `crates/core/src/runtime/action_params.rs:96-119` (with `run_requirements` at 185-232)
- **Bug**: `submitted` is built only from values where `v.as_str()` succeeds, so a schema-declared param supplied as a JSON number/bool is treated as *absent*: if the field has a default, the submitted value is silently overwritten with the default (`params.insert` at line 116); if required with no default, the caller gets a spurious "required field is missing" even though a value was provided. The language spec (`l[action.params]`) says param is "an arbitrary key-value map".
- **Failure scenario**: operator invokes an action with `{"replicas": 3}` where `replicas` is a schema param with default `"1"` → the closure runs with `replicas = "1"`, silently ignoring the supplied 3. Both the OI path (`oi/handler/actions.rs:201`) and script `Action.call` path (`barrier/action_call.rs:104`) are affected.
- **Severity**: low — schema params are string-kinded by design, so non-string submissions are unusual, but the silent substitution (rather than a type error) is wrong when it happens.
- **Fix testability**: easy — pure unit test on `apply_schema` with a `Value::Number` input.
- **Confidence**: likely

#### Audit-lag faults are filed without dedup and are never cleared
- **Location**: `crates/core/src/runtime/audit.rs:94-109`
- **Bug**: every `RecvError::Lagged` files a fresh `audit_lag` fault against the pseudo-app `seedling`. Unlike the reconciler's fault-filing paths (which check `already_filed`, e.g. `system/reconcile/faults.rs:318-327`), there is no dedup, and no code path ever clears `audit_lag` faults, so they stay active forever (GC only prunes *cleared* faults).
- **Failure scenario**: a sustained event burst causes repeated lag → dozens of permanently-active duplicate faults accumulate; `count_active_faults` and the operator fault list are polluted until each is manually cleared.
- **Severity**: low — noise/unbounded active-fault growth under sustained lag; no state corruption.
- **Fix testability**: easy — file via the same helper twice and assert a single active `audit_lag` fault once dedup is added.
- **Confidence**: certain (mechanism), likely (operational impact)


### 8. Runtime barrier/orchestration: barrier, replay, oracle, probe, scaling

#### `rt.stop`/`rt.start`/`rt.query` on a named Deployment ignore the operator's scaling decision
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:487`
- **Bug**: `extract_instances` resolves a named Deployment via `ensure_scaled_group(..., dep.def.lock().scale.start.max(1))`, i.e. the declared lower bound — not the effective scale from `scaling_decisions` (which is what the steady-state reconciler uses, `reconcile.rs::compute_effective_scales` → `scaling::effective_scale`). The excess instances returned by `ensure_scaled_group` are discarded. The comment on this block claims "an action that needs to refer to all of them — rt.signal, rt.stop, rt.query — gets every replica back from this single call", which is only true when no scaling decision raised the count; `do_signal` was given a `find_all_instances` expansion to work around exactly this, but `do_stop`/`do_start`/`do_query` were not.
- **Failure scenario**: Deployment `web` has `scale(1..5)`; operator scales it to 3 (persisted decision). An action runs `rt.stop(app.deployment("web"))` (e.g. stop DB → snapshot volume → restart). Only 1 of 3 replicas is put into the operation's desired state as `Unscheduled` (`compute_during_operation` only covers listed instances; nothing else touches the other replicas), the stop barrier waits on that 1 instance only, and the script proceeds believing the deployment is down while 2 replicas are still running — e.g. writing to a volume that live containers are still using.
- **Severity**: high — silent partial stop of a scaled deployment during maintenance actions is a data-corruption-class hazard, and scaling above the lower bound is a first-class feature (`r[scaling.decision]`).
- **Fix testability**: moderate — needs a DB-backed registry + scaling decision + `run_operation` with `TestWorldOracle`, asserting the Stop entry's resource count.
- **Confidence**: certain

#### Second and later barriers on the same `Started` never record `started_at`, so their deadlines are never enforced
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:2071-2101` (`Started::check_barrier` attach/synthesis logic)
- **Bug**: An `ActionLogEntry` can hold only one `BarrierRecord`. When a chained barrier (`rt.start(x).running().ready()`) suspends after an earlier barrier already occupied the entry's barrier slot and was committed, the later barrier finds no pending entry with `barrier.is_none()`, and the `already_tracked` guard (correctly) suppresses the synthetic entry — so no record with `started_at` is ever created or persisted for the later barrier. Its deadline check at line 2038 then always sees `started_at == None` and never fires.
- **Failure scenario**: `let s = rt.start(dep); s.running(); s.ready(30);`. Pass 1: `.running()` attaches its barrier to the Start entry and suspends; the record commits. Later pass: `.running()` passes via the oracle, `.ready(30)` is unsatisfied → no barrier record anywhere carries `required_state=Ready`, so `started_at` is `None` on every subsequent pass. If the container never becomes ready, the operation suspends forever instead of failing after 30s — violating `r[barrier.deadline]` ("the barrier must throw"). The operation is unkillable except by manual cancel.
- **Severity**: high — chained state methods are the documented API shape (`l[rt.started.state-methods]`), and the deadline is the only thing that turns a wedged resource into an operation failure.
- **Fix testability**: easy/moderate — pure `run_operation` + `InMemoryActionLog` + injected `now_secs`: suspend on `.running()`, satisfy running, advance clock past the `.ready()` deadline, assert `Failed` (currently stays `Suspended` forever).
- **Confidence**: certain

#### `rt.signal` replay dedup is value-based, not positional — duplicate calls are swallowed and changed instance sets re-deliver
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:1067-1080` (`do_signal`)
- **Bug**: Replay skipping matches "any committed entry with the same `(resources, signal)`" instead of matching the entry at the current `call_index` like `do_exec`/`do_write` do. Two consequences: (a) a second, legitimately distinct `rt.signal` call with the same target and signal later in the closure is treated as already delivered once the first one is committed — and is skipped even when the runtime is no longer replaying (`already && !is_replaying()` returns `Ok` without sending or logging); (b) if the expanded instance set differs across a restart (replica replaced by healthcheck-replace, scale changed — `expanded` is recomputed live via `find_all_instances`), the committed entry no longer matches, so a replay re-sends the signal, violating the at-most-once guarantee of `l[rt.signal]`/`r[rt.signal]`, and its pending entry overwrites the original log row at that index.
- **Failure scenario**: `rt.signal(db, "SIGHUP"); rt.start(job).terminated().ensure_success(); rt.signal(db, "SIGHUP");` — the job barrier suspends, the first signal commits; on the pass where the barrier passes, the second `rt.signal` finds `already == true`, `is_replaying() == false`, and silently returns: the reload-after-migration signal is never delivered.
- **Severity**: high — sequential "signal, wait, signal again" is a natural pattern (config reload before and after a step) and the second delivery is silently lost.
- **Fix testability**: easy — `run_operation` with a recording stub `ContainerSignaler`, two identical signal calls with a suspending barrier between; assert two deliveries across passes.
- **Confidence**: certain

#### `Started` collection methods (`one`/`only`/`except`/`select`) are silent no-ops
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:2228-2231`
- **Bug**: The spec (`l[rt.started.type]`) says `Started` implements the Collection interface with methods returning `Started`s "corresponding to the resources". The implementation ignores the arguments entirely and returns `this.clone()` — no filtering — while carrying the `l[impl rt.started.type]` annotation as if complete (and no `todo!()` marker despite the repo rule for stubs).
- **Failure scenario**: `rt.start(app).except(app.deployment("optional")).ready()` still waits for `optional` to become Ready, hanging/deadline-failing the operation on a resource the author explicitly excluded. `rt.exec(started.only(app.job("j")), ...)` fails with "target Started must carry exactly one container instance" whenever the original `Started` held more than one resource.
- **Severity**: medium — wrong barrier scope whenever these documented methods are used; silent, so scripts appear correct.
- **Fix testability**: easy — construct a `Started` with two instances, call `only`/`except`, assert the resource set shrinks.
- **Confidence**: certain

#### `check_barrier`'s `already_satisfied` can match a committed `Stop` entry for a later `.terminated()` barrier
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:1960-1968`
- **Bug**: The satisfied-in-committed-log fast path matches any committed entry by `(resources, required_state, satisfied)` with no positional anchoring and no call-kind check. `do_stop` is the one path that persists `satisfied=true` records (`required_state=Terminated`), so a later `.terminated()` barrier on the same resource set short-circuits against the earlier Stop's record instead of consulting the world.
- **Failure scenario**: `rt.stop(app.job("j")); rt.start(app.job("j")).terminated().ensure_success();` (named jobs get one op-scoped instance UUID, so both calls carry identical resources). The stop commits `Terminated/satisfied=true`; a suspension occurs; on a later pass the `.terminated()` barrier returns immediately while the re-run job is still executing, and `compute_termination` then evaluates a not-yet-terminated resource — `None` → reported as failure, so `ensure_success()` throws and the operation fails even though the job would have succeeded (or, with a stale exit observation, reports the previous run's outcome).
- **Severity**: medium — needs stop-then-rerun of the same resource set within one action, but produces a hard wrong outcome when hit.
- **Fix testability**: easy — `run_operation` script as above with `TestWorldOracle`; suspend once, then assert the barrier does not pass while the oracle still reports Running.
- **Confidence**: likely

#### Sub-action replay re-runs param validation instead of recovering the recorded params
- **Location**: `crates/core/src/runtime/barrier/action_call.rs:74-77` and `144-152`; contract documented at `crates/core/src/runtime/barrier.rs:77-85`
- **Bug**: The `CallKind::SubAction` contract states the log entry's `extra` payload exists "so replay can recover the called action's name and the post-validation params without re-running validation". `call_action` instead validates on every pass (before the replay check) and never reads the committed payload; `record_subaction_entry`'s replay arm just advances `call_index`. Validation is not deterministic across passes: `validate_volume_params` queries the live `site_volumes` table via `RtVolumeLookup`, and `apply_schema` reflects the current AppDef.
- **Failure scenario**: An action calls a sub-action with a `kind: "volume"` param, the sub-action's work suspends on a barrier, and the operator deletes the referenced site volume (or an update changes the sub-action's schema) before the next pass/restart. Replay then throws a validation error mid-operation — the closure diverges from the committed log at a point the original run had already passed, failing (or misaligning) an operation whose sub-action was already in flight.
- **Severity**: medium — replay-window race; violates the determinism requirement of `r[operation.composition.params]` that the in-scope doc comment claims to implement.
- **Fix testability**: moderate — DB-backed harness: commit a SubAction entry, remove the site volume, re-run the pass, assert replay still succeeds (currently fails).
- **Confidence**: likely

#### Explicit deadline `0` (and negatives) means "fail immediately", spec says it means "use the default"
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:1584, 2167, 2179, 2191, 2215` (`d.max(0) as u64` passed straight through)
- **Bug**: `l[rt.started.state-methods]` says "The argument deadline must be a positive integer...; if it's zero or absent, the default deadline for that state is used". The implementation maps `0` (and clamps negatives) to `Some(0)`, which the deadline check (`elapsed >= d`) trips on the very next pass — the opposite of falling back to the default.
- **Failure scenario**: A script author writes `rt.start(dep).ready(0)` expecting the documented default 30s: the operation fails with "Barrier deadline of 0s exceeded" on the second pass, ~2s later.
- **Severity**: low — divergence only for the literal `0`/negative inputs; note tests (`barrier_deadline_zero_expires_on_second_pass`, `rt_stop_deadline_is_enforced`) assert the current behaviour, so either the spec or the code/tests need reconciling — they currently contradict each other.
- **Fix testability**: easy — pure `run_operation` test with `.ready(0)`.
- **Confidence**: certain (about the mismatch; which side is wrong is a spec decision)

#### Barrier `satisfied=true` is never persisted for `check_barrier` barriers, inflating suspension backoff and mis-timing regressed barriers
- **Location**: `crates/core/src/runtime/barrier/runtime.rs:2011-2021` (mark-pending-only) with `crates/core/src/runtime/barrier/replay.rs:511-513` (only pending entries are committed)
- **Bug**: When a previously-suspended barrier finally passes, the pass is replaying, `pending` is empty, and the satisfied flag is written nowhere — the committed record stays `satisfied=false, started_at=<original>` forever (`do_stop` re-pushes a corrected record; `check_barrier` does not). Downstream, `earliest_unsatisfied_barrier_wait_secs` (operation loop) takes the max elapsed over all unsatisfied records, so once any early barrier has been waiting a while, every later fresh barrier in the same operation is polled at the long-wait cadence (up to 300s) instead of the 2s fresh cadence, and "barrier still waiting" logs attribute the wrong elapsed time to the current barrier. It also means a later world regression re-times an already-passed barrier against its original `started_at` (partly mitigated by the oracle-first rule, which only helps while the condition still holds).
- **Failure scenario**: install action: barrier A waits 10 minutes, passes; barrier B starts fresh → next re-poll is scheduled ~18s+ out (ramping toward 300s) rather than 2s, adding minutes of avoidable latency to multi-barrier installs; a short-lived barrier B that would resolve in 3s now takes a full long poll interval.
- **Severity**: low — latency/observability wrongness rather than incorrect terminal outcome; the regression-retiming half is explicitly acknowledged by `r[barrier.deadline]`'s oracle-first rule.
- **Fix testability**: moderate — needs the log + two sequential barriers; assert the first record flips to satisfied after it passes.
- **Confidence**: certain (behaviour), likely (that it is unintended rather than accepted lag)


### 9. Runtime TLS, identity, secrets

#### Exact-hostname fast path serves stale/expired certs over newer valid covering certs

- **Location**: `crates/core/src/runtime/tls/store.rs:351-400` (mirrored in `crates/core/src/runtime/tls/state.rs:320-351`), consumed by `crates/core/src/runtime/tls/serve.rs:66-92`
- **Bug**: `find_active_for_hostname` returns any `state='active'` row whose `hostname` column equals the SNI name before considering SAN coverage of other certs, and never checks `not_after` anywhere on the serve path. This contradicts the spec (`docs/spec/runtime.md` r[tls.strategy.manual]: "When more than one stored cert covers the same hostname, the most recently created active row wins"). Manual/CSR/ACME rows are never auto-superseded by a later cert stored under a different `hostname` column, and manual certs stay `active` forever, so an expired exact-match row permanently shadows a newer valid cert.
- **Failure scenario**: Operator has a manual (or orphaned ACME) cert row for `foo.example.com`, later uploads a fresh `*.example.com` wildcard cert (stored with `hostname = "*.example.com"`, so `supersede_other_active_for_hostname` doesn't touch the old row). The old row expires but remains `active`; every Caddy `get_certificate` lookup for `foo.example.com` returns the expired cert instead of the valid wildcard (or instead of 204/HTTP-01 fallback) → TLS handshake failures/outage for that host with no self-healing.
- **Severity**: high — serves an expired certificate in a realistic per-host-cert-to-wildcard migration flow; effectively an outage until the operator manually deletes the stale row.
- **Fix testability**: easy — pure unit test on an in-memory `Db`: insert an older active exact-hostname cert (expired `not_after`) plus a newer active wildcard cert with covering SAN, assert `find_active_for_hostname` returns the newer/valid one.
- **Confidence**: certain (behaviour); likely (that the spec's newest-covering-row-wins is the intended contract for this case)

#### Tailscale issuance path bypasses retry blocks and failure debounce, retrying every reconciler tick

- **Location**: `crates/core/src/runtime/tls/issuance.rs:298-311` (dispatch before decision) and `issuance.rs:193-278` (`run_tailscale`)
- **Bug**: `run()` dispatches Tailscale-discovered hostnames to `run_tailscale` before loading the unified `compute_state` decision, so `Decision::Blocked` (operator retry-block, spec r[tls.cert.retry-block]: "While a block is set, the issuance coordinator must skip the hostname") and the failure debounce are never consulted. `run_tailscale`'s only guard is "skip if a fresh Tailscale-origin cert exists"; on failure there is no cert, so nothing gates retries.
- **Failure scenario**: tailscaled is down/unreachable while a Tailscale site ingress exists. The reconciler calls `coord.ensure()` every ~5 s (`crates/daemon/src/main.rs:1151`), and each call hits the tailscaled API and inserts + finalises a failed `tls_cert_attempts` row — ~17k rows/day. Knock-on: `state::Snapshot::load` caps attempts at 1000 (`state.rs:114`), so after ~85 minutes every other hostname's `last_attempt` is evicted from the snapshot, silently disabling *their* failure debounce too — ACME hostnames in a failing state then re-attempt against the CA every tick, burning rate limits. An operator retry-block on the tailnet hostname cannot stop the churn.
- **Severity**: high — unbounded retry storm in a plausible failure mode (tailscaled outage), with cascading loss of debounce for unrelated ACME hostnames.
- **Fix testability**: moderate — DB-handle harness: seed a retry block / recent failed attempt for a hostname registered as a Tailscale site ingress, call `run` with an unreachable socket path, assert no new attempt row is opened within the debounce window.
- **Confidence**: certain

#### ACME DNS-01 TXT records leaked on most failure paths

- **Location**: `crates/core/src/runtime/tls/acme.rs:176-231`
- **Bug**: `cleanup_txt` runs only on the `poll_ready` returned-but-not-Ready branch and after success. Every other exit after `set_txt` — `set_ready` error, `poll_ready` transport error, keypair/CSR error, `finalize_csr` error, `poll_certificate` error, a `NoDnsChallenge`/`BadAuthState` on a later authorization — returns via `?` with the `_acme-challenge` TXT record still published.
- **Failure scenario**: CA rejects the challenge (`poll_ready` errors) → the TXT record stays in the operator's Route 53 zone indefinitely. Repeated failures for a hostname later removed from management leave permanent stray records; Route 53's UPSERT masks it for retried hostnames but nothing ever deletes abandoned ones.
- **Severity**: low — no functional breakage of subsequent issuance (UPSERT replaces the value), but persistent pollution of the operator's DNS zone contrary to the module's own cleanup contract.
- **Fix testability**: hard — the error branches sit mid-flight in the `instant-acme` flow; exercising them needs a stub ACME directory (or refactoring cleanup into a guard object that a unit test can observe).
- **Confidence**: certain

#### Wildcard hostnames produce the wrong DNS-01 challenge record name, so wildcard issuance can never succeed

- **Location**: `crates/core/src/runtime/tls/dns.rs:74-77` (`challenge_record_name`), with `crates/core/src/runtime/tls/keypair.rs:59-68` advertising wildcard support
- **Bug**: `challenge_record_name("*.example.com")` yields `_acme-challenge.*.example.com`, but RFC 8555 requires the wildcard's challenge record at `_acme-challenge.example.com` (the `*.` label must be stripped). `keypair::build_csr` explicitly documents that wildcard SANs are preserved, and nothing rejects a wildcard hostname in `issue_acme_dns`/the coordinator, so the flow is reachable.
- **Failure scenario**: Operator calls `tls.cert.issue-acme-dns` with hostname `*.example.com` (a policy for that pattern matches it exactly). Route 53 publishes a literal `\052.example.com` TXT record, the CA queries `_acme-challenge.example.com`, finds nothing, and the order fails — every time, with a confusing challenge-failure error.
- **Severity**: medium — a supported-looking input (wildcard CSR support is documented in-code) deterministically fails; ordinary per-hostname issuance is unaffected.
- **Fix testability**: easy — unit test `challenge_record_name("*.example.com") == "_acme-challenge.example.com"` (or a test asserting wildcard hostnames are rejected up front, if that is the chosen fix).
- **Confidence**: certain (behaviour); likely (that wildcard issuance was meant to work rather than be rejected)

#### `issue_now` corrupts the in-flight dedup set

- **Location**: `crates/core/src/runtime/tls/issuance.rs:285-293`
- **Bug**: `issue_now` inserts the hostname into `in_flight` without checking whether it was already present, and unconditionally removes it when done. If a background `ensure()` task is mid-flight, the marker it owns gets erased by `issue_now`'s completion (or by the background task finishing while `issue_now` still runs), letting subsequent `ensure()` calls spawn duplicate tasks for the same hostname.
- **Failure scenario**: Reconciler `ensure()` is running an ACME flow for `foo.example.com`; operator triggers `issue-acme-dns` for the same host. After the manual call returns, the still-running background task's dedup marker is gone; the next 5-second tick spawns another background task that queues behind `issue_lock` and re-runs the decision — extra no-op flows and, for back-to-back manual triggers, avoidable duplicate orders.
- **Severity**: low — `issue_lock` serialisation plus decision re-evaluation prevents concurrent ACME flows, so the impact is redundant work and log noise rather than incorrect certs.
- **Fix testability**: moderate — needs a coordinator harness with a slow stubbed flow to observe the dedup set across overlapping `ensure`/`issue_now` calls.
- **Confidence**: certain (bookkeeping error); likely (that consequences stay benign given the lock)


### 10. Runtime app/desired-state/image management

The two most serious defects rooted in this subsystem — `AppRegistry::reload` swapping in a partially-evaluated AppDef on script error, and the placeholder `ActionName` in `derive_status` — are reported under §4 (the critical finding C1 and the `/apps/list` finding), because their operator-visible damage happens in the OI handlers.

#### Newly-secret params are only migrated out of plaintext storage at daemon startup, not on reload
- **Location**: `crates/core/src/runtime/apps.rs:156-169` (`reload` — missing call), migration only invoked at `crates/core/src/runtime/apps.rs:252` (`load_from_db`)
- **Bug**: `migrate_newly_secret_params` runs solely in `load_from_db`. When a script update or param-triggered reload flips a param's `secret` flag to true (or a previously-erroring script finally evaluates far enough to declare it secret), the existing plaintext row in `params` is left in place until the next daemon restart. Spec r[secret.migration] (`docs/spec/runtime.md:1394-1395`) requires the value be moved to protected storage "at the next opportunity, without requiring operator intervention".
- **Failure scenario**: App has `app.param("apikey")` with a stored plaintext value; operator updates the script to `app.param("apikey").kind("password")`. The reload succeeds, `def.params` marks it secret, but the plaintext value stays in the unencrypted `params` table indefinitely on a long-running daemon (months on a stable deployment), despite the operator having explicitly marked it secret.
- **Severity**: medium — secret value persists unprotected at rest for an unbounded window in a realistic flow; no functional breakage (readers merge both tables).
- **Fix testability**: easy — in-memory Db: register, `upsert_param`, `reload` with a script declaring the param secret, assert `params` row gone and `secret_params` row present.
- **Confidence**: likely

#### Re-pinning via `rt.warm_images` does not clear a pending pin expiration
- **Location**: `crates/core/src/runtime/images.rs:27-34` (`upsert_pin`)
- **Bug**: `upsert_pin` uses `ON CONFLICT(app, reference) DO NOTHING`, so a pin that already carries an `expires_at` (stamped by a dirty-probe `reconcile_pins_after_update`) keeps that expiry even when the app explicitly re-warms the reference. Spec r[image.pin.expiry] (`docs/spec/runtime.md:844`) says expirations "are cleared whenever a pin's reference is observed to be valid again for the owning app", and r[image.pin] treats a `rt.warm_images` call as the way to re-pin — the re-pin is silently a no-op here.
- **Failure scenario**: A script update with a skipped probe handler stamps a 30-day expiry on pin `(app, ghcr.io/x:1)`. Days later a scheduled action calls `rt.warm_images` for the same reference (image pulled/kept warm for a future upgrade). The stale expiry survives; on day 30 the reconciler deletes the pin and image GC removes the image the app just asked to keep warm, forcing a re-pull at the worst moment.
- **Severity**: medium — wrong pin lifecycle in an edge case (expiring pin + explicit re-warm), leading to unwanted image GC; self-heals with a re-pull.
- **Fix testability**: easy — pure unit test on in-memory Db: stamp `expires_at`, call `upsert_pin`, assert `expires_at` is NULL (change conflict action to `DO UPDATE SET expires_at = NULL`).
- **Confidence**: likely


### 11. Runtime site networking: site services, ingresses, attachments, external mappings, tailscale

#### Tailscale provider never marks the discovered ingress stale when tailscaled is unreachable
- **Location**: `crates/core/src/runtime/tailscale.rs:169-177`
- **Bug**: The `run()` loop's `Unreachable` arm (socket missing / connection refused — i.e. tailscaled stopped or uninstalled) only logs and records status; it does **not** call `mark_existing_stale(true)`, and it resets `consecutive_failures` to 0 so the API-error threshold path can never fire either. Spec `r[ingress.site.lifecycle]` explicitly requires: "When a discovery source temporarily disappears (e.g. the underlying provider becomes unreachable) the corresponding site ingress must be marked stale". Staleness is only set for API/decode errors (after 5 in a row) and for reachable-but-logged-out states — not for the most common outage mode, the daemon being down.
- **Failure scenario**: Tailscale ingress exists (non-stale) with attachments; operator runs `systemctl stop tailscaled` (or uninstalls Tailscale). Every poll returns `Unreachable`; the row stays `stale = 0` forever. The reconciler keeps emitting routes for the dead `*.ts.net` hostname, and the TLS coordinator keeps dispatching that hostname to the Tailscale issuer (which requires a non-stale row and then fails against the dead socket), producing repeated cert-attempt failures. Because `delete_site_ingress` rejects any discovered ingress, the operator also has no way to remove the ghost ingress.
- **Severity**: high — spec-mandated behaviour is missing in the most common provider-outage mode; ingress state and TLS issuance are wrong until tailscaled returns.
- **Fix testability**: moderate — extract the poll-result handling from `run()` into a testable function and assert against an in-memory `Db` that an existing discovered row becomes stale on `Unreachable`.
- **Confidence**: certain

#### `mark_existing_stale` / upsert have no ownership check, so a manual ingress named "tailscale" is permanently disabled
- **Location**: `crates/core/src/runtime/tailscale.rs:287-296` (and `tailscale.rs:390-403`)
- **Bug**: `mark_existing_stale` looks up the row by the fixed name `"tailscale"` and sets `stale` without checking `source.is_discovered()`. Nothing reserves that name — `create_site_ingress` happily creates a manual ingress called "tailscale". Additionally, when the provider later gets a healthy identity, `upsert_discovered_row` refuses to replace a manual row (correctly) but then unconditionally attempts `create` under the same name, which fails on the PK forever — so the stale flag is never cleared (`set_stale(..., false)` is only reachable via the discovered-row path) and no discovered ingress can ever be created.
- **Failure scenario**: Operator creates a manual site ingress named "tailscale" for `foo.example.com` with attachments. tailscaled reports logged-out (or no Self identity) on any poll → the provider marks the operator's **manual** ingress stale → the reconciler drops all its attachments from the proxy config. There is no OI route to clear `stale`; even when Tailscale is healthy again the flag stays set (the upsert `create` errors every poll). Recovery requires deleting and recreating the ingress (losing attachments) or DB surgery.
- **Severity**: medium — silent, permanent outage of an operator resource, but only when the operator picks the colliding name.
- **Fix testability**: easy — pure unit test with `Db::open_in_memory()`: create a manual ingress named "tailscale", call `reconcile_db(None)` (or `upsert_discovered_row`), assert the manual row is untouched.
- **Confidence**: certain

#### External service/volume mapping rows are never purged on app deregistration and are silently inherited by a future app with the same name
- **Location**: `crates/core/src/runtime/external_service_mappings.rs` and `crates/core/src/runtime/external_volume_mappings.rs` (no per-app delete exists; the deregister cleanup at `crates/core/src/oi/handler/apps.rs:1366-1394` purges params, generations, faults, scaling, restart-gens, stopped, schedules — but not these two tables)
- **Bug**: Both mapping tables are keyed `(app, external_name)` with no FK to `registered_apps` and no cleanup on deregister. Rows for a fully deregistered app persist indefinitely. Consequences: (a) a later, unrelated app registered under the same name silently inherits the old app's mappings — including **volume** mappings (`read_only` flag and target volume), so the new app can mount data volumes the operator configured for the previous app; (b) orphan site-target service mappings keep blocking `delete_site_service` (both the OI check at `services.rs:261` and the v39 trigger) with an error naming an app that no longer exists.
- **Failure scenario**: Deregister app `blog` that had `external volume "data" → app{files, uploads}`; months later register a different app also named `blog` declaring an external volume `data` → it is auto-wired to `files/uploads` and mounts that data with no operator action or event. Or: deregister the only app mapping `postgres-prod` → `site service delete postgres-prod` is refused forever until the operator discovers the ghost mapping and unmaps it manually.
- **Severity**: medium — every other per-app table is purged on deregister, and the volume-inheritance case exposes another app's data; requires deregister + name reuse, or leaves confusing delete blocks.
- **Fix testability**: easy — add `delete_for_app` functions with pure `Db::open_in_memory()` unit tests, plus a handler-level test via the existing OI test support.
- **Confidence**: likely (persistence across *uninstall* is clearly intended; the asymmetry at *deregister* strongly suggests these tables were forgotten)

#### One failing DNS address family discards the other family's fresh results
- **Location**: `crates/core/src/runtime/site_services/resolver.rs:396-433` (`lookup_records`)
- **Bug**: `lookup_records` treats any AAAA-query error other than `NoRecordsFound` as a total failure (`return Err(other)`) before the A query even runs, and vice versa. A resolver that answers A queries fine but returns SERVFAIL/REFUSED for AAAA (a well-known broken-middlebox behaviour) makes the host permanently unresolvable — `record_failure` runs, no records are ever cached, and after 5 ticks a `site_service_endpoint_unresolvable` fault is filed even though perfectly good A records were obtainable the whole time.
- **Failure scenario**: Site service endpoint `db.corp.internal` behind a corporate resolver that REFUSES AAAA queries → endpoint never resolves, backend pool for that endpoint stays empty, fault filed; meanwhile `dig A db.corp.internal` works.
- **Severity**: medium — full loss of an endpoint in an environment-dependent but real-world DNS configuration; correct data was available.
- **Fix testability**: moderate — needs a stub DNS server (or hickory mock) that errors one record type; the surrounding streak/fault logic is already unit-tested via `seed_*`.
- **Confidence**: likely

#### AAAA-only endpoint without IPv6 egress is misreported as "NAT64 not active"
- **Location**: `crates/core/src/runtime/site_services/resolve.rs:147-154`
- **Bug**: When a DNS name has AAAA records but the host lacks IPv6 egress (and there are no A records), the outcome is `Unroutable { NeedsNat64ButDisabled }` — the only variant of `UnroutableReason` — even when NAT64 **is** active. The reconciler's fault text (`faults.rs:888`) then tells the operator the endpoint "require[s] NAT64 but NAT64 is not active", which is factually wrong in both directions: NAT64 may be active, and enabling it would not help (the real problem is no IPv6 egress and no A records).
- **Failure scenario**: v4-only host with NAT64 enabled, endpoint `v6only.example.com` (AAAA-only) → fault filed saying "require NAT64 but NAT64 is not active"; operator wastes time on NAT64 configuration that cannot fix it. (The `Unroutable` classification itself is correct and test-asserted; only the reason/diagnostic is wrong.)
- **Severity**: low — routing decision is correct; only the operator-facing diagnosis is misleading.
- **Fix testability**: easy — add an `UnroutableReason` variant (e.g. `NoIpv6EgressForAaaaOnly`) and adjust the existing pure unit test `dns_aaaa_only_without_v6_egress_is_unroutable`.
- **Confidence**: certain (of the behaviour; the existing test asserts the current reason, so confirm intent before changing)

#### Promised `tailscale_unreachable` fault is never filed
- **Location**: `crates/core/src/runtime/tailscale.rs:35-37` (doc on `FAULT_AFTER_FAILURES`)
- **Bug**: The constant's documentation says that after 5 consecutive transient API errors "the provider files a `tailscale_unreachable` system fault", but no code anywhere in the crate files such a fault (the string appears only in this comment). At the threshold the provider only marks the row stale, so the operator gets a silently-stale ingress with no fault explaining why (and per project convention, the `warn!` logs here should be paired with a fault).
- **Failure scenario**: tailscaled starts returning HTTP 500 on `/localapi/v0/status`; after 5 polls the ingress goes stale and its attachments stop serving, but the fault list shows nothing — the operator must grep logs to learn why traffic stopped.
- **Severity**: low — observability gap during a provider outage, no incorrect routing.
- **Fix testability**: moderate — needs the DB-backed faults table (in-memory `Db` works) plus extracting the poll-failure handling from `run()`.
- **Confidence**: certain (that no fault is filed); likely (that filing one is the intended behaviour)


### 12. System reconciliation engine

#### Transient observe failure kills a running Job permanently
- **Location**: `crates/core/src/system/reconcile/pods.rs:140-163` and `crates/core/src/system/reconcile/pods.rs:319-360`
- **Bug**: When `observer.observe()` errors, `observe_one_pod` logs "skipping instance" but returns a fully actuatable `ObservedInstance` with every flag false (`is_running: false`, `container_exists: false`), and `actuate_one_pod` has no check on `observe_failure`. For a Job with `desired == Ready`, the terminal-detection predicate `(!container_exists && !is_running && previously_ran)` then evaluates true, so the reconciler stops the Job and inserts it into `completed_jobs`.
- **Failure scenario**: A long-running Job is mid-execution (`container_running` already persisted, so `previously_ran` is true). One podman/systemd query fails transiently (`tokio::try_join!` in `observe_pod_instance` bubbles any of the three probe errors). That tick, the Job is treated as "naturally terminated": `actuator.stop` kills the container, and `job-terminal.defense` guarantees it is killed again if it ever reappears. Spec `r[autonomous.job-terminal]` requires the reconciler to "currently observe as gone" — a failed observation is not an observation of absence.
- **Severity**: high — a single transient observation error destroys an in-flight batch workload and permanently prevents its completion.
- **Fix testability**: moderate — stub System whose observer errors once for a Job instance previously marked started; assert no stop is issued.
- **Confidence**: certain

#### Uninstall unit-prefix match stops sibling apps whose names extend the uninstalling app's name
- **Location**: `crates/core/src/system/reconcile.rs:1448-1496`
- **Bug**: `run_uninstall_phase` uses `list_units("seedling-{app}-")`, a plain `starts_with` filter (`systemd.rs:410`). Unit names are `seedling-{app}-{resource}-{suffix}.service` and app names may contain hyphens, so app `foo`'s prefix `seedling-foo-` also matches every unit of app `foo-bar`.
- **Failure scenario**: Apps `app` and `app-db` are both installed; the operator uninstalls `app`. Once `app`'s own units are gone, `list_units` still returns `app-db`'s units, so (1) the `units.is_empty()` completion check never passes — `app` is stuck in `Uninstalling` forever — and (2) the retry branch calls `reset_failed_unit` + `stop_unit` on **`app-db`'s running units every 5-second tick**, which `app-db`'s reconcile pass then restarts, bouncing a healthy app indefinitely.
- **Severity**: high — repeated outage of an unrelated healthy app plus a never-completing uninstall, triggered by a perfectly ordinary naming pattern.
- **Fix testability**: moderate — stub process manager returning units for both apps; assert only exact-app units are targeted and uninstall completes.
- **Confidence**: certain

#### Reconciler tears down units in systemd auto-restart backoff, defeating the start-limit / crash_loop mechanism
- **Location**: `crates/core/src/system/reconcile/pods.rs:433-457` (with observer mapping `Activating → UnitActive`, `observer.rs:239-242`)
- **Bug**: During systemd's `Restart=` backoff a transient unit sits in `activating (auto-restart)` with no running container. The observer maps `Activating` to `UnitActive`, so the teardown branch `obs.unit_failed || obs.unit_active` fires: the reconciler stops and destroys the unit mid-backoff and starts a **fresh** transient unit next tick — which resets systemd's `StartLimitBurst` accounting.
- **Failure scenario**: A container crashes ~1s after start. `restart_sec` is 5s and the reconcile tick is 5s, so nearly every crash window is observed as `unit_active && !is_running` → `stop_broken_unit` → new unit. Each unit only ever accumulates 1-2 starts, so `failed/start-limit-hit` is never reached, the `crash_loop` hard fault required by `r[autonomous.restart.start-limit-hit]` never files, and the workload churns (image setup, network create/destroy, unit create/destroy) every ~10s forever — exactly the unbounded auto-recovery the spec's backoff item (`r[autonomous.restart.backoff]`: limits exist so systemd gives up "before the reconciler" intervenes) is meant to prevent. It also tears down containers momentarily in the normal `Created`→`Running` startup gap.
- **Severity**: medium — no data loss, but the spec'd hard-fault path is effectively unreachable for fast-crashing containers and the node churns indefinitely.
- **Fix testability**: moderate — distinguish `Activating`/auto-restart from `Active` in observation and unit-test the actuation decision with a stubbed observed state.
- **Confidence**: likely

#### App skipped on desired-state/registry error has its whole data plane torn down (and all-apps failure triggers full idle teardown)
- **Location**: `crates/core/src/system/reconcile.rs:550-562`, `crates/core/src/system/reconcile.rs:675-678` (same pattern in `phases.rs:114-116`, `phases.rs:237-241`, `phases.rs:358-361`)
- **Bug**: When `compute()` fails for an app, `snapshot_all_apps` drops the app from the tick entirely. The nftables rules, service routes, and proxy config are then rebuilt from the surviving apps only and **applied**, wiping the skipped app's ingress DNAT, service routes, and vhosts while its containers are still running. If every installed app fails compute, `apps.is_empty()` is true and `tear_down_idle()` flushes all rules/routes and removes the Caddy and resolver containers and the NAT64 instance — a full infra teardown on a live node. The per-app `routes::build`/`build_service_dnat_rules`/`proxy::collect` registry-error `continue`s have the same drop-then-apply effect.
- **Failure scenario**: A transient rusqlite error in the registry during `compute` for the sole installed app → the tick treats the system as idle, tears down Caddy/resolver/NAT64 and flushes nftables while the app's pods run on → total connectivity outage until compute succeeds again, then full infra re-bring-up (oscillation if the error is intermittent).
- **Severity**: medium — outage-grade blast radius, but only reachable via an unusual (DB/registry) error; "fault.non-blocking" intended skip semantics become destructive because the applied state is absolute, not incremental.
- **Fix testability**: moderate — stub registry that errors for one app; assert previously-applied rules for that app survive the tick.
- **Confidence**: certain (mechanism); the trigger frequency is the only uncertainty

#### Healthcheck-replace uses declared lower bound instead of effective scale as the healthy target
- **Location**: `crates/core/src/system/reconcile.rs:1312` (`let target = dep_def.scale.start;`)
- **Bug**: The bump condition is `any_unhealthy && healthy < target` with `target` fixed at the scale range's lower bound, but the running instance count follows the operator-chosen effective scale (`scaling::effective_scale`, used at `reconcile.rs:603-613`). Spec `r[autonomous.healthcheck-replace]` requires spawning a replacement whenever an `on_failure: replace` instance is observed unhealthy.
- **Failure scenario**: Deployment declares scale 1..5; operator scales to 5. One instance goes unhealthy: healthy = 4, target = 1 → `4 < 1` is false → no replacement is ever spawned and the unhealthy instance is never swapped out; the deployment sits permanently one backend short (routing excludes it via prefer-healthy).
- **Severity**: medium — self-healing silently disabled for any deployment scaled above its lower bound, a normal operating mode.
- **Fix testability**: moderate — db-harness test with a scaling decision row and one unhealthy pod; assert the deployment lands in `unhealthy_replace_deployments`.
- **Confidence**: likely

#### Replace-loop guard never evaluates rolling updates of `on_failure: monitor` deployments
- **Location**: `crates/core/src/system/reconcile.rs:1325-1332` and `1363-1369`
- **Bug**: `grace_secs_by_dep` is populated only *after* the `if !matches!(policy, Some(Replace)) { continue; }` filter, and the guard's candidate list (`unhealthy_replace_deployments ∪ rolling_updates`) is `filter_map`ed through that map. So a rolling update on a deployment with a healthcheck but `on_failure: monitor` has no grace entry and is never checked, though spec `r[autonomous.healthcheck-replace.guard]` covers "a fresh instance brought up by **either** a rolling update or by healthcheck-replace".
- **Failure scenario**: Monitor-policy deployment with a healthcheck gets a bad code push; the new-hash instance never becomes healthy. `compute_stop_inhibitions` keeps all stale instances inhibited and the +1 over-provision active forever; no `health_check_replace_failed` fault is filed; the rollout livelocks silently until the next push.
- **Severity**: medium — permanent stuck rollout with no operator signal, but requires the non-default `monitor` policy.
- **Fix testability**: moderate — unit-level: move the `grace_secs_by_dep.insert` above the policy filter and test candidate construction with a monitor-policy deployment in `rolling_updates`.
- **Confidence**: likely

#### `stop_failed` faults from scale-down stops are filed and immediately cleared in the same tick
- **Location**: `crates/core/src/system/reconcile/faults.rs:513-537` with `crates/core/src/system/reconcile/pods.rs:533-535`
- **Bug**: The Unscheduled stop path pushes a `stop_sent` observation *before* attempting `actuator.stop`, unconditionally. `file_pod_actuation_faults` files `stop_failed` faults for that tick's stop failures, then clears every `stop_failed` fault whose instance emitted `stop_sent` this tick — which includes the very instance whose stop just failed.
- **Failure scenario**: Scale-down stop fails every tick (e.g. network removal keeps erroring). Each tick a `stop_failed` fault is created and cleared inside one `db.call`, so no active fault is ever visible to the operator, while the fault table churns two writes per tick indefinitely.
- **Severity**: medium — a persistent actuation failure in a routine flow (scale-down/retire) is invisible; the retire path then also wedges (instance never reaches terminal state).
- **Fix testability**: moderate — db-harness: feed a `PodActuationUpdate` containing both a `stop_failure` and its `stop_sent` observation; assert an active fault survives.
- **Confidence**: certain

#### `ingress_conflict` and site-service endpoint faults never clear after a daemon restart
- **Location**: `crates/core/src/system/reconcile/faults.rs:613-732` and `crates/core/src/system/reconcile/faults.rs:822-934`
- **Bug**: Both reconcilers clear faults only for entries in `prior \ current`, where `prior` is an in-memory set (`prev_ingress_conflicts`, `prev_site_service_faults`) that starts empty on every process start, while the faults themselves are persisted in the DB. Unlike `reconcile_unresolved_site_attachments` (which sweeps all active faults against the current keep-set each tick), there is no full sweep.
- **Failure scenario**: A conflict exists → fault filed → daemon restarts → operator removes the conflicting site ingress while the daemon is down (or before the first tick). On the first tick `current` no longer contains the tuple and `prior` is empty, so `resolved` is empty — the persisted `ingress_conflict` fault stays active forever (until the same conflict recurs and resolves within one process lifetime). Same for `site_service_endpoint_unresolvable`/`unroutable`. Additionally, within one process, a fault for an app that leaves a conflict is not cleared as long as another app keeps the same `(host, port)` conflicted, since clearing is keyed on the tuple, not the party.
- **Severity**: medium — never-resolving stale faults that mislead operators; no functional impact on the data plane.
- **Fix testability**: moderate — db harness: pre-seed an active `ingress_conflict` fault, run `reconcile_ingress_conflicts` with an empty report on a fresh Reconciler, assert the fault clears.
- **Confidence**: certain

#### An empty proxy config is never applied, leaving stale Caddy state and an unclearable `proxy_failed` fault
- **Location**: `crates/core/src/system/reconcile.rs:950-951`, `1012-1018`, `1068-1084`
- **Bug**: When `virtual_hosts` and `l4_routes` are both empty, `apply_config` is skipped and the success branch (`Ok(()) if has_proxy_config`) is also skipped. Removing the last ingress in the system therefore never propagates to Caddy: the previous vhosts/l4 routes stay loaded (Caddy keeps listeners, certs, and routes to now-dead service IPs), the cached upgrade-continuity config on disk keeps the stale vhosts, `ingress_removed` observations are persisted claiming teardown happened, and a lingering `proxy_failed` system fault from an earlier failed apply can never clear while the config remains empty.
- **Failure scenario**: One app with one ingress; the operator pushes an update deleting the ingress (app still installed, so idle teardown never runs). Caddy serves the stale vhost indefinitely; external exposure is mostly masked because the nftables ingress DNAT is removed, but Caddy-local listeners, the cached config, and fault state all stay wrong.
- **Severity**: low — drift is mostly masked by nftables; main harm is stale cached config restored on upgrade and a stuck fault.
- **Fix testability**: moderate — stub proxy: apply non-empty then empty config; assert the second apply happens (or fault clears).
- **Confidence**: likely (the skip is clearly deliberate as an optimisation, but the remove-last-ingress and fault-clear consequences look unintended)

#### `OBS_KINDS` restart-dedup seed is missing three persisted kinds
- **Location**: `crates/core/src/system/reconcile.rs:189-210`
- **Bug**: `seed_written_obs` can only intern kinds present in `OBS_KINDS`, but `ObservationFact::to_obs_kinds` (types.rs:524-576) also persists `health_check_fail`, `unit_start_limit_hit`, and `volume_backend_mismatch`, none of which are in the list. Their DB rows are dropped from the seed on restart, defeating the documented purpose ("observations persisted in a previous session are not re-written with a fresh timestamp on restart").
- **Failure scenario**: Container persistently unhealthy across daemon restarts → each restart appends a duplicate `health_check_fail` row with a fresh timestamp; same for a start-limit-hit unit and a backend-mismatched volume. Observation history accumulates duplicates and downstream timestamp-based reasoning sees a falsely-recent first occurrence.
- **Severity**: low — history noise/duplication only; within-process dedup still works.
- **Fix testability**: easy — pure unit test asserting every kind emitted by `to_obs_kinds` (plus the route/proxy synthetic kinds) is internable by `OBS_KINDS`.
- **Confidence**: certain

#### `emit_state_changes` emits duplicate state-change events for scaled deployments
- **Location**: `crates/core/src/system/reconcile/state.rs:48-99`
- **Bug**: `groups` is built with one entry per *desired resource instance*, so a deployment scaled to N produces N identical `(app, kind, res_name)` groups; each group's `find_instances_for_group` returns all N instances, yielding N copies of every `StateEntry`. The event loop compares each copy against the unchanged `prev_states`, emitting the same `resource_state_changed` event N times per actual transition (and running N× redundant observation queries).
- **Failure scenario**: Deployment scaled to 3; one instance transitions Ready→Terminating. The UI event stream receives 3 identical `resource_state_changed` events (9 DB group/observation queries per tick for the group).
- **Severity**: low — duplicate events and wasted queries; state bookkeeping itself stays correct.
- **Fix testability**: easy — dedupe `groups` by key and unit-test that entries are unique per instance; or db-harness asserting one event per transition.
- **Confidence**: certain


### 13. System actuation: actuator, observer, breadcrumb, journal, stub, types

#### Observer can never detect systemd's start-limit-hit, so crash-loop protection is dead code
- **Location**: `crates/core/src/system/observer.rs:236`
- **Bug**: `UnitStartLimitHit` is emitted only when `active == Failed && s.sub == "start-limit-hit"`, but `sub` is populated from systemd's `SubState` property (`systemd.rs:393`), which for a failed service is always `"failed"`. `"start-limit-hit"` is a value of the separate `Result` property, which `unit_state` never reads. So the fact is unreachable against the real backend. (Compounding: `CollectMode=inactive-or-failed` set in `systemd.rs:227-230` garbage-collects the failed unit, so the observer typically sees `UnitGone` instead.)
- **Failure scenario**: A container crash-loops until systemd gives up (start-limit-hit). The observer reports `UnitFailed`/`UnitGone` instead of `UnitStartLimitHit`, so `pods.rs` never sets `unit_start_limit_hit`, no `crash_loop` fault is filed (r[autonomous.restart.start-limit-hit] / spec runtime.md:698, 1082), and the reconciler's start path calls `reset_failed_unit` + `start_transient` every 5 s tick — defeating systemd's start limit and restarting the broken container forever with no operator-visible hard fault. Existing tests pass because they inject `ObservationFact::UnitStartLimitHit` directly and the stub ProcessManager only produces `running`/`dead` sub-states.
- **Severity**: high — a spec-mandated autonomous-recovery/fault path silently never fires in production; permanently broken containers restart indefinitely.
- **Fix testability**: hard — needs a real systemd unit driven into start-limit-hit to validate whichever property the fix reads; the sub-state mapping itself is unit-testable once `unit_state` exposes `Result`.
- **Confidence**: likely

#### Image pull retries have no back-off and exhaustion is permanent
- **Location**: `crates/core/src/system/actuator/pull.rs:42-59`
- **Bug**: A failed pull sets `in_flight = false`, and the next `ensure_image_available` call (every 5 s reconciler tick, `daemon/src/main.rs:1151`) retries immediately — there is no back-off despite `images.rs:176` and spec `interface.md:786` ("retries and back-off"). After 5 failures the entry is marked `exhausted` and nothing ever clears it (`pulling` entries are only removed on pull success, which can no longer happen automatically).
- **Failure scenario**: Registry is unreachable for ~30 seconds while an app needs an image. Five attempts burn in ~25 s, the image is marked exhausted, and even after the registry recovers the reconciler never pulls it again — the app stays down with an `image_pull_failed` fault that spec `runtime.md:1074-1075` says "is cleared automatically when a subsequent pull succeeds", but no subsequent automatic pull ever occurs. Recovery requires a manual `/images/pull` (which bypasses the map) or a daemon restart.
- **Severity**: high — transient registry blips are routine at deployment scale and permanently disable automatic actuation of the affected workload.
- **Fix testability**: moderate — unit-test the `PullState` decision logic with a stub runtime whose `pull_image` fails, using `tokio::time::pause` for back-off timing.
- **Confidence**: certain (behaviour as coded); the "intended back-off" reading is supported by the spec and the contradicting comment in `images.rs`.

#### Declared writes on named volumes can be silently lost to a pods-phase/volumes-phase race
- **Location**: `crates/core/src/system/actuator/pod.rs:162-177` (with `crates/core/src/system/actuator.rs:379-395` and `reconcile.rs:704-720`)
- **Bug**: `ensure_volumes` creates a missing named non-tmpfs volume via `vol_store.create` but deliberately skips its declared `writes` ("The reconciler handles their lifecycle"). The reconciler's `Actuator::start(Volume)` applies writes only when `!vol_store.exists(&name)` (`just_created`). The pods phase and volumes phase run concurrently (`tokio::join!` in `reconcile.rs:704`), and `create` is idempotent `create_dir_all`, so if the pod's `ensure_volumes` creates the directory first, the volume phase sees it existing and never applies the writes — permanently, since named volumes only get writes on first creation.
- **Failure scenario**: App with a Deployment mounting a named volume that declares config-file `writes` is installed while its image is already local (e.g. pre-warmed via `rt.warm_images`). On the first tick both phases actuate concurrently; if the pod phase wins the create, the container starts with an empty volume and the declared config file never appears, with no error or fault.
- **Severity**: medium — interleaving-dependent, but the outcome is silent, permanent misconfiguration in a supported flow (warmed images make it realistic).
- **Fix testability**: moderate — with the stub System, drive `start_pod_instance` for a pod mounting a not-yet-created named volume, then `Actuator::start` for the Volume, and assert the declared writes exist.
- **Confidence**: likely

#### Journal tail reader drops one entry (`--tail 1` returns nothing)
- **Location**: `crates/core/src/system/journal.rs:118-127`
- **Bug**: The reader seeks to Tail, calls `previous()` `tail` times (cursor lands *on* the tail-th-from-last entry), then reads with `next_entry()`, which advances *past* the current entry (sd_journal semantics; the crate is a thin wrapper — verified in systemd 0.10.1 source). The entry the cursor is on is never emitted, so every request yields at most `tail - 1` historical entries, always dropping the oldest of the window; the same applies when the journal has fewer than `tail` entries (the very first entry is skipped).
- **Failure scenario**: `seedling-ctl apps logs <app> --tail 1` (non-follow) returns zero lines even though matching entries exist; `--tail 100` returns 99. The correct pattern (as journalctl does) reads the current entry first before calling next.
- **Severity**: medium — affects every log read; mostly cosmetic at large N but `--tail 1`/small-N flows return visibly wrong (empty) output.
- **Fix testability**: hard — needs a journald instance with seeded entries to verify counts end-to-end.
- **Confidence**: likely

#### Named tmpfs Volume actuation delegates to podman's tmpfs driver and persists "RAM-only" writes to disk
- **Location**: `crates/core/src/system/actuator.rs:334-362`
- **Bug**: For a tmpfs `Resource::Volume`, `start` creates a podman volume with the tmpfs driver and applies the declared `writes` to its `volume_mountpoint` (`/var/lib/containers/storage/volumes/<name>/_data`). Spec `runtime.md:900` explicitly forbids delegating tmpfs storage to the container runtime's tmpfs driver. Containers never mount this volume — `translate/container.rs:310-318` bind-mounts named tmpfs volumes from `/run/seedling/tmpfs-volumes/<VolumeName>` instead — so the writes are dead copies, and because no tmpfs is mounted over `_data` until a container mounts the volume (which never happens), the contents sit on persistent disk.
- **Failure scenario**: A BSL app declares a named tmpfs volume with a `writes` entry containing a secret (the whole point of tmpfs volumes being RAM-backed). The secret is written to persistent disk under podman's storage directory and survives reboots until the Volume resource is stopped, violating the RAM-based-filesystem semantic.
- **Severity**: medium — data-confidentiality semantics violated in an edge-but-supported configuration; also leaves an author-confusing dead write path (the comment "always re-apply writes" believes this volume is the real storage).
- **Fix testability**: moderate — with the stub runtime, assert `Actuator::start(Volume{tmpfs})` no longer writes into the podman volume mountpoint; disk-persistence aspect needs real podman.
- **Confidence**: likely

#### Stopping a named tmpfs Volume leaves its real backing data behind
- **Location**: `crates/core/src/system/actuator.rs:463-479`
- **Bug**: `stop` for a tmpfs Volume removes only the podman marker volume. The actual data lives in `/run/seedling/tmpfs-volumes/<VolumeName>` (the path containers bind-mount), which is never removed — `stop_pod_instance` cleans only *anonymous* volumes' host paths. Spec `runtime.md:1013`: "Stopping a Volume instance must remove the named volume."
- **Failure scenario**: Operator removes a tmpfs volume from an app (or uninstalls the app), then later re-adds a volume with the same name within the same boot. `ensure_volumes` just `create_dir_all`s the existing directory and re-applies declared writes, so all stale files from the deleted volume reappear in the new container. Meanwhile the observer reports `VolumeMissing` → `volume_cleaned_up` even though the data still exists in RAM.
- **Severity**: medium — wrong behaviour (data resurrection, cleanup reported but not performed) in an edge flow within a single boot.
- **Fix testability**: moderate — stub System with `TMPFS_VOLUMES_DIR` pointed at a temp dir; stop the Volume and assert the backing directory is gone.
- **Confidence**: likely

#### Observer reports a gracefully-stopping container as removed
- **Location**: `crates/core/src/system/observer.rs:201` (with `types.rs:537`)
- **Bug**: `ContainerStatus::Unknown` is mapped to `ObservationFact::ContainerMissing`, which `to_obs_kinds` persists as `container_removed`. Podman maps its `"stopping"` (and `"removing"`, `"initialized"`) states to `Unknown` (`podman.rs:807-811`), so a container that exists and is draining is recorded as removed.
- **Failure scenario**: A workload with a long `stop_timeout_secs` (e.g. postgres flushing) is stopped; ticks during the drain window observe `container_removed` in `world_observations`, prematurely advancing the lifecycle oracle (e.g. satisfying stop/termination barriers) while the old container still holds its volumes and network — an operation sequenced after the stop can start against a not-yet-released resource.
- **Severity**: medium — wrong observation in a real but timing-dependent window; consequences depend on downstream barrier consumers.
- **Fix testability**: moderate — feed the observer a fake ContainerRuntime returning a `"stopping"`-derived state and assert no `container_removed` fact is produced.
- **Confidence**: likely (misclassification is certain from the code; downstream harm is possible). Independently found by the host-integration auditor via `parse_container_status` (`podman.rs:805-813`), which is the co-located root cause: any status string podman emits that is not in the recognised list (`"stopping"`, `"removing"`, `"initialized"`) maps to `Unknown`.

#### Stub `start_transient` extracts the wrong image from the argv, corrupting stub-mode image bookkeeping
- **Location**: `crates/core/src/system/stub.rs:467-473`
- **Bug**: The stub picks the image as the *last* argv element containing `/` or `:` (`.iter().rev().find(...)`). `podman_args` puts the image *before* the command/entrypoint args, so any container whose trailing command args contain `/` or `:` (e.g. `["sh", "-c", "..."]`, `["node", "server/index.js"]`) gets a command argument recorded as its image; a phantom `StubImage` is minted for it and the container's `image_id` doesn't match the actually-pulled image.
- **Failure scenario**: In stub-backed tests, `reconcile/images.rs` marks the phantom id as in-use instead of the real image's id, so the real image's `last_used` is never refreshed and stub-mode image-GC/pin tests exercise different protection behaviour than production — hiding (or fabricating) GC-protection bugs; `/images/list` also shows phantom entries.
- **Severity**: low — stub-only, but it undermines the fidelity of exactly the image-tracking flows the stub exists to test.
- **Fix testability**: easy — build a `TransientUnitSpec` from `podman_args` for a spec with a command containing `/`, call the stub, assert `inspect().image_id` matches the pulled reference's id.
- **Confidence**: certain (mechanism), likely (test impact)


### 14. System networking: caddy, data plane, translate, resolver, jool, nat64, netinfo

#### Resolver blue/green upgrade starts the new slot on the address the old slot still holds
- **Location**: `crates/core/src/system/resolver/startup.rs:412-418` (with `start_slot` at 129-204 and `poll_until_healthy` at 222-250)
- **Bug**: On image mismatch, `start_slot(other, …, &resolver_ip)` runs the new container with `--ip6 <resolver_addr>` while the active container is still running with that exact static address on the same `seedling-resolver` network. Netavark rejects a duplicate static IP allocation, so the new container never reaches `Running` and the upgrade times out every tick. Independently, `poll_until_healthy` probes `http://[resolver_ip]:8080/health` — an address the *old* container still owns — so the "new slot" health check is answered by the old container and proves nothing about the new one. Contrast with the Caddy upgrade, which uses a per-slot admin socket.
- **Failure scenario**: Bump `RESOLVER_IMAGE` (e.g. CoreDNS 1.12.1 → 1.12.2) and deploy. Every tick: podman run in the green slot fails with "IP address already allocated", the unit restart-loops until the start limit, `poll_until_healthy` times out after 60 s, the tick files a resolver fault, and the upgrade never completes.
- **Severity**: high — the documented resolver blue/green upgrade path cannot succeed, and per-tick 60 s timeouts stall the resolver phase on every reconciliation.
- **Fix testability**: moderate — the stub `ContainerRuntime` can model per-network static-IP allocation and assert the new slot uses a distinct address (or that the old slot is stopped first) and that the health probe targets the new container.
- **Confidence**: likely (certain that the health probe aliases to the old container; likely on netavark rejecting the duplicate static IP)

#### Regenerated Corefile is never applied to an already-running resolver
- **Location**: `crates/core/src/system/resolver/startup.rs:315-319` and `crates/core/src/system/resolver/config.rs:20-47`
- **Bug**: `ensure_resolver_running` rewrites the Corefile every tick, but when the active container is running, healthy, and on the right image it returns immediately; nothing compares the new Corefile against what the container was started with, and the generated Corefile contains no `reload` plugin, so CoreDNS never re-reads the bind-mounted file. Config changes therefore only take effect via an image change or idle teardown.
- **Failure scenario**: Apps are installed; operator restarts the daemon with a different `--dns-upstreams` list (or a `--nat64` change that flips `dns64`/`translate_all`). The resolver container — a transient systemd unit that survives daemon restarts — keeps serving with the old forward targets/DNS64 config indefinitely, while docs/networking.md promises "regenerate the Corefile if upstreams or NAT64 status changed".
- **Severity**: medium — wrong DNS behaviour after any resolver-config change, but only triggered by operator flag changes across daemon restarts.
- **Fix testability**: moderate — with the stub runtime, assert that a running resolver is restarted (or reloaded) when the generated Corefile content differs from the previous tick's.
- **Confidence**: likely

#### Pod /64 network prefixes collide: all static Jobs share one prefix; scaled replicas birthday-collide on one UUID byte
- **Location**: `crates/core/src/system/translate/proxy.rs:82-87` (`pod_network_prefix`)
- **Bug**: The pod /64 is derived from only the kind byte (byte 6) and `uuid[0]` (byte 7) — 8 bits of instance entropy. Static Jobs are constructed with a nil UUID (`identity.rs`), so *every* static Job on the node, across all apps, derives the identical `fd5e:XXYY:ZZWW:0500::/64`. Scaled/singleton instances of the same kind collide whenever two UUIDs share their first byte (birthday problem: ~52% for 20 replicas). Each instance gets its own uniquely-named podman network (`seedling-<display_name>`) but with the same subnet; podman/netavark refuses to create a second network with an in-use subnet, so the second pod fails to start (and even if it were created, two bridges would carry the same connected /64, and mount-DNAT rules gated on `pod_prefix` in `nft.rs` would match the wrong pod's traffic).
- **Failure scenario**: Two apps each define a Job; both run (or one job's network lingers) → the second `podman network create` fails with a subnet-in-use error and that job can never start. Or: scale a deployment to 20 replicas → better-than-even odds one replica's network creation permanently fails.
- **Severity**: high — deterministic outage for any node running more than one static Job, and probabilistic startup failure at the scale this deployment is preparing for.
- **Fix testability**: easy — pure unit test: derive prefixes for two Job instances (nil UUIDs) or two instances with equal `uuid[0]` and assert uniqueness after the fix.
- **Confidence**: likely (the collision math is certain; the exact failure mode depends on netavark's duplicate-subnet rejection, which is standard behaviour)

#### `spec_hash` does not cover `stop_signal` / `stop_timeout_secs`, so changing them never restarts the container
- **Location**: `crates/core/src/system/translate/container.rs:229-241` (`spec_hash`) and `98-223` (`podman_args`)
- **Bug**: `spec_hash` hashes the podman argv, but `stop_signal`/`stop_timeout_secs` are applied as systemd unit properties (`kill_signal`/`timeout_stop_secs` in `actuator.rs:223-236`) and never appear in the argv. The hash therefore doesn't change when they do, violating `r[update.spec-hash]` ("captures its full configuration") — the reconciler sees the running instance as up to date and the new stop behaviour is never applied.
- **Failure scenario**: Operator changes `.stop_signal("SIGINT")` to `.stop_signal("SIGTERM")` or adjusts `.stop_timeout()` and pushes the app. No restart occurs; subsequent stops/replacements still use the old signal/timeout until an unrelated spec change forces a restart.
- **Severity**: medium — wrong behaviour on a real but narrow update path (graceful-shutdown tuning silently not applied).
- **Fix testability**: easy — unit test: two `ContainerSpec`s differing only in `stop_signal` (or `stop_timeout_secs`) must produce different hashes.
- **Confidence**: likely

#### A plain-HTTP ingress is silently dropped when another ingress on the same hostname uses TLS
- **Location**: `crates/core/src/system/translate/proxy.rs:207-232` (`ensure_vhost`) with `crates/core/src/system/caddy/config.rs:59-67`
- **Bug**: Vhosts are keyed by hostname only and `tls_acme` is OR-ed across all ingresses for that hostname. In `build_caddy_config` the HTTP server only carries routes for vhosts with `!tls_acme`, so once any ingress for `x.com` terminates TLS, the merged vhost's routes — including those declared by a plain-HTTP ingress on port 80 — are emitted only into the HTTPS server. If no other vhost puts routes in the HTTP server, Caddy doesn't even bind the HTTP port, while nftables still DNATs that port to Caddy.
- **Failure scenario**: App A declares `x.com` HTTP on 80; app B (or the same app) declares `x.com` HTTPS on 443. Requests to `http://x.com:80` get connection-refused (or a 404 if another HTTP vhost exists), with no fault raised.
- **Severity**: medium — silent breakage of a declared ingress, but requires two ingresses sharing a hostname with mixed TLS.
- **Fix testability**: easy — unit test on `build_proxy_config` + `build_caddy_config` with one TLS and one plain ingress for the same hostname, asserting the HTTP route survives.
- **Confidence**: likely

#### HTTP/HTTPS routes are not scoped to their declared ingress port; same-hostname multi-port ingresses shadow each other
- **Location**: `crates/core/src/system/caddy/config.rs:28-75` and `crates/core/src/system/translate/proxy.rs:119-169`
- **Bug**: All HTTP listener ports share one `seedling_http` server (likewise HTTPS) whose routes match on `host` only — never on the listener port — and `ensure_vhost` merges every ingress with the same hostname into one route list regardless of port. So each vhost is reachable on *every* ingress port of that protocol class, and two ingresses for the same hostname on different ports both emit `/` routes into one vhost, where the first terminal route wins on both ports.
- **Failure scenario**: `x.com:80` → service A and `x.com:8080` → service B (e.g. public site + admin endpoint): both ports serve service A; service B is unreachable. Also, any client can reach `x.com`'s app via port 8080 (declared only for `y.com`) by setting the Host header — cross-ingress port exposure.
- **Severity**: medium — deterministic misrouting whenever a hostname spans two ports; unintended port exposure otherwise.
- **Fix testability**: easy — unit test on `build_caddy_config` with two same-hostname different-port vhosts, asserting per-port servers or port-scoped matchers.
- **Confidence**: certain (behaviour), likely (that the configuration is expressible in BSL — ingress identity is `hostname:port`, which permits it)

#### Same port declared as both HTTP and HTTPS produces a Caddy config that fails to load, taking down all ingress
- **Location**: `crates/core/src/system/caddy/config.rs:31-75`
- **Bug**: `seedling_https` and `seedling_http` servers are built independently from the listener set; nothing prevents both from listing `:P` in `listen`. Caddy rejects a config where two servers bind the same address, and the `POST /config/` is all-or-nothing — one bad pair invalidates every route and every cert policy.
- **Failure scenario**: Ingress `a.com` plain HTTP on 8080 and ingress `b.com` HTTPS on 8080 (distinct resources, since identity is `hostname:port`). The next config POST returns an error; Caddy keeps whatever config it had (or none after a restart replay), and *all* ingresses stop updating/serving, not just the conflicting pair.
- **Severity**: medium — edge-case operator input, but blast radius is total ingress outage rather than a scoped fault.
- **Fix testability**: easy — unit test asserting `build_caddy_config` never emits the same listen address in two servers (or that the conflict is surfaced as a per-ingress fault upstream).
- **Confidence**: likely

#### `jool instance display <name>` is not a valid jool invocation, so idle teardown silently leaves the translator installed
- **Location**: `crates/core/src/system/jool.rs:44-47` (`instance_exists`), used by `remove_instance` at 82-98
- **Bug**: Jool's `instance display` mode takes no instance-name positional argument (it lists all instances); passing `seedling` makes the command exit non-zero, so `instance_exists` returns `Ok(false)` and `remove_instance` returns `Ok(())` without ever running `jool instance remove`. `teardown_nat64` thus reports success while the translator instance stays active, contradicting the documented idle behaviour ("an idle node carries no stale translator state").
- **Failure scenario**: Node goes idle → reconciler calls `teardown_nat64` → no-op; `jool instance display` still shows the `seedling` instance and the netfilter hook keeps translating.
- **Severity**: low — the leak is benign (re-activation is idempotent via the EEXIST path), but the teardown contract is silently unmet and errors can never surface.
- **Fix testability**: hard — needs a host with the jool userspace tools (or a fake `jool` binary on PATH asserting the argv).
- **Confidence**: possible (depends on the installed jool CLI's argument handling; the failure mode is silent either way)

#### NAT64 detection accepts any IPv6 result without excluding the canonical `ipv4only.arpa` addresses
- **Location**: `crates/core/src/system/nat64.rs:31-54` (`detect_external_nat64`)
- **Bug**: The spec (`r[infra.nat64.detection]`, docs/spec/runtime.md:1317) requires treating an AAAA as synthetic only if it is *outside* the canonical `192.0.0.170`/`192.0.0.171` addresses; the code treats any `is_ipv6()` socket address as proof of external NAT64. A resolver stack that returns IPv4-mapped results (`::ffff:192.0.0.170`) or an interception middlebox that answers AAAA queries indiscriminately causes a false positive, and seedling then declines to stand up its own translator on a host that has no NAT64.
- **Failure scenario**: `auto` mode on a host whose libc/resolver returns v4-mapped IPv6 for `ipv4only.arpa` → "existing NAT64 detected" → no jool instance, no `dns64` in the Corefile → pods cannot reach any IPv4-only destination.
- **Severity**: low — requires an unusual resolver stack, but the consequence (no IPv4 reachability from all pods) is severe when it hits, and it is an explicit spec deviation.
- **Fix testability**: easy — factor the classification over a list of `SocketAddr`s and unit-test mapped/canonical vs. `64:ff9b::…` inputs.
- **Confidence**: possible (real-world trigger is rare; the spec deviation itself is certain)

#### Resolver address is `…fd00::35`, not the documented `…fd00::53`
- **Location**: `crates/core/src/system/resolver.rs:32-39` (`resolver_addr`)
- **Bug**: `addr[15] = 53` writes decimal 53 (0x35), producing `fd5e:XXYY:ZZWW:fd00::35`. docs/networking.md and the code comment both state the container listens at `::53` (hex), "chosen to match the DNS port". The address is used consistently everywhere in code so nothing breaks functionally, but the documented well-known address — which operators may firewall or debug against — is wrong, and the stated memorability property doesn't hold.
- **Failure scenario**: Operator adds a firewall exception or monitoring probe for `<prefix>:fd00::53` per the docs; it never matches the real resolver at `::35`.
- **Severity**: low — internally consistent, but a documented interface value diverges from reality.
- **Fix testability**: easy — unit test asserting `resolver_addr(...)` ends in `0x0053` (fix must also handle already-deployed nodes whose pods were started with `--dns …::35`).
- **Confidence**: certain (about the mismatch)

#### IPv6/IPv4 egress probes accept default routes from any routing table, not just main
- **Location**: `crates/core/src/system/netinfo.rs:110-167` (`has_default_v4_route`, `has_default_v6_route`)
- **Bug**: The RTM_GETROUTE dump returns routes from all tables, and unlike `data_plane/routes.rs` (which filters `effective_table == 254`), these probes accept a `/0` unicast route from any table. docs/networking.md specifies "a default IPv6 unicast route (::/0) in the main table". A default route confined to a policy-routing table that pod-forwarded traffic can't reach (e.g. a per-interface table populated by systemd-networkd `RouteTable=`, or a source-scoped VPN table) makes `detect_ipv6_egress` return true.
- **Failure scenario**: IPv4-only host with a v6 default route only in a policy table whose rules don't match pod-sourced ULA traffic → `translate_all` is not emitted → pods receive real AAAAs, send native v6 into a dead path, and dual-stack destinations time out instead of going through NAT64.
- **Severity**: low — requires a policy-routing setup, but the resulting failure (pod flows blackholing) is hard to diagnose.
- **Fix testability**: easy — extract the route-classification into a pure function over `RouteMessage`s (reusing `effective_table`) and unit-test a table-51820 default route being ignored.
- **Confidence**: possible

#### HTTP→HTTPS redirect targets the globally-first HTTPS port rather than the vhost's own HTTPS port
- **Location**: `crates/core/src/system/caddy/config.rs:282-288` (`redirect_route`), fed by `translate/proxy.rs:224-229` (`HttpRedirect` discards the target port)
- **Bug**: `HttpRedirect` records only `from_port` and `code`; the redirect's Location is built from `https_ports.first()` — the lowest HTTPS port on the whole node — not the port of the ingress that declared the redirect. It only works today because the shared `seedling_https` server incidentally serves every vhost on every HTTPS port.
- **Failure scenario**: `a.com` HTTPS on 443 with redirect, `b.com` HTTPS on 8443 with redirect: `http://b.com:8080` redirects to `https://b.com` (port 443) — a port `b.com` never declared and which only answers because of the port-scoping flaw above; fixing that flaw would turn this into a broken redirect.
- **Severity**: low — currently masked, but the wrong value is persisted into client-visible Location headers and blocks fixing the port-scoping bug independently.
- **Fix testability**: easy — unit test with two HTTPS ports asserting each vhost's redirect targets its own port.
- **Confidence**: certain (about the selection logic)


### 15. Host integration: podman, systemd, volume_store, confined_write

#### Parent-directory creation in confined_write is not confined — symlink escape via mkdirat
- **Location**: `crates/core/src/system/confined_write.rs:79`
- **Bug**: The module contract says "Every write is kernel-confined", but only the final `openat2` uses `RESOLVE_BENEATH`. The parent-directory loop calls `mkdirat(dir_fd, accum, ...)` with plain path resolution, which follows symlinks in intermediate components. A symlink inside the root pointing outside it causes the daemon (root) to create directories outside the confined root; the subsequent `openat2` fails with `Escape`, but the out-of-root directories have already been created as a side effect.
- **Failure scenario**: Root contains `data -> /etc` (a symlink placed by a prior write or an untrusted volume). `write(root, "data/newdir/f.txt", …)` runs `mkdirat` on `data`, then `data/newdir`, creating `/etc/newdir` outside the root before the final open is rejected. Directory-creation side effects leak outside confinement.
- **Severity**: medium — writes are still confined, but directory-creation side effects escape the root, which the module explicitly promises against; requires a pre-existing in-root symlink.
- **Fix testability**: easy — unit test: create a symlink in the tempdir root pointing to a second tempdir, call `write(root, "link/sub/f.txt", …)`, assert no directory was created under the target. Fix by resolving each mkdirat step with `openat2(RESOLVE_BENEATH)` on the parent fd (mkdirat has no resolve-flags variant, so open each segment beneath the fd).
- **Confidence**: likely

#### `remove_dir_all` on a symlinked volume path deletes the symlink target's contents
- **Location**: `crates/core/src/system/volume_store.rs:117,301,368`
- **Bug**: `remove`, `confirm_delete_held`, and `remove_site` fall back to `tokio::fs::remove_dir_all(&path)` for non-btrfs paths. If `path` is itself a symlink to an external directory, `remove_dir_all` follows it and recursively deletes the target's contents. Volume/site names are joined directly (`volumes_dir.join(name)` / `join(format!("site-{name}"))`) with no symlink check.
- **Failure scenario**: A volume directory `{data}/volumes/site-foo` is a symlink (e.g. operator relocated storage, or an attacker with write access to the volumes dir). Removing site `foo` deletes the contents of wherever the symlink points, not just the volume.
- **Severity**: medium — data loss outside the intended volume, but requires the on-disk path to be a symlink, which the normal create path never produces.
- **Fix testability**: moderate — create a symlink at the site path in a tempdir-backed `VolumeStore`, call `remove_site`, assert the target dir survives. Fix by lstat-ing / rejecting symlinks or using `remove_file` when the path is a symlink.
- **Confidence**: possible

#### Legacy volume migration can silently merge/overwrite data if canonical path pre-exists and rename semantics differ
- **Location**: `crates/core/src/system/volume_store.rs:76-95`
- **Bug**: `migrate_legacy` handles a pre-existing `canonical_path` by removing it if empty or holding it if non-empty, then `rename(legacy, canonical)`. But `is_empty` only checks the first directory entry via `read_dir().next_entry()`; if the canonical dir is a btrfs subvolume, `remove(canonical)` (which detects btrfs and runs `btrfs subvolume delete`) is correct, but the subsequent `rename` of a plain legacy dir onto a path where a subvolume was just deleted is fine. The real issue: `read_dir` errors other than success are propagated, but a canonical path that is a *file* (not a dir) makes `read_dir` fail with `NotADirectory`, returning `Err` and aborting migration rather than holding it — leaving both paths intact and the app pointing at the wrong one. Minor, but the "nothing is silently dropped" guarantee in the doc comment does not hold for non-directory canonical paths.
- **Failure scenario**: `{data}/volumes/<app>-volume-<name>` exists as a regular file (corrupt state). Migration errors out; operator sees an I/O error instead of the promised hold-and-migrate.
- **Severity**: low — edge case requiring a corrupt canonical path; fails loudly rather than corrupting.
- **Fix testability**: easy — unit test with a file at the canonical path.
- **Confidence**: possible

#### `podman exec` argv can inject podman flags when a command argument begins with `-`
- **Location**: `crates/core/src/system/podman.rs:1013-1021`
- **Bug**: `exec_command` builds `podman exec [--env …] <name> <argv…>` without a `--` separator before `argv`. Since the user-controlled `argv` elements follow the container name positionally, podman treats a leading-dash argv element after the name... actually podman stops option parsing after the container name, so this is safe for argv. However `name` is passed as a positional after the `--env` options; a name beginning with `-` would be parsed as a podman flag. Names are validated elsewhere, so this is low-risk, but there is no `--` guard.
- **Failure scenario**: If an instance display name ever began with `-` (guarded by name validation today), `podman exec --env X=Y -weird cmd` would misparse. Defence-in-depth only.
- **Severity**: low — names are validated to be bsl-shaped (alphabetic first char), so not currently reachable.
- **Fix testability**: easy — assert argv construction inserts `--`.
- **Confidence**: possible

#### Signal delivery to a non-running container returns success, but spec requires silently skipping non-running instances — TOCTOU only skips *missing*, not *stopped*
- **Location**: `crates/core/src/system/podman.rs:460-476`
- **Bug**: `signal_container_impl` maps a 404/no-such-container to `Ok(false)` (skipped) but any other outcome to `Ok(true)`. Podman's kill endpoint returns an error (HTTP 409 "container is not running") when the container exists but is stopped/created. That error is neither 404 nor "no such container", so it maps via `map_api_err` to `Err(...)`. Per spec `l[rt.signal]`, "Container instances that are not running are silently skipped (no error)". The caller (`barrier/runtime.rs:1092`) only logs the error and continues, so the operation does not fail — but it logs a spurious warning and, more importantly, the intent "silently skipped" is violated: a stopped instance yields an error path rather than the skip path.
- **Failure scenario**: `rt.signal` targets a deployment where one replica is in `created`/`exited` state. Podman returns 409 not-running; code returns `Err`, producing a warning log for what the spec says should be a silent skip.
- **Severity**: low — no operation failure (caller swallows the error), but violates the spec's silent-skip contract and produces noise.
- **Fix testability**: hard (needs real podman to produce the 409) or moderate with a stubbed client; alternatively treat "not running"/"is not running" 409 bodies like the not-found case.
- **Confidence**: likely

#### IPv6 gateway derivation assumes /64-or-larger and clobbers host bits, producing wrong gateway for non-aligned prefixes
- **Location**: `crates/core/src/system/podman.rs:267-270`
- **Bug**: `create_network_impl` computes the gateway by taking `prefix.network()` octets and setting only byte 15 to 1 (`gw_bytes[15] = 1`). For an IPv6 prefix whose network address already has non-zero low bytes cleared by `network()` this is fine, but it hard-codes `::1` as the host suffix. If two seedling pod networks are carved from prefixes that share the same lowest byte pattern but differ higher up it is fine; the real defect is when `prefix` is longer than /120 (host part < 8 bits) — the gateway `...::1` may fall *outside* the subnet, and netavark rejects it. The IPv4 path has the same assumption (`gw4[3] = 1`, line 284) requiring the subnet be at least a /24-equivalent host space.
- **Failure scenario**: A pod network allocated a /125 IPv6 prefix (8 addresses, host range `..0`–`..7`); gateway is set to `..1` which is inside, OK. But a /120 with network base `…:aa00` → gateway `…:aa01`? byte 15 forced to `1` overwrites `00`→`01`, still inside. The break case: prefix base whose byte 15 is non-zero after masking is impossible; however for a prefix shorter than /120 where the intended gateway convention differs, `::1` silently differs from operator expectation. Practically this only misbehaves for prefixes longer than /120 where forcing byte 15 to 1 can land outside the range (e.g. /126 with base `…fc`: `…01`? base byte15 = 0xfc, forced to 1 → `…01`, which is *below* the network base and outside the subnet).
- **Severity**: low — seedling allocates /64-aligned pod prefixes in practice, so not reached; only a latent bug if prefix sizing changes.
- **Fix testability**: easy — unit test `create_network`'s gateway computation for a /126 prefix and assert the gateway is within the subnet.
- **Confidence**: possible

#### `list_images_impl` filters `<none>:<none>` tags but not per-element `<none>` in repo_tags from older podman
- **Location**: `crates/core/src/system/podman.rs:533-538`
- **Bug**: Tags are filtered against the exact literal `"<none>:<none>"`. Some podman/libpod versions report untagged images with `repo_tags` containing the entries `"<none>"` (name only) or omit them. Only the fully-qualified `<none>:<none>` sentinel is stripped, so a bare `<none>` tag would pass through as if it were a real tag.
- **Failure scenario**: libpod returns `repo_tags: ["<none>"]` for a dangling image; `list_images` reports a tag literally named `<none>`, which downstream image-management logic may treat as a usable reference.
- **Severity**: low — depends on podman version emitting the bare form; modern libpod uses `<none>:<none>`.
- **Fix testability**: easy — unit test the filter with `"<none>"`.
- **Confidence**: possible


### 16. Daemon and ctl crates

#### Daemon can start with zero OI listeners when `--interface` fails to resolve (doc says fatal)
- **Location**: `crates/daemon/src/main.rs:1565` (with arg contract at `crates/daemon/src/main.rs:90-94`)
- **Bug**: The `--interface` flag's documented contract is "Failure to resolve a named interface is fatal", but `resolve_oi_addrs` only logs a warning and continues. If every named interface is missing/addressless and no `--listen` is given, the returned address list is empty; `oi::run` (crates/core/src/oi/server.rs:169-182) happily iterates zero addresses and returns Ok, so the daemon logs "seedling ready" while listening on nothing.
- **Failure scenario**: Boot-time race — daemon starts before the NIC/tailscale interface is up (or operator typos the interface name) → warning buried in logs, daemon runs but is unreachable by every client until manually restarted.
- **Severity**: high — silent management-plane outage in a realistic deployment race, and directly contradicts the flag's stated behaviour.
- **Fix testability**: easy — unit test `resolve_oi_addrs` with a nonexistent interface name and assert it aborts (or returns error) instead of returning an empty vec.
- **Confidence**: certain

#### UDP port forward terminates permanently on a single oversized datagram
- **Location**: `crates/ctl/src/forward.rs:273`
- **Bug**: `if client.send_datagram(pkt).is_err() { break; }` treats every send error as fatal. quinn's `send_datagram` returns `SendDatagramError::TooLarge` for payloads exceeding `max_datagram_size()` (path-MTU bound, typically ~1200-1450 bytes), so one large local datagram tears down the whole forward loop. The client also ignores the `max_udp_payload` field the server returns for exactly this purpose (spec `i[forward.mtu]`, `docs/spec/interface.md:558-563`); the spec's model is that over-limit payloads are dropped and reported, not session-fatal.
- **Failure scenario**: `ctl apps forward <app> <svc> 53 --proto udp`, then a local resolver sends an EDNS query/response of ~4KB → `send_datagram` returns TooLarge → forward exits mid-session with a stats summary, no error explaining why.
- **Severity**: high — any UDP workload with datagrams above path MTU (DNS with EDNS, QUIC, game/VPN traffic) kills the forward on first packet; only tiny-datagram protocols work reliably.
- **Fix testability**: moderate — needs a quinn client/server pair (or the stub OI harness) to feed an oversized datagram and assert the loop drops it and continues.
- **Confidence**: certain

#### `--trust-any events` can never connect (empty fingerprint pinned on resubscribe)
- **Location**: `crates/ctl/src/main.rs:271` (consumed at `crates/ctl/src/main.rs:409-417`, `crates/ctl/src/subscribe.rs:23-29`)
- **Bug**: In the `trust_any` branch `resolved_fingerprint` is set to `String::new()`. `Command::Events` passes it to `subscribe()`, which opens a fresh connection with `ClientAuth::Fingerprint("")`. `FingerprintVerifier` compares the 64-hex server fingerprint against `""` (crates/protocol/src/client.rs:120-127), which never matches, so every connect fails.
- **Failure scenario**: Dev build, `seedling-ctl --trust-any events` → the initial connection (TrustAny) succeeds, then the events resubscribe loop retries with the empty pinned fingerprint for the full 300 s reconnect window and exits 1.
- **Severity**: low — `--trust-any` only exists under `debug_assertions`, but the command combination is completely broken.
- **Fix testability**: moderate — needs a test OI server; assert `events` under trust-any either reuses TrustAny auth or the probed fingerprint.
- **Confidence**: certain

#### Dynamic resources preserved for a replay are never torn down when the replay is abandoned
- **Location**: `crates/daemon/src/main.rs:458-468` (preservation) and `crates/daemon/src/main.rs:1345-1453` / `crates/daemon/src/main.rs:1496-1543` (abort paths)
- **Bug**: Startup orphan cleanup deliberately skips dynamic resources whose `operation_id` matches the persisted current operation, expecting the replay to adopt them. But `replay_interrupted_operation` has several abort paths (app unregistered, missing/undecryptable params → `revert_install_and_fault`, phase mismatch, scheduler refusal) that clear `current_operation` without stopping those units/containers/networks or deleting their `dynamic_resources` rows. The comment at main.rs:487-489 confirms the reconciler ignores these resources, so nothing else cleans them.
- **Failure scenario**: Daemon crashes mid-install (containers started via `rt.start`), restarts, and param decryption fails (e.g. `seedling.db.key` replaced) → app reverts to NotInstalled and a fault is filed, but the install's containers keep running untracked until the *next* daemon restart.
- **Severity**: medium — needs a crash plus a replay failure, but the result is unmanaged workloads running while the app claims NotInstalled.
- **Fix testability**: moderate — stubbed System backend + DB with a `current_operation` row and matching `dynamic_resources` rows whose params can't be decrypted; assert the stub containers/units are stopped.
- **Confidence**: likely

#### TCP forward tears down the connection on half-close, dropping the response
- **Location**: `crates/ctl/src/forward.rs:191-216`
- **Bug**: The per-connection relay breaks out of the loop on `tcp_read` returning `Ok(0)` (local client closed its write side) and on `fwd_recv` returning `Ok(None)` (service finished its write side), then finishes/drops both directions. Half-close semantics are not honoured: local write-shutdown should only `finish()` the QUIC send side while continuing to relay service→client data, and vice versa.
- **Failure scenario**: Forward a TCP service; the local client sends a request and calls `shutdown(SHUT_WR)` while waiting for the reply (classic `nc -N`, some RPC/batch protocols) → relay breaks on EOF, the QUIC stream is finished and the TCP socket dropped before the service response arrives; the client sees a connection reset with no data.
- **Severity**: medium — correct behaviour for protocols using half-close is silently broken; plain request/response protocols that don't half-close are unaffected.
- **Fix testability**: moderate — loopback TCP client that half-closes plus a stub forward server; assert the response still arrives.
- **Confidence**: certain

#### SIGTERM not handled by forwards, event subscriptions, or log follow, contrary to the graceful-shutdown spec
- **Location**: `crates/ctl/src/forward.rs:239` and `crates/ctl/src/forward.rs:311`; `crates/ctl/src/subscribe.rs:129`; `crates/ctl/src/logs.rs:84`
- **Bug**: Spec `i[ctl.graceful-shutdown]` and `i[ctl.logs.follow-interrupt]` (docs/spec/interface.md:1119-1138) require clean exit on SIGINT **or SIGTERM** for port forwards, event subscriptions, and log follow. All three only select on `tokio::signal::ctrl_c()` (SIGINT). `subscribe.rs:69` is even annotated `i[impl ctl.graceful-shutdown]`. SIGTERM takes the default disposition: immediate process death, no control-stream close, no stats summary.
- **Failure scenario**: A supervised forward (systemd unit, tmux kill, `kill <pid>`) receives SIGTERM → process dies without closing the control stream; the server-side forward lingers until the 30 s QUIC idle timeout, and the spec-mandated stats summary is never printed.
- **Severity**: low — cleanup still happens eventually via idle timeout; behaviour deviates from spec but self-heals.
- **Fix testability**: moderate — spawn the CLI against a stub server, send SIGTERM, assert graceful close/summary on stderr.
- **Confidence**: certain

#### Startup cleanup ordering leaks pod networks of orphaned app containers
- **Location**: `crates/daemon/src/main.rs:738-809` (network scan) vs `crates/daemon/src/main.rs:811-866` (orphan app-container scan)
- **Bug**: The orphan pod-network scan treats any network whose backing container still exists as live. Containers labelled `seedling.app` for apps that are no longer registered still exist at that point — they are only removed by the later container scan. Their `seedling-<display>` networks are therefore classified as live and survive; nothing at runtime removes them (the reconciler's network GC at reconcile.rs:1151 covers only stray shells). The shell-container scan explicitly runs before the network scan for exactly this reason; the app-container scan does not.
- **Failure scenario**: App deregistered while daemon was down (or daemon killed mid-remove) → restart removes the stale containers but their podman networks persist, holding subnet allocations until the *next* restart.
- **Severity**: low — bounded leak, self-heals one restart later; may cause subnet-pool pressure or name collisions in the interim.
- **Fix testability**: moderate — stub container runtime pre-seeded with an unregistered-app container plus matching network; assert both are removed in one startup.
- **Confidence**: certain (ordering), likely (practical impact)

#### `tls certs upload-manual --cert - --key -` silently sends an empty key
- **Location**: `crates/ctl/src/tls.rs:471-482` (used at `crates/ctl/src/tls.rs:367-380`)
- **Bug**: Both `--cert` and `--key` document `-` for stdin. `read_pem_arg` reads stdin to EOF; when both are `-`, the first call consumes everything and the second returns `Ok("")` — an empty `key_pem` is sent to the server with no client-side error. Same trap for `csr upload-cert --cert -` combined with anything else reading stdin.
- **Failure scenario**: `cat cert.pem key.pem | seedling-ctl tls certs upload-manual example.com --cert - --key -` → server rejects with a key-mismatch/parse error that points the operator at the wrong cause (or the cert half is misparsed as containing both blocks).
- **Severity**: low — fails closed with a confusing error; no bad state stored.
- **Fix testability**: easy — unit-level: detect both args being `-` and error, testable with a pure check.
- **Confidence**: certain

#### Backup-strategy missing-volume guard fails open when the pre-check requests error
- **Location**: `crates/ctl/src/backups.rs:299-307`
- **Bug**: `check_missing_volumes` uses `.ok()?` on both list requests, so any request failure returns `None`, which the callers interpret as "all volumes exist" and proceed to create/update the strategy. Spec `i[ctl.backup.strategy.allow-missing]` (docs/spec/interface.md:1162-1163) makes this check the mandatory abort gate ("must abort with an error before sending the request"), and the server side does not validate volume existence at create time (crates/core/src/oi/handler/backups.rs:141-176) — volumes only fail per-run later.
- **Failure scenario**: Transient request failure (or permission error) during the pre-check → strategy referencing a typo'd volume is created without `--allow-missing`; the mistake only surfaces as failed backup runs at 3am.
- **Severity**: low — requires a pre-check failure to coincide with a bad volume id, but it silently defeats the only existence guard.
- **Fix testability**: moderate — needs a stub client whose list endpoints error; assert the CLI aborts instead of proceeding.
- **Confidence**: certain

#### `apps install` silently discards params without `=`
- **Location**: `crates/ctl/src/apps.rs:386-393`
- **Bug**: Install params are parsed with `filter_map` over `splitn(2, '=')`, dropping any argument lacking `=` with no warning — inconsistent with `apps action`/`apps shell`, where a bare key maps to `true` per spec `i[ctl.action.params]`/`i[ctl.shell.params]`.
- **Failure scenario**: `ctl apps install myapp adminpassword` (typo for `admin=password`, or a user expecting the action-style bare-flag syntax) → the param vanishes; the install proceeds with a default value or fails with an unrelated "missing param" server error.
- **Severity**: low — user error required, but the silent drop hides it.
- **Fix testability**: easy — pure unit test on the parsing closure.
- **Confidence**: certain

#### `ingresses site update --description ""` sets an empty string, contradicting its own help text
- **Location**: `crates/ctl/src/ingresses.rs:50-52` (help), `crates/ctl/src/ingresses.rs:168-172` (encoding)
- **Bug**: The flag's help says "pass an empty string to clear", but the code sends `"description": ""`, and the server's double-Option handling (crates/core/src/oi/handler/ingresses.rs:314-316, 365-378) only treats JSON `null` as clear — `""` is stored as an empty-string description. Only `--clear-description` sends `null`.
- **Failure scenario**: Operator runs `... update x --description ""` expecting the description removed → row now holds `""`, which renders as an empty description rather than "none" in listings/UI.
- **Severity**: low — cosmetic data difference; a correct flag exists.
- **Fix testability**: easy — unit test the request body built for `--description ""` (or fix the help text).
- **Confidence**: certain

#### `events` exits 0 when the server rejects the subscription
- **Location**: `crates/ctl/src/subscribe.rs:93-98` (and `crates/ctl/src/subscribe.rs:48`)
- **Bug**: When `/events/subscribe` returns an `error` response, `run_subscribe_session` prints it and returns `SessionOutcome::GracefulClose`, which `subscribe()` treats as a clean exit — process exit code 0 despite the subscription failing.
- **Failure scenario**: Scripted monitoring (`seedling-ctl events || alert`) — server rejects the subscribe (e.g. future authz or param validation) → CLI prints the error JSON to stderr and exits 0, so the wrapper never alerts.
- **Severity**: low — currently hard to trigger since subscribe has no failure modes beyond transport, but the exit code is wrong when it does.
- **Fix testability**: moderate — stub server returning an error object; assert non-zero exit.
- **Confidence**: certain


### 17. Web crate

#### Advertised WebTransport port is hard-coded to the default whenever explicit listen addresses are used
- **Location**: `crates/web/src/main.rs:261-265`
- **Bug**: `wt_port` is set to `DEFAULT_WT_PORT` (7893) whenever `wt_listen` is non-empty, instead of the port the WT server actually binds. `resolve_bind_addrs` passes explicit `SocketAddr`s through with their own ports, so the bound port and the advertised port (used in `wt_url` in `auth::handle_connect` and in the CSP `connect-src` in `http.rs`) diverge. Note `wt_listen` here is also the fallback from the HTTP `--listen` set (`wt_bind_sources`), so plain `--listen` deployments are affected too.
- **Failure scenario**: `seedling-web --wt-listen '[::]:4443'` binds WT on UDP 4443 but every `/connect` response says `wt_url = https://host:7893/...` and the CSP only allows `:7893`; the browser can never open the WT session. Same for `--listen 10.0.0.1:8080` with no WT flags (WT binds UDP 8080, advertises 7893). Only the blessed port 7893 works (e.g. `docs/plans/seedling-web-as-app.md` uses `--listen [::]:7893`).
- **Severity**: high — a documented flag combination (`--wt-listen`, see docs/deploying.md:145) silently breaks all WebTransport connectivity.
- **Fix testability**: easy — pure unit test on the port-selection logic once extracted (assert advertised port equals the explicit addr's port).
- **Confidence**: certain

#### Certificate rotation swaps only after expiry, serving an expired cert for up to an hour per rotation
- **Location**: `crates/web/src/wt_cert.rs:69` (with `crates/web/src/wt.rs:364-375`)
- **Bug**: `rotate_if_needed` promotes `next` only when `current.not_after <= now`, and the check runs on an hourly timer. Between the cert's real notAfter and the next tick (up to ~1h, plus the stored `not_after` being computed slightly after the cert was built), the endpoint keeps presenting an expired certificate, which browsers reject even under `serverCertificateHashes` (validity period is still checked). The spec (`docs/spec/web.md` w[wt.cert.rotation]) requires rotation *before* expiry. Additionally, because `next` is pre-generated 6 days before promotion with only 7 days validity, promoted certs alternate between ~6-day and ~1-day lifetimes, so this outage window recurs frequently.
- **Failure scenario**: cert expires at T; until the next hourly `rotate_if_needed` tick, every new WT handshake fails with a TLS validity error → no new web sessions (login succeeds but the live session never connects) for up to an hour, recurring every rotation cycle.
- **Severity**: high — recurring window where the primary transport is unavailable to all new sessions, and a direct spec violation.
- **Fix testability**: easy — unit test `rotate_if_needed` with `not_after` set shortly in the future and assert the swap happens before expiry (e.g. once `next` exists and expiry is within the tick interval).
- **Confidence**: certain (that the expired-cert window exists); likely (that all target browsers reject it — the W3C WebTransport spec mandates the validity check)

#### `/logs/stream` hangs forever when the daemon returns an error response
- **Location**: `crates/web/src/daemon.rs:233-266` (`start_log_stream`), consumed at `crates/web/src/wt.rs:240-261`
- **Bug**: `start_log_stream` reads the daemon's first response line but never inspects it, then blocks in `conn.accept_uni()`. The daemon's `/logs/stream` handler (crates/core/src/oi/server.rs:369-438) writes an `{"error":...}` line and finishes the bidi *without* opening a uni stream and *without* closing the connection, so `accept_uni` pends indefinitely on the still-open dedicated connection. The browser's request stream never receives any response (the `{"result":{}}` line is only written after `start_log_stream` returns).
- **Failure scenario**: browser requests logs with params the daemon rejects (`requirements_invalid`, unknown app, journal open failure) → the gateway task hangs, the dedicated QUIC connection and WT stream leak, and the log view spins forever; the daemon's actual error message is discarded.
- **Severity**: high — any daemon-side validation error on log streaming, a normal flow, produces an indefinite hang instead of an error.
- **Fix testability**: moderate — needs a stubbed daemon endpoint that answers the bidi with an error line; assert `start_log_stream` returns the error instead of hanging.
- **Confidence**: certain

#### `peek_request` size limit is checked only after reading the whole line into memory
- **Location**: `crates/web/src/proxy.rs:22-28`
- **Bug**: `buf.read_line(&mut first_line)` reads until a newline with no bound; the `MAX_FIRST_LINE` (1 MiB) check runs only after the entire line has been buffered, so the limit does not actually limit anything.
- **Failure scenario**: an authenticated WT client opens a bidi stream and sends gigabytes with no `\n`; the gateway buffers it all, growing memory unboundedly until OOM, despite the explicit 1 MiB guard.
- **Severity**: medium — memory exhaustion requires an authenticated client, but the written guard is entirely ineffective.
- **Fix testability**: easy — feed a `>1 MiB` newline-free prefix through a duplex stream and assert `peek_request` errors after ~1 MiB, not after the full payload (use `take`/limited reader).
- **Confidence**: certain

#### IPv6 Host headers produce a mangled `wt_url` and CSP `connect-src`
- **Location**: `crates/web/src/auth.rs:57` and `crates/web/src/http.rs:92`
- **Bug**: The port is stripped with `host.split(':').next()`, which turns an IPv6 host header like `[::1]:7894` (or `[::1]`) into `[`, producing `wt_url = "https://[:7893/wt?..."` and `connect-src 'self' https://[:7893`.
- **Failure scenario**: operator browses to `http://[::1]:7894/` (the daemon itself defaults to `[::1]`, so IPv6 loopback use is natural) → login succeeds but the returned WT URL is unparsable and the CSP names a garbage origin; the WebTransport session can never be established.
- **Severity**: medium — complete breakage, but only for IPv6-literal host access.
- **Fix testability**: easy — unit test hostname extraction with `[::1]:7894`, `[::1]`, `example.com:7894`.
- **Confidence**: certain

#### Shell cleanup guard can never fire: `exit_relay` returns `true` unconditionally
- **Location**: `crates/web/src/shell.rs:216-236`
- **Bug**: `exit_relay` returns the constant `true` whether or not a complete exit frame was received (e.g. `daemon_recv` errors immediately and `exit_buf` is empty), so `did_exit` is always true, `guard.mark_exited()` always runs, and the `ShellSessionGuard` `/shells/stop` cleanup is dead code. Additionally, the error returns in steps 1-7 (lines 91-187) happen before the guard is even created, so a shell already started on the daemon is never stopped if e.g. opening the WT uni streams fails.
- **Failure scenario**: daemon handshake succeeds but `open_prefixed_wt_uni` fails (browser navigating away mid-open) → the gateway returns without ever issuing `/shells/stop`; the daemon session lingers until its stdin bidi is torn down by connection close (on the *shared* daemon connection, which stays up). The guard that was written to cover exactly this can never trigger.
- **Severity**: low — the daemon also cleans up when it observes stdin EOF/stream reset, so the leak window is limited; but the guard logic is provably inert.
- **Fix testability**: easy for the constant-true defect (unit-test the exit-relay outcome for empty/partial frames); moderate to observe the leaked session end-to-end.
- **Confidence**: certain (that `did_exit` is always true); likely (that a real-world leak window results)

#### Event subscriber can receive duplicated events at subscribe time
- **Location**: `crates/web/src/event_broker.rs:31-49`
- **Bug**: `publish` pushes to the `recent` cache under the lock but calls `tx.send` *after* releasing it, while `serve_client` snapshots the cache and subscribes under the same lock. If a subscriber takes the lock between a publisher's cache push and its `tx.send`, the event appears both in the cached replay and in the live broadcast, so the client receives the line twice. (Concurrent publishers can also send in a different order than they cache.)
- **Failure scenario**: a `WebSessionStarted` is being published while a browser opens `/events/subscribe` → the browser processes the same event twice (e.g. duplicate rows/toasts in views driven purely by the event feed, per w[sessions.events]).
- **Severity**: low — small race window, and duplicates keyed by `session_id` are often idempotent, but the delivered stream is wrong.
- **Fix testability**: easy — deterministic unit test by splitting publish into cache-then-send with a hook, or by moving `tx.send` under the lock and asserting no duplicates under a concurrent loop.
- **Confidence**: certain (race exists); possible (user-visible impact)

#### Lagged event subscribers silently lose events with no gap signal or resync
- **Location**: `crates/web/src/event_broker.rs:70-72`
- **Bug**: On `RecvError::Lagged(n)` the broker only logs server-side and continues; the client is never told that `n` events were dropped and the `recent` cache is not replayed. The spec (w[sessions.events], w[routes.events]) has clients rely on the feed to keep state current "without polling", which a silent gap breaks permanently for that session.
- **Failure scenario**: a burst of >512 events (channel capacity) while a tab is backgrounded/throttled → the client misses e.g. `OperationCompleted`/`WebSessionStopped` events and shows stale state (operation forever "running", ghost sessions) until a manual reload.
- **Severity**: medium — wrong UI state in a plausible high-churn deployment, exactly the situation larger-scale use creates.
- **Fix testability**: moderate — unit test with a tiny broadcast capacity and a stalled subscriber; assert a gap marker/resync line is written to the stream.
- **Confidence**: certain (behaviour); likely (that it is a defect rather than accepted design — nothing in spec or client accounts for gaps)

#### Stale dispatcher's `cancel_all` can wipe registrations belonging to the new daemon connection
- **Location**: `crates/web/src/daemon.rs:75-89` and `203-224`
- **Bug**: `try_reconnect` shares one `UniRouter` between the old and new connections. The old connection's `run_uni_dispatcher` task calls `router.cancel_all()` whenever its `accept_uni` fails — which happens asynchronously when the old connection is dropped at `*self.inner.lock().await = new_client`. If that fires after a shell has registered stdout/stderr IDs on the *new* connection, those registrations are cleared and the oneshot receivers resolve as dropped. Concurrent `open_bi` failures can also run two `try_reconnect`s, compounding this.
- **Failure scenario**: daemon restarts; gateway reconnects; a user immediately opens a shell → `register_uni` entries are erased by the dying dispatcher's `cancel_all` → `handle_shell_start` logs "dispatcher dropped stdout receiver" and the shell fails/hangs despite a healthy connection.
- **Severity**: medium — wrong behaviour in the reconnect edge case; self-heals on retry but shells started in the window break.
- **Fix testability**: moderate — needs a harness with two fake connections sharing a router (or generation-tag the router per connection and unit-test that an old generation's cancel is a no-op).
- **Confidence**: likely

#### WebTransport bind failure is silently non-fatal, and a failed rebind after rotation kills WT permanently
- **Location**: `crates/web/src/wt.rs:32-37`
- **Bug**: When `Endpoint::server(config)` fails, `run_wt_server` logs and `return`s, leaving the process running with no WT listener on that address. The spec (w[bind]) says "Failure to bind any configured address at startup is a fatal error" — HTTP bind failure calls `process::exit(1)` (main.rs:355-360), but WT bind failure does not. The same `return` runs on the re-bind after every hourly cert rotation, so a transient UDP bind error at rotation time permanently disables WebTransport until the process is restarted.
- **Failure scenario**: WT port already taken at startup → HTTP serves the login page, `/connect` succeeds, but no WT endpoint exists; every session silently fails to connect. Or: rotation fires, rebind races with another socket user and fails once → WT dead forever with only an error log.
- **Severity**: medium — full loss of the primary transport with the process appearing healthy, in edge conditions.
- **Fix testability**: moderate — bind a UDP socket on the port first, start `run_wt_server`, assert fatal exit (startup) or retry (rotation).
- **Confidence**: certain

#### Duplicate `WebSessionStopped` emitted when a reaped session's connection later closes
- **Location**: `crates/web/src/wt.rs:289-302` (with `crates/web/src/wt.rs:333-360`)
- **Bug**: When the reaper drops a stale session it publishes `WebSessionStopped`; when the underlying WT connection finally closes, `handle_incoming` unconditionally publishes a second `WebSessionStopped` for the same `session_id` (the `remove` is a no-op but the event is still sent). The registry-remove + event-publish is not conditional on the session still existing.
- **Failure scenario**: laptop lid closes → heartbeats stop → reaper emits Stopped at 10 min → hours later the QUIC connection times out and a second Stopped for the same id hits the feed; clients tracking session lifecycle by events see a stop for a session they already removed (and the actor-activity feed records a spurious "closed web session" action refreshing that actor's `last_seen`).
- **Severity**: low — mostly harmless duplicate, but the event stream misrepresents lifecycle.
- **Fix testability**: easy — make `remove` return whether the entry existed and unit-test that no second event is published; registry is plain in-memory.
- **Confidence**: certain
