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
- [x] Batch 5: runtime and fault code
- [x] Batch 6: collections, validators, CLI, web, uninstall and image pulls
- [x] Requirements restored by the fidelity review
- [x] Blocking shell.attach, withholding-fault app status, rt.write to mapped external volumes
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
- `/faults/list` and `/faults/clear` take the app as a string: `_system` and `seedling` are accepted as they are, any other name must be a valid registered app (`not_found` otherwise, `requirements_invalid` if malformed). The CLI and web already send the name through unchanged, so neither needed a restriction removed.
- `install_interrupted` is cleared on either completion path of an install (the action path and the no-install-action path).
- During an image probe `rt.query` records nothing; only `rt.start` records images (`rt.warm_images` has its own path).
- `--script-max-call-depth` is refused at argument parsing with a value parser, so clap reports the error and the daemon never starts.
- External volumes' lifecycle state is held in `external_volume_states` (v74), written each iteration from the desired state (including for an uninstalling app, whose external volumes are `Unscheduled`) and read through `history::read_lifecycle`, which every reader of a lifecycle state now uses. The v74 migration also deletes the synthetic observations earlier versions recorded of external volumes. The pure `derive_lifecycle_state` reads an external volume as `Pending`.
- Demoted-instance retirement and the action keep-set share `gc::is_torn_down` (state `Unscheduled`, or the most recent observation moves to `Unscheduled`), so the two cannot disagree. A history that cannot be read still keeps the instance.
- Host constants: an undetermined time zone reads `UTC`, files `timezone_undetermined`, is retried each iteration and re-evaluates installed apps once found; `sysconst::detect_timezone` returns a `Result`. IPv4 and IPv6 egress, and their faults, were already done with the host faults issue. `AVAILABLE_THREADS`, `AVAILABLE_MEMORY`, `HAS_SNAPSHOTS`, `NODE_NAME` and `host_constant_undetermined` are not part of this issue's item and are left to the issue that names them.
- Anonymous Deployments, Jobs and Services hold an `anon_id` (an `InstanceId`) from creation, derived from the operation id and creation order (`next_anon_instance_id`), and `extract_instances` uses it as the instance id, so extraction no longer mints identities (`ReplayContext.anon_counter` is gone). Collections identify members as `Member::Named` or `Member::Anonymous`; `Started` narrowing matches anonymous instances by instance id. An `HttpService` coerces to the service it views, as that service's own member (an external service stays `external_service`), and `.one()` on it returns the service.
- A validator context guard sits in `Freezable::ensure_unfrozen` (so every resource builder throws) and in the `app` and `Param` methods that change the app. `app.param(name)` throws only for a name not yet declared, since looking up a declared one changes nothing. `service.port` is left alone, as it declares nothing.
- `templates update <name> [<source>]` shares `DefinitionArgs` with the other definition commands, so `--source` and `--script-file` are gone.
- `auth_required` is `none` when no `password_hash` is configured or it is not a PHC string with hash output; the console shows sign-in unavailable from the probe's 401 or from a login attempt's. A heartbeat answered `alive: false` closes the WebTransport session, which the existing reconnect path replaces.
- Console controls moved to `dangerous`: cancel CSR, clear TLS policy, clear image pins (global and per app), unmap external service and external volume, stop resource, unset param. Reset-to-default of a required param stays `write`, as it restores rather than removes. Clearing faults is `write` whatever the app's phase (it was `dangerous` outside `not_installed`).
- `/apps/uninstall` already matched ACN uninstall (the spec's id is ACI); only a test was added. `/images/pull` answers `fetch_failed`, and a pin that cannot be written is now `internal` instead of a logged warning.
- Wire resource type names come from `ResourceKind::type_name`, which replaced `journal::resource_kind_slug`, `stopped::kind_str`/`parse_kind` and the Debug-lowercase formats. v75 rewrites stored `externalvolume`/`externalservice` in `stopped_resources`, `faults` and `instance_restarts`. Internal storage (history, dynamic resources, container labels) keeps its Rust names.
- `shell.attach` blocks the closure's own thread: the session hands the closure a `ShellHost` that runs the job on the async runtime (`Handle::block_on`) and returns a `ShellExit`. The attachment's result is recorded in the closure's shared `ShellProgress`, so a rerun of the closure after a later call suspends it gets the same answer instead of attaching again; the per-pass flag in `ShellControl` is what makes a second call in one run throw. A job whose exit status was not observed (its wait failed) counts as an attachment that ended first, never as success.
- A session's exit code is the job's status once the closure has returned, with a signal exit as 128 plus the signal, so a job ended by the stop reads 143 rather than -1; -1 is kept for a failed session (a failing closure, `shell.error`, no attach, a job that could not be attached, stdin that could not be delivered, or a stop that landed before the job was known).
- The session's registry entry keeps its place after the attachment ends, but loses its container and pty (`ShellRegistry::detach`), so a late resize never reaches a closed pty.
- Dynamic resources created by a shell closure are not given to the reconciler: the closure runs with no database and no active progress, as before, so nothing starts them and `rt.start(...).terminated()` after an attach waits until the session is stopped. Nothing in this change tears resources down when the attachment ends. Wiring shell sessions into the reconciler's desired state needs per-session progress beside the app's single operation slot; it is not part of this issue.
- `faulted` needs a withholding fault (`faults::WITHHOLDING_KINDS`); the CNR mapping of `degraded` to `warning` and `faulted` to `failed` already read the statuses right, so it did not change. `health/faults` still counts every active fault.
- `rt.write` resolves an unbound ExternalVolume through the writer (`VolumeWriter::resolve_external`) against the host's mappings, so a read that fails is a throw naming the failure rather than "resolves to nothing"; `try_resolve_external_volumes` is the fallible form of the existing resolver. A read-only binding throws too, and so does a Volume declared read-only (RT#volume-writes asks for it and the code did not check).

- Fidelity review, proxy and TLS: `tls_chain_state.csr_rejected` (v77) records that Canopy's last answer rejected the CSR, so `canopy_certificate_failed` is held for it; it is set on a malformed answer and cleared by any other recorded answer. `cert_acquisition_failed` is one fault per ingress instance naming each failing hostname with its latest acquisition error, refreshed in place and cleared once every hostname is served. `health_check_failed` treats a `ContainerMissing` observation as unscheduled. Host-originated traffic redirection is IPv6 only; the IPv6 egress probe now reads the main routing table like the IPv4 one. A snapshot is refused only when the source is not a BTRFS subvolume. A manual upload's leaf is the certificate whose public key matches the key, wherever it stands, and the chain is stored leaf first. A relay stream is reset (send and receive sides) on timeout, an oversized or malformed response and a failed write.
- Fidelity review, runtime: the Canopy server identifier is cached with the key fingerprint of the offer that resolved it (`canopy_settings.server_id_key`, v76), used only while the selected offer's key matches, and discarded on a failed send. Operation-scoped volume names are `seedling-op-<operation id without hyphens>-<n>`, so they never carry the binding key. Scale is clamped on every successful re-evaluation (`scaling::clamp_to_definition`), not only `/apps/update`. `--gc-retain-restart-records` (default 50) reaches the GC through `InstanceGcConfig`, as do the image-GC interval and clamping. A stale Job is stopped and started in the same pass. `/images/remove` answers `image_in_use` (the spec's code, `ErrorCode::ImageInUse`) and clears pins before the not-found check. `/connect` is parsed by `ConnectRequest::parse` (400 for non-object, non-string members, or both credentials). `transition_phase` writes the database before the in-memory phase and returns a `Result`.
- Fidelity review, language: the `HTTP#balance` check is `ResolvedBalance::spins()` run by `defs::check_route_balances` from `check_evaluated`; the 0.25 fallback is gone. The `invoke` cycle error names the actions from the first occurrence of the re-entered action. `ParamUnset` withholds the value when the param is secret now or was stored as secret. Image-discovery probes call change handlers with as many arguments as they declare. DTLS ingresses are managed hostnames, and ingresses terminating TLS or DTLS without HTTP hand their hostnames to the proxy's certificates set.
- Fidelity review, console: the session token is held in memory only; `/connect` sends at most one credential. One shared `ParamFields` component serves actions, install and shells. Volume shell labels are display names joined with `, `. The other-session flag sits on the web-session count and the safety mode control in the highest other mode's colour. Healthcheck state reads the instance's `health` field, falling back to inference when the server omits it.
