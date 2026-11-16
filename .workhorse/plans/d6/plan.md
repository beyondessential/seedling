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
- [x] Failed reads become definite answers
- [x] Database and internal failures reported with the wrong error code
- [x] Malformed script input coerced or ignored
- [x] Crash recovery and replay
- [x] Faults out of line with the fault catalogue
- [x] Action, shell and change-handler dispatch
- [x] Host startup ordering, addressing and the web listener
- [x] Resource and lifecycle behaviour the code does not enforce
- [x] Wire, event and ACME protocol shapes
- [ ] Operations: start action, persisted queue, unreadable state, intent-before-effect
- [x] OI error codes and request validation
- [x] Host: new faults, key file, permissions, reserved prefix and env redaction
- [ ] Batch 5: runtime and fault code
- [ ] Batch 6: collections, validators, CLI, web, uninstall and image pulls
- [ ] Requirements restored by the fidelity review
- [ ] Blocking shell.attach, withholding-fault app status, rt.write to mapped external volumes
- [ ] Review round 2, part 1
- [ ] Review round 2, part 2
- [x] VOLM, SVCM and INGM handlers
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
- Database errors answering `internal` in scale, stop, unstop, restart settings and faults (round 3 part 1, round 2 part 2), image discovery's `script_error` (round 3 part 1), and volume shells' `internal` (round 3 part 1): done with the error codes issue.
- `on_action("start", ...)` with `params` throwing, `on_schedule` outside the static context, and the reserved `install` name (dispatch issue, round 2 part 2) were done with the BSL issue; the malformed-input issue added the unknown-key checks.

## Decisions

- Secret sub-action params are kept in the action log encrypted under the secret key, apart from the readable params, so replay can still recover them; a call with secret params and no key to protect them fails rather than recording them in the clear.
- Files that must be owner-only are created through one helper, `seedling_protocol::keys::write_new_private_file`, which opens with mode 0600 and refuses an existing path, including a planted symlink.
- The container stop-timeout default stays with the supervisor, as the correction to the BSL issue says. `rt.stop`'s default deadline counts a container with no stop timeout at systemd's default of 90 seconds.
- Install completion order is a sequence number (`install_seq`) rather than a timestamp, so two installs can never tie; apps installed before it existed are ordered by name.
- A definition is checked once it has evaluated in full (`defs::check_evaluated`), for rules a later call could otherwise undo: redirect pairing, and containers with no image.
- A run whose exit status was never captured succeeded only if `unit_inactive` (the unit inactive or gone, failed units being kept loaded) is recorded after it ended.
- Shell and forward events carry the actor of the request that opened the session, including the session's end however it came about; `FaultCleared` carries the clearing request's actor when an operator clears it, and none when the runtime does.
- ARI is re-fetched by its own task, waking for the soonest due certificate and at least hourly; Retry-After is held between one minute and one day, and any failed fetch waits six hours.
- A stop that fails its deadline keeps its call slot, so a script that catches the failure reaches the next call at the next position; the deadline's start is read from the call's own log record, never by matching resources.
- A reconstruction that cannot read a recorded value fails (`Error::UnreadableParam`), and `load_all_params_for_app` returns an error when either parameter table cannot be read; a change handler is never run with an `old` that is not the previous generation, and `old.<type>(name)` resolves against `old`'s own definition.
- An instance whose observation history cannot be read has an undetermined lifecycle state (`Undetermined`): no event, no barrier decided or timed out, no retirement. The oracle's `lifecycle_state` returns a `Result` so a caller must decide.
- A failed read leaves faults as they stand (disallowed registry), reads as `null` or `internal` for operators (status, `/apps/show`), and keeps images, instances and schedules in place; a reconciliation iteration that dropped every app does not idle.
- A schedule whose last fire time cannot be parsed is not fired and holds `schedule_unreadable`; unreadable schedule records hold `schedules_unreadable` on `_system`.
- A certificate in the proxy's cache is observed valid only inside its validity window; one that cannot be read leaves the hostname undetermined.
- Backends are held behind `system::backends::Backends`: the container runtime and process supervisor start as erroring stubs when unreachable, are probed again with doubling delay up to 300 seconds, and replace the stubs in place; `container_runtime_unavailable` clears once both are reachable.
- Observe-failure streaks are reset only for instances the calling pass observed, since they are shared by every app and resource kind. Faults keyed by an instance, image reference, attachment or hostname are filed with `file_once` and cleared by key.
- `health_check_replace_failed` clears when the generation history shows a generation made after the fault, so a change made while the daemon was down still clears it.
- A NAT64 translator that cannot be set up at startup no longer exits the daemon; the reconciler retries it and withholds workloads. This path has no test, since set-up runs the host's translator.
- The `registry_error` split into `desired_state_unreadable` and `instance_identity_unreadable` is left to round 2 part 1; only the over-broad clear was fixed here.
- A shell's params are validated by the same code as an action's (`validate_shell_params`), and a `volume` param is bound through `bind_volume_params`, which both dispatches use; a binding that cannot be made fails the dispatch before the closure runs. `shell.error`'s message is the reason the client is given.
- A change handler is called with as many of `rt` and `old` as it declares, read from its definition in the script.
- Pod network prefixes are allocated and recorded in `pod_networks` (v72), unique per resource kind, and released when the network is removed or found gone at startup; a kind holds at most 256 networks at once. A pod running with no record is left out of routing.
- The resolver is brought up before the iteration's pods phase and stops the iteration while it is down, as NAT64 does.
- The proxy image check already existed (`caddy-config.yml`, `check-caddy-image.sh`, the `--ignored` image test that first shows an undeclared field rejected); it was built and run here with podman and passed, so no code changed for it.
- The web interface does not start with no daemon fingerprint in any build; `--daemon-trust-any` remains an explicit debug-build option.
- A Job's deadline is enforced by the reconciler from the latest `container_running` observation. A replace update holds every start of the Deployment until no stale instance remains. A service whose pool empties records `backend_lost`, which takes it from Ready back to Scheduled.
- `rt.exec` asks the executor whether the target is running before the call is recorded, and what the command wrote is recorded under the container's own journal fields (`ExecOutput`).
- Forwards end when their app leaves `Installed` or a definition drops their port, whether by update or parameter change, and a relay ends with its forward. A stream naming no live forward is reset.
- Revoked key fingerprints are kept in `revoked_keys` (v73) so the bootstrap file does not authorise one again; bootstrap fingerprints are normalised and a bad line stops the daemon.
- Every Cargo, npm, GitHub Actions and container image dependency is at its latest release except `rhai`, which is held at 1.25.1: from 1.26 a closure that captures `app` cannot be passed to a method called on `app`, which every BSL script does, so each fails with ErrorDataRace.
- The proxy image moves to Caddy 2.11.9 and caddy-l4 0.1.3 with tag `2.11.9-1`, and the runtime pulls that tag, so it must be published before this reaches a host.
- Host facts that cannot be determined at start (NAT64 detection, IPv4 and IPv6 egress) are held in `HostProbes`, looked for again each iteration, and fault until found (`nat64_detection_failed`, `ipv4_egress_undetermined`, `ipv6_egress_undetermined`). Undetermined egress constants read `false`; NAT64 stays active while detection is undetermined, and a later answer does not deactivate it. A fact found later re-evaluates every installed app. A failed idle NAT64 teardown files `nat64_teardown_failed`, which has no test since teardown runs the host's translator.
- The secret key is only created when no database holds protected values; a missing key beside protected values is a startup error (`Cipher::load_or_create`). Audit entries that cannot be written hold `audit_log_unavailable` with a count of those lost, and the file is reopened for each entry. Five failed definition rechecks file `recheck_failed`. External volumes named `seedling-op-*` are refused at creation.
- The site-volume sweep and the `backup-snap-` reservation are gone. A new site volume's record is written before its storage and removed again if the storage fails; deleting a site volume deletes the record (refused while a mapping targets it, in one step) and puts it back if the hold fails. An app target of an external volume mapping that is unregistered, undeclared or unexported is `requirements_invalid`, as VOLM says, though the issue text says `not_found` for the undeclared case.
- A held volume's record that cannot be read fails the list and every lookup of that id; `restore_held_to_site` moves the data back if the record cannot be removed.
- A site service that has endpoints, the mapping tables and site ingress attachments are written through single-step helpers (`create_if_absent`, `delete_unmapped`, `create_unique`, `update_existing`, `set_target`) so each check and write happen together.
- The resolver treats a lookup that returned no records from either family as unanswered, whether or not a family failed, names every failed family in `failed_families` (replacing `degraded_family`), wakes for the soonest due entry, and reads the system DNS configuration again on each refresh until it can; its unanswered lookups then reach `site_service_endpoint_unresolvable`. A failed read of the site services in `refresh_due_hosts` is logged at error level and tried again at the next wake: the fault catalogue has no kind for it.
- Attachment events carry the target (a redirect's `redirect_preserve_path` too), including `SiteIngressAttachmentRemoved`. The Tailscale provider reports its changes as events with no actor, and discovery status lists a provider only if the runtime runs it, with `healthy` null before its first poll.
- `redirect_preserve_path` is the wire name; the CLI and web console send it.
- CLI additions: `volumes app`, `volumes external map|remap|unmap|list|declared`, `services site create --endpoint`, `ingresses site discovery refresh`.
