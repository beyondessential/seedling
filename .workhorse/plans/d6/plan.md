# Bring the code in line with the rewritten specs

Each of the card's issues is one theme of divergence between the code and the specs #C6 rewrote.
Each issue is worked on its own and committed on its own, with a test that fails without the fix.

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

## Decisions

- Secret sub-action params are kept in the action log encrypted under the secret key, apart from the readable params, so replay can still recover them; a call with secret params and no key to protect them fails rather than recording them in the clear.
- Files that must be owner-only are created through one helper, `seedling_protocol::keys::write_new_private_file`, which opens with mode 0600 and refuses an existing path, including a planted symlink.
- The container stop-timeout default stays with the supervisor, as the correction to the BSL issue says.
