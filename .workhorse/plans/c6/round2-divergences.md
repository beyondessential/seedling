# Round 2 code divergences (for D6)

## Web
- crates/web/src/http.rs content_security_policy: `connect-src 'self' https://*:<port>` for an IPv6-literal Host.
- crates/web/src/http.rs connect / auth.rs handle_connect: 400 for a malformed body or both token and password, before any credential source.
- crates/web/src/main.rs: --wt-port conflicts with --wt-listen, and with --listen only when neither --wt-interface nor --wt-listen is given; wt_bind_port always args.wt_port.
- crates/web/src/event_broker.rs: reset back-off to 1 s once a subscription is established.
- crates/web/src/shell.rs exit_relay: relay the exit frame only when complete.
- crates/web/src/wt.rs: answer /forwards/start with requirements_invalid (direct connection needed).
- crates/web/src/proxy.rs peek_request and wt.rs: oversized, non-JSON or non-object first line answered requirements_invalid.
- crates/web/src/auth/tailscale.rs extract_actor: visible ASCII is non-empty bytes 0x21–0x7E.
- crates/web/src/web_sessions.rs: web entries' actor_* always strings.

## Runtime A and LINUX
- crates/core/src/system/observer.rs unit_fact: read the unit's Result for start-limit-hit, not SubState.
- crates/core/src/system/systemd.rs CollectMode: `inactive`; discard failed units once their result is observed.
- crates/core/src/system/reconcile.rs idle check: tear down proxy, resolver and NAT64 when no app is installed or installing (uninstalling apps don't count); daemon main sets NAT64 up only when needed.
- crates/core/src/system.rs and reconcile.rs: re-probe unavailable backends with back-off, swap in live backends, clear container_runtime_unavailable.
- reconcile.rs: caddy_failed for a rejected upgrade clears only when an upgrade succeeds; tear_down_idle clears caddy_failed and resolver_failed.
- crates/core/src/system/reconcile/phases.rs compute_nftables_rules: redirect site ingress attachment ports too.
- crates/core/src/system/reconcile/site_proxy.rs detect_conflicts: site-versus-site (first created wins; may need a creation time) and redirect-port claims.
- systemd.rs is_owned_slice/existing_owned_slices: record written slices, remove only recorded ones.
- crates/daemon/src/main.rs (~530): stop wiping /run/seedling/tmpfs-volumes at start.
- crates/core/src/runtime/apps.rs migrate_newly_secret_params and apps/secret_params.rs: migrate back to plain storage when secret is dropped; keep history from the secret period redacted.
- crates/protocol/src/events.rs SiteIngressAttachmentRemoved: add target_kind, target_app, target_service, redirect_url, redirect_code.
- crates/core/src/system/translate/proxy.rs: check two pods binding one prefix merge into one route.

## Runtime B
- Service Ready↔Scheduled: crates/core/src/system/reconcile/routes.rs record an empty-pool observation; runtime/barrier/oracle.rs derive_service_lifecycle allow Ready→Scheduled and back; reconcile/state.rs written_obs allow alternating facts.
- Cancelled outcome: oi/handler/actions/lifecycle.rs:443 emit OperationCancelled (new OiEvent variant and cancelled() in protocol/src/events.rs); runtime/generations.rs Outcome gains cancelled; record_outcome is test-only so outcomes are never recorded; update web consumers (actor_activity.rs, frontend types, EventsSidebar, Apps, AppDetail).
- Uninstall (reconcile.rs ~2010–2045): units_possibly_of rules out units matching another app's recorded identity; operator clear of uninstall_unidentifiable_units lets teardown complete and is not refiled; completion needs every recorded instance Unscheduled (not just units); delete_instances_for_app failure only logged.
- crash_loop (reconcile/faults.rs apply_crash_loop_faults ~1094): clear only after running the restart window without a recovery restart.
- runtime/priority.rs:138 load_all_app_priorities: an unparseable stored priority holds the app back and files app_priorities_unreadable instead of defaulting.
- runtime/schedules.rs: validate an empty invocation for scheduled runs, file schedule_params_invalid, record the fire.
- reconcile/faults.rs: registry_error split into desired_state_unreadable (app) and instance_identity_unreadable (instance).
- runtime/audit.rs spawn_audit_task: keep running, file audit_log_unavailable on open or write failure, reopen and retry per entry, count unwritten events, clear on next write.

## Interface B
- oi/handler.rs parse_and_dispatch: unparseable request → requirements_invalid (not not_found).
- Rename DeploymentRestarted operation_id → restart_id: oi/handler/apps.rs restart_deployment; protocol/src/events.rs and tests; web frontend EventsSidebar.tsx, AppDetail.tsx, useOi.test.tsx.
- oi/handler/apps.rs: restart_deployment needs not_installed unless Installed; unstop_resource refuses undeclared resources (requirements_invalid) and emits ResourceUnstopped only when a stop was removed (runtime/stopped.rs unstop_resource returns whether it deleted).
- oi/shells/session.rs (~717/733/745) and volume_session.rs: signalled job reports 128+signal, -1 only for session failure.
- protocol/src/error.rs ImageInUse; oi/handler/images.rs remove_image returns it.
- oi/forwards/session.rs (~203): key exhaustion → server_busy; (~503) remove the "UDP relay ended" status.
- Uninstall and removal end every forward and shell session of the app.
- Operation actors: oi/handler/actions/lifecycle.rs:890 queued dispatch loses the actor; param_change and post-install start action too; carry and persist through admission and queueing.
- oi/auth.rs import_bootstrap_file: normalise fingerprints per TRN, fail startup naming a malformed line.
- oi/handler/faults.rs: accept _system and seedling; not_found for other unregistered apps (list and clear); database failure → internal.
- reconcile/faults.rs file_resource_fault and reconcile/restarts.rs: resource_type uses WIRE type names.
- oi/handler/images.rs DiscoverParams.action_params and runtime/probe.rs: key by kind then name.

## TLS and interface A
- oi/handler/tls.rs decision_status (~1167): none not default; pending for any acquisition in progress; active only when usable; null when facts unknown (runtime/tls/state.rs:801).
- reconcile/state.rs and caddy/cert_observation.rs: record certificate-status and usable-served facts; check proxy-held certificates against their window; cert_acquisition_failed in-progress test uses the full definition.
- runtime/barrier/oracle.rs cert_valid_for: key on the usable-served fact.
- runtime/tls/parse.rs parse_chain (81): leaf by key match, for upload-manual and CSR upload-cert.
- oi/handler/tls.rs issue_acme_dns (379): preconditions → requirements_invalid; running issuance → operation_in_progress; runtime/tls/issuance.rs:531 issue_now uses claim_if_idle.
- runtime/tls.rs: remove TlsCertState::Failed (147) and RetryBlockSource::Auto (391).
- runtime/tls/store.rs: startup sweep closing pending non-Canopy attempts as failure; issuance.rs:364 trigger renewal/manual/on_demand.
- Renewal information: re-fetch per RFC 9773 §4.3; keep the stored window on a failed re-fetch.
- oi/handler/tls.rs hostname view: proxy and proxy_issuer (directory host); frontend lib/types.ts and TlsHostnamesTable.tsx (and test) incl. "default" → "none".
- oi/handler/tls.rs:1507 settings: validate contact_email; empty cert_profile counts as null.
- oi/handler/apps.rs /apps/show: instance health (ContainerHealth, system/podman.rs:908); protocol events InstanceHealthChanged emitted on change; console consumes both.
- Check: Canopy-only revocation stays with Canopy (runtime/canopy/certificates.rs); canopy_certificate_failed last-strategy condition fires while a usable certificate serves.

## Language
- oi/handler/actions.rs invoke_action: drop the not_found for a name that is also a shell.
- Action-context throws missing: on_action/on_start (defs/app/action.rs), on_shell (defs/app/shell.rs), on_install (defs/app/install.rs), description (defs/app.rs:463), Param kind/required/default_value/description/secret (defs/param.rs ~107-190).
- defs/service.rs:101-110: Service.port must not throw in the action context.
- Validator context (defs/app.rs:146 ensure_not_validating): also guard builder methods on captured handles, container builders, the app and param calls above, canopy_type, on_change, validate, on_schedule.
- defs/service.rs:536 route: refuse `{` and control characters other than tab.
- defs/ingress.rs:208: redirect codes 301/302/307/308 only.
- defs/collection/selector.rs:80 and runtime/barrier/runtime.rs:2600: anonymous resources match no names/name_patterns (Option<&str>).
- runtime/barrier/runtime.rs parse_exec_options (~1974-1990): container env rules for rt.exec env.
- runtime/action_params.rs validate_volume_params (~158): empty optional volume param is absent.
- defs/container.rs:317-318: interval and timeout at least 1.
- defs/container.rs mount (~557, ~572): key volume_mounts by canonical path.
- system/reconcile/proxy.rs: a DTLS ingress is passed through as raw TCP/UDP without terminating DTLS, contrary to ING's termination table.

## New interface specs (round 3 input)

### INGM (site ingresses)
- oi/handler/ingresses.rs create_site_ingress: duplicate name → requirements_invalid (not internal).
- delete_site_ingress: allow deleting a discovered ingress its provider no longer reports; event source is the real source (not hardcoded "manual"); emit SiteIngressAttachmentRemoved for each attachment removed with it.
- update_site_ingress: no event when nothing changed.
- attach_forward/attach_redirect: re-attach on an existing (port, protocol) replaces the target (site_ingress_attachments::update), keeps created_at, emits SiteIngressAttachmentUpdated; identical target is a no-op.
- Parameter `preserve_path` → `redirect_preserve_path` (also crates/ctl/src/ingresses.rs:229, web frontend routes/Ingresses.tsx:241).
- SiteIngressAttachmentRemoved carries the target (protocol/src/events.rs, detach).
- discovery_status: healthy null before the first poll.
- CLI: no command for /ingresses/site/discovery/refresh (web UI neither).

### SVCM (services), in oi/handler/services.rs unless stated
- Zero endpoints allowed (SITE changed to match); no change needed there.
- Duplicate service name, duplicate endpoints within a create → requirements_invalid (not internal).
- Endpoint add: unknown service → not_found; duplicate endpoint → requirements_invalid (both internal today).
- Delete and endpoint remove: unknown service or absent endpoint → not_found (requirements_invalid today).
- Delete while mapped: make the mapping check and the delete one indivisible step.
- Map: unregistered app → not_found; missing target → requirements_invalid; already-mapped slot → requirements_invalid (internal today).
- Remap: missing mapping or unregistered app → not_found; missing target → requirements_invalid.
- Unmap: missing mapping → not_found.
- remote_host: refuse loopback addresses (127.0.0.0/8, ::1) like localhost.
- runtime/site_services/resolver.rs: unreadable system DNS configuration at startup never retries and records nothing (endpoints silently unresolved); refresh_due_hosts read failure only logs. Retry, and surface it.
- CLI services site create cannot pass endpoints (needs an option now that the console flow is endpoint-less-first anyway).

### VOLM (volumes), in oi/handler/volumes.rs unless stated
- Held delete: unknown id → not_found, I/O failure → internal.
- Held restore (system/volume_store.rs restore_held_to_site): a failure after the move strands data; an errored request leaves state unchanged; unknown id → not_found; must not emit SiteVolumeCreated.
- Held list (volume_store.rs list_held): unparseable record → internal, not skipped.
- Managed site volume create: host_path given → requirements_invalid.
- Snapshot and promote without BTRFS, or a non-subvolume source → requirements_invalid (internal today).
- Snapshot source: absent → not_found; bind or snapshot source → requirements_invalid; failed history read → internal (not "no volume").
- Delete or promote of an absent site volume → not_found.
- Records and storage: managed create, snapshot and promote write storage before the record (failed record write strands storage); site delete holds data before deleting the record.
- External map: check app registered (not_found), target exists and app targets are exported, reject reserved seedling-op- names, duplicate mapping → requirements_invalid.
- Remap and unmap: missing mapping → not_found; remap checks its target; unmap triggers reconciliation.
- crates/core/src/reserved.rs: drop the backup-snap- site-volume prefix; delete system/leftover_snapshots.rs and its call in daemon main (sweep dropped from the spec).
- ACTU hold order (volume_store.rs hold_inner): commit the held record before the move.
- CLI: no commands for /volumes/app/list, /volumes/external/list, /volumes/external/declared.
