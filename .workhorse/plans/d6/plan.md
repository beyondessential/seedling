# Bring the code in line with the rewritten specs

Each of the card's issues is one theme of divergence between the code and the specs #C6 rewrote.
Each issue is worked on its own and committed on its own, with a test that fails without the fix.

## Issues

- [x] Secret values exposed
- [x] Throw when a pod binds an external service
- [x] Deployment scale ranges
- [x] Manual certificate uploads' key type
- [x] TLS and Canopy decisions
- [x] Ingress decisions
- [x] BSL throws, reserved names and defaults (and its correction)
- [ ] Failed reads become definite answers
- [ ] Database and internal failures reported with the wrong error code
- [ ] Malformed script input coerced or ignored
- [ ] Crash recovery and replay
- [ ] Faults out of line with the fault catalogue
- [ ] Action, shell and change-handler dispatch
- [ ] Host startup ordering, addressing and the web listener
- [ ] Resource and lifecycle behaviour the code does not enforce
- [x] Wire, event and ACME protocol shapes
- [ ] Operations: start action, persisted queue, unreadable state, intent-before-effect
- [x] OI error codes and request validation
- [ ] Host: new faults, key file, permissions, reserved prefix and env redaction
- [ ] Batch 5: runtime and fault code
- [ ] Batch 6: collections, validators, CLI, web, uninstall and image pulls
- [ ] Requirements restored by the fidelity review
- [ ] Blocking shell.attach, withholding-fault app status, rt.write to mapped external volumes
- [ ] Review round 2, part 1
- [ ] Review round 2, part 2
- [ ] VOLM, SVCM and INGM handlers
- [ ] Review round 3, part 1
- [ ] Review round 3, part 2

## Overlaps between issues

Several items appear in more than one issue.
Each is done once, in whichever issue is worked first, and the later issue notes it as already done.

- `audit_log_unavailable` and the audit task: Host faults (AUD#write-failures), round 2 part 1 (`spawn_audit_task`), round 3 part 2 (`runtime/audit.rs`).
- `/faults/clear` and `/faults/list` for `_system` and `seedling`: batch 5 item 1, round 2 part 2 (`oi/handler/faults.rs`).
- Removing `TlsPolicy::Tailscale`: TLS and Canopy decisions. The ingress decisions issue says to leave it until then.
- Uploaded key algorithms (`KeyType`): manual uploads issue, round 3 part 1.
- `redirect_preserve_path`: VOLM/SVCM/INGM issue, round 3 part 1.
- `unstop_resource`: round 2 part 2 says an undeclared resource is `requirements_invalid`; round 3 part 1, which is later, says `not_found`. Follow the spec (CTL).
- Wire resource type names: batch 6 item 7, round 2 part 2 (`file_resource_fault`, restarts).
- Generation outcome and `OperationCancelled`: crash recovery (`record_outcome`), round 2 part 1.
- `schedule_params_invalid` and scheduled runs: operations issue, round 2 part 1 (`runtime/schedules.rs`).
- Unparseable `last_fired_at`: failed reads (SCHED), round 3 part 2.
- Secret-flag migration both ways: round 2 part 1 (`migrate_newly_secret_params`).
- Unparseable request is `requirements_invalid`: OI error codes (WIRE#requests), round 2 part 2 (`parse_and_dispatch`).
- `registry_error` split: fault catalogue, round 2 part 1.
- Action and shell sharing a name: dispatch issue, round 2 part 2 (`invoke_action`).
- `Service.port` in the action context: dispatch issue (SVC), round 2 part 2.
- Validator context guards: batch 6 item 2, round 2 part 2.
- `issue-acme-dns` error codes: OI error codes, round 2 part 2.
- ARI renewal-information re-fetch: wire shapes (TLS), round 2 part 2.
- NAT64 teardown fault and idle teardown: host faults, round 2 part 1.
- Service `Ready` and `Scheduled`: lifecycle issue (LCY), round 2 part 1.
- `rt.exec` output to the container's log: lifecycle issue (RT), fidelity review.
- Forward teardown on uninstall: lifecycle issue (FWD), round 2 part 2.
- Bootstrap fingerprint normalisation: lifecycle issue (TRN), round 2 part 2.
- App status derivation: failed reads (STS), withholding faults issue, fidelity review (`operating` above `not_installed`).
- Env redaction and unknown secret params in `/apps/show`: secrets issue (done), host faults issue.
- `container_runtime_unavailable` clearing: fault catalogue, round 2 part 1.
- Start action after install: operations issue, round 3 part 2 (no install action).
- Persisted operation queue: operations issue, round 3 part 1 (encrypted params), round 2 part 2 (actor).
- Held volume record before the move: operations issue, VOLM issue.
- Reserved `seedling-op-` prefix: host faults issue, VOLM issue.
- `/connected-clients/list` failures: failed reads (WEB), OI error codes.
- `--wt-port` with `--wt-listen`: startup issue (WEB), round 2 part 1.
- `remote_host` validation: VOLM/SVCM/INGM issue, round 3 part 1.
- Site volume delete while mapped: VOLM/SVCM/INGM issue, round 3 part 1.
- Host constants: batch 5 item 7, round 3 part 2 (CONST).

## Already done in an earlier issue

- Uploaded key algorithms (round 3 part 1): done with the manual uploads issue, which detects all five algorithms.
- Removing `TlsPolicy::Tailscale` and `ErrorCode::CanopyUnavailable`: done with the TLS and Canopy decisions.
- Status precedence (TLS#certificate-status ordering): done with the TLS and Canopy decisions; round 2 part 2 still owes `none` for `default`, usable-certificate facts and `null` when unknown.
- `CollectMode` and discarding a failed unit once its result is observed (round 2 part 1, LINUX#supervised-units): done with the BSL issue, as termination depends on it.
- The database's side files being owner-only (host faults issue, INFRA#file-permissions): done with the secrets issue, by creating the database file owner-only before SQLite opens it; that issue still owes nothing else for it.
- `/apps/show` unknown secret params: done with the secrets issue; the host faults issue still owes env redaction.
- A control request over 64 KiB losing its actor (wire shapes issue): done with the OI error codes issue, which reads the actor from the whole request.
- Unparseable request is `requirements_invalid` (round 2 part 2, `parse_and_dispatch`): done with the OI error codes issue.
- `/connected-clients/list` read failures (failed reads issue, WEB): done with the OI error codes issue.
- `issue-acme-dns` precondition codes (round 2 part 2): done with the OI error codes issue; round 2 part 2 still owes `operation_in_progress` for a running issuance and `claim_if_idle`, and round 3 part 1 the Tailscale-hostname refusal.

- ARI renewal-information re-fetch on the RFC 9773 §4.3 schedule, keeping the stored window on failure (round 2 part 2): done with the wire shapes issue.
- The operation trigger list (wire shapes issue): already held; replay uses `replay`, and `boot` arrives with the start action in the operations issue.

## Decisions

- Secret sub-action params are kept in the action log encrypted under the secret key, apart from the readable params, so replay can still recover them; a call with secret params and no key to protect them fails rather than recording them in the clear.
- Files that must be owner-only are created through one helper, `seedling_protocol::keys::write_new_private_file`, which opens with mode 0600 and refuses an existing path, including a planted symlink.
- The container stop-timeout default stays with the supervisor, as the correction to the BSL issue says. `rt.stop`'s default deadline counts a container with no stop timeout at systemd's default of 90 seconds.
- Install completion order is a sequence number (`install_seq`) rather than a timestamp, so two installs can never tie; apps installed before it existed are ordered by name.
- A definition is checked once it has evaluated in full (`defs::check_evaluated`), for rules a later call could otherwise undo: redirect pairing, and containers with no image.
- A run whose exit status was never captured succeeded only if `unit_inactive` (the unit inactive or gone, failed units being kept loaded) is recorded after it ended.
- Shell and forward events carry the actor of the request that opened the session, including the session's end however it came about; `FaultCleared` carries the clearing request's actor when an operator clears it, and none when the runtime does.
- ARI is re-fetched by its own task, waking for the soonest due certificate and at least hourly; Retry-After is held between one minute and one day, and any failed fetch waits six hours.
