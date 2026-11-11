# Round 3 closing fixes

Every finding below is decided; apply it.
Where an item says "check the code", read it to learn the fact, then specify it; the code is not authority where it breaks an AGENTS.md rule or a settled decision in gaps.md.
Where the spec now requires something the code does not do, add a line to your "Spec leads code" list.
Paths are under `.workhorse/specs/`; line numbers are approximate.

## S: site resources and events

Files: `interface/services.md`, `interface/volumes.md`, `interface/site-ingresses.md`, `interface/events.md`, `runtime/site-resources.md`, `web/console.md`.

1. SVCM resolver-status: replace `degraded_family` with `failed_families` (array of `"AAAA"`/`"A"`, the families whose most recent lookup failed, empty when none). Redefine `last_attempt_failed` as SITE's threshold condition: the most recent lookup returned no records and at least one family's lookup failed. Keep SITE#partial-dns-answers consistent.
2. EVT: add `redirect_preserve_path` (boolean, present only for a redirect) to the three site ingress attachment events. Keep the name `redirect_preserve_path` (matches `redirect_url`, `redirect_code`); the code's `preserve_path` request member is a divergence.
3. A mapping target that does not exist is `requirements_invalid` in both VOLM and SVCM (a reference inside the request, not the addressed object).
4. EVT's actor rule: list events a discovery provider's poll causes, including a poll `/ingresses/site/discovery/refresh` started, among the events the runtime causes on its own (no actor).
5. Mappings whose target stops existing: the runtime MUST refuse to delete a site volume while an external volume mapping targets it, as for site services (SITE and VOLM). A mapping whose target no longer exists (app deregistered, volume or service gone from the definition, volume no longer exported) is kept, and the external volume or service behaves as unmapped while it lasts. Check FLT's `external_volume_not_mapped` row and the external service equivalent; if they cover only a missing mapping, report the FLT wording needed (FLT belongs to fixer R1).
6. State once, in SITE#external-mappings and SITE#site-service-lifecycle, that the runtime reconciles promptly after a mapping change or a site service endpoint change; drop VOLM's own copy and link.
7. Forward attachment port: check the code for which service port a forward attachment's traffic reaches, and state it in SITE#attachments.
8. Console: attach and detach forward and redirect targets on manual and discovered site ingresses; offer deletion of a stale discovered site ingress.
9. SVCM table: `endpoints` required cell becomes "no, default empty".
10. EVT common-members table: scope it to the events EVT lists, so WEB's `session_id` on web session events is not a shell session.
11. Ordering: ascending byte order everywhere; give `/volumes/held/list` and resolver-status entries an order (byte order of name and host).
12. VOLM snapshot and promote: a storage failure answers `internal`, as for create, delete and restore.
13. INGM `healthy`: name the antecedent of "reached it", define or replace "running identity", and make the client rule SHOULD NOT. Refresh with no discovery provider running: check the code's answer and specify it with a WIRE code.
14. SVCM loopback exclusions: also exclude names ending `.localhost` (RFC 6761), IPv4-mapped loopback (`::ffff:127.0.0.0/104`) and the unspecified addresses `::` and `0.0.0.0`. Glossary RFC 6761 if not already.
15. SITE endpoint addressing: an endpoint whose name returns no records in either family, with no failure, counts towards the unresolvable threshold as a failure would. An IPv6 literal on a host without IPv6 egress is unroutable, like an AAAA-only name. While IPv6 egress is undetermined, link the NAT64 rule for the undetermined case.
16. SITE partial answers: check the code and say whether the failed family's kept answer routes traffic; give the retry schedule ("promptly" becomes a named point, such as the next reconciliation iteration, from the code).
17. SITE `:107`: "an external service mapped to a site service that exposes none of its ports has no target".
18. Site redirect `Location`: move the rule from INGM into SITE#attachments, stating the request-target is appended raw, as PRX says for redirect routes; INGM links it.
19. EVT `resource_name`: "string or null", `null` for an anonymous resource.
20. EVT actor for replayed operations: events emitted by an operation resumed by replay carry no actor.
21. EVT `ScaleChanged.previous_scale`: nullable, `null` where the previous effective scale cannot be read. If the member table is in `interface/app-control.md`, leave it and report it (fixer W owns that file).

## T: TLS, apps, actions, params

Files: `runtime/tls/*.md`, `interface/tls.md`, `interface/params.md`, `interface/apps.md`, `interface/actions.md`, `interface/templates.md`, `runtime/app-teardown.md`, `runtime/operations.md`.

1. CHAIN: the default chain covers every managed hostname no policy matches except one that takes a Tailscale certificate.
2. CHAIN/TLS: the proxy MUST NOT order a certificate for a hostname on no chain (ACME DNS-01 policy or Tailscale hostnames); it serves the runtime's certificate once one exists.
3. TLSM `issue-acme-dns`: refuse a hostname that takes a Tailscale certificate with `requirements_invalid`.
4. TLSM: `retry`, `retry-blocks/set`, `retry-blocks/clear`, `issue-acme-dns` refuse a `hostname` that is not a hostname with `requirements_invalid`; blocks and retries match managed hostnames ignoring ASCII case.
5. PRM: a `name` that breaks the naming rules is refused with `requirements_invalid`.
6. TLSM `key_type`: enumerate the values from the code.
7. TCAN: member tables (`| member | type | meaning |`) for the success answer (`chain` as a PEM string, `state`, `last_error`) and the held-certificate list, from the code.
8. ACI cancel: `/apps/action/cancel` applies only to a running operation and cannot withdraw a queued one. TRD's note: the operator waits for the queued operation to start, then cancels it.
9. PRM `/apps/plan`: a parse or evaluation failure of whichever definition is evaluated is reported in `errors`, not refused.
10. Nits: APPS history redaction "either of whose values was protected when the change was recorded"; APPS `on_exit` `"default"` is reported when the definition sets no exit strategy; APPS `description` rows "string or null"; TLSM hostnames list for an unregistered `app` is `not_found`; TLSM delete refuses a `csr_pending` row with `requirements_invalid` (cancel it with `csr/cancel`) unless the code already handles it atomically, in which case specify the code; delete ACI's repeated line 34; ACI and TPL: "where several refusals apply, the server MAY answer with any of them"; APPS:29 and ACI:99 link `events.md#app-events`; APPS:155 "except where a member's description gives it a value for that case"; TLS:411 "or Canopy cannot be asked about it"; CHAIN:96 "The runtime MUST treat as a refusal only an answer the strategy's issuing authority gave."; TLS:32 and TCAN:37 third-party behaviour into notes; PRM:27 drop `/apps/params/unset`; CHAIN:69 keep only "MUST keep serving that certificate until a replacement outranks it".
11. OPS: the persisted queue's parameters carry the same protection as a persisted operation's.
12. OPS replay: an operation-scoped volume binding is re-established under its persisted name, and the binding's name is among the persisted parameters.
13. TRD: while the recorded identities cannot be read, the runtime holds `instance_identity_unreadable` (link FLT). TRD:56 "whatever requested it".

## L: language

Files: `language/*.md`.

1. RT:45: delete "and to at least one where that count is 0".
2. ACN: when an app moves to `Installed` without an install operation (no install action), the runtime runs the start action with trigger `install`.
3. DEPL: the scale bounds hold outside update strategies; during a rolling or replace update the instance count follows UPD (link).
4. CTR:57 "whose entrypoint is set to a non-empty array".
5. Undetermined host constants: extend the fallback-plus-fault pattern (CONST#determining-host-constants) to `AVAILABLE_THREADS` (fallback 1) and `HAS_SNAPSHOTS` (fallback false); an undetermined `AVAILABLE_MEMORY` or `NODE_NAME` fails the evaluation with the same fault. State that constants are determined at each evaluation. Check the fault's name in FLT; if FLT needs a wording change, report it (fixer R1 owns FLT).
6. HTTP: a second `compress`, `balance` or `rate_limit` call at the same level replaces the first entirely.
7. DEPL health check command runs under `/bin/sh -c` in the container.
8. ACN: a `params` option on `app.on_action("start", …)` throws.
9. RT `rt.write`: to a named Volume whose storage does not exist yet, the runtime creates the storage first (check the code; if it does otherwise, specify the code's behaviour where it is coherent); to a `readonly()` Volume, it throws.
10. PARAM: a change handler declaring more than two parameters fails when the runtime calls it.
11. BUNDLE: a bundle breaking the metadata rules is refused at registration and update; check the code for the code it answers with and specify it (WIRE code), with the server as actor.
12. Nits: ACN:167 "A volume binding lasts for one operation, while a parameter's value and the setup an install performs outlive it."; CONST:112 "No resource matches `ResourceType.Parameter` or `ResourceType.Action` in a selection's `types`.", drop line 115; JOB:36 "once `seconds` seconds have passed since it reached Running"; HTTP:70 "any route"; VOL:17 "runtime"; RT:155 "whereas it reapplies a tmpfs volume's declared file writes"; APP:53 fold into 52; ACN:101 "whether the param needs a value"; ACN:158 "Declaring a `volume` param in the install action's param schema MUST throw"; CTR:60 "Setting `command` replaces the image's entrypoint and its default arguments together, so a script that sets it supplies any arguments with `arg`."
13. Leave SVC:54 (pods binding an external service's ports) unchanged; it waits on the user.

## W: wire, transport, clients

Files: `interface/wire.md`, `interface/transport.md`, `interface/keys.md`, `interface/app-control.md`, `interface/images.md`, `interface/port-forwards.md`, `interface/shells.md`, `interface/client.md`, `interface/faults.md`, `interface/canopy-relay.md`, `overview.md` (external documents only).

1. TRN: the server records each revoked key's fingerprint and never re-imports a revoked key from `authorized_keys`; other entries are imported at start as now.
2. WIRE: the client MUST end every opening line with a line feed; state the client side of a subscription request (it keeps its side open, or finishes it; check the code and pick the one the server handles).
3. WIRE/FWD: at the stream limit the server resets a port forward data stream with application error code 0 instead of writing `server_busy`.
4. RLY: header values are bytes 0x20 to 0x7E and horizontal tab.
5. CTL unstop: a resource missing from the definition is `not_found`; take stop's three-kind list; a definition update drops stops on resources the new definition no longer declares.
6. CTL `ScaleChanged.previous_scale` nullable, `null` where unreadable (if the table is here).
7. IMGM: a `proposed_script` replacing a script spanning several files is refused; check the code for the code it answers with.
8. FWD and SHL: "when the app leaves `Installed`".
9. CLI: an unreadable `known_hosts` makes the client refuse to connect, without overwriting it.
10. FLTS: add a record row for faults about an app service as a whole (`resource_type` `service`, its name, `instance` `null`).
11. TRN: the server's key is Ed25519.
12. CLI: gitignore patterns as a borrowed term, citing git's gitignore documentation, and a root External documents row.
13. SHL: two mounts whose display names sanitise to the same value are refused with `requirements_invalid`.
14. FLTS: `/restarts/settings/set` with neither member, or with `params` `{}`, is refused with `requirements_invalid`.

## R1: runtime core

Files: `runtime/faults.md`, `runtime/reconciliation.md`, `runtime/autonomous.md`, `runtime/audit-log.md`, `runtime/lifecycle.md`, `runtime/scheduled-actions.md`, `runtime/canopy-reporting.md`, `runtime/observation.md`, `runtime/generation.md`, `runtime/garbage-collection.md`.

1. RCN: each decided autonomous operation is recorded with its provenance before step 7 performs it, and step 8 records its outcome; an operation whose entry cannot be recorded is not performed.
2. RCN: the steady-state desired state is the whole resource graph for `Installing` and `Installed` apps; an `Uninstalling` app's recorded instances are desired `Unscheduled`; a `NotInstalled` app has no desired state.
3. RCN: "autonomous operation" in steps 5 to 8 and lines 57 to 62; step 4 schedules a resumption without waiting for it.
4. FLT `script_error`: drop "on an update"; it is filed when a stored definition fails to parse or evaluate outside an update request.
5. FLT `definition_source_moved`: empty subject, both digests in the description.
6. FLT `cert_acquisition_failed`: keyed by the ingress instance, filed while any of its hostnames fails, cleared when every hostname of the ingress is served.
7. AUD: check the code. If the writer drops entries when behind, say the runtime MAY drop them rather than block and MUST hold `audit_lag` while it does; if it queues, say entries are written late, never dropped, and `audit_lag` holds while behind.
8. AUTO `crash_loop`: no automatic restart of the instance, whichever component performs it.
9. LCY: when an instance's history cannot be read, its state is undetermined: no barrier is decided on it, no state-change event is emitted, and reconciliation does not act on it in that iteration.
10. SCH: a schedule whose last fire time cannot be read or parsed does not fire; the runtime holds a fault for it (use an existing kind if one fits, else add a row with filed, cleared and shape).
11. CNR: a machine detail member the runtime cannot determine is `null`, never 0.
12. CNR: the cached server identifier is discarded when the selected offer's client identity changes.
13. LCY: a service is `Ready` while every port's routing pool is non-empty.
14. GC: retiring an instance clears its active faults as FLT specifies (with their events), then deletes the records. GC:75 "any `rt` call addressing a Deployment".
15. Nits: AUTO:67 name the consequence of "broken" (check the code); FLT:57 name the unit of "one evaluation"; FLT operation_failed and operation_cancelled clear when a later operation completes successfully; LCY volume `Scheduled` vs `Ready` (say when a volume is `Scheduled` but not `Ready`, or merge them, from the code); OBS:45 and the backend wording follow ACTU's data-directory backend (R2 aligns ACTU); SCH:19 add a step to RCN's list for schedule checks, or drop the loop reference; GEN:35 link OPS#completion instead of restating it; AUTO:101 cut, AUTO:131 link the scale section.
16. Apply any FLT wording fixers S and L report back to you through me; none yet.

## R2: runtime infrastructure

Files: `runtime/proxy.md`, `runtime/infrastructure.md`, `runtime/resolver.md`, `runtime/update-strategies.md`, `runtime/actuation.md`, `runtime/nat64.md`, `runtime/priority.md`, `runtime/images.md`, `runtime/identity.md`, `platform/linux.md`, `language/ingress.md` (item 1 only).

1. ING and PRX: "the same port number" holds for TLS, DTLS and non-terminating ingresses; an HTTPS-terminating ingress delivers through the Service's routes (PRX#route-emission). PRX defines the fallback route's port: the Service port of the Service's HTTP binding, the lowest-numbered where several (check the code and the HTTP service spec for whether several can exist).
2. INFRA ingress redirection table: "UDP for a DTLS ingress; TCP, and UDP as well, for HTTPS; TCP otherwise".
3. INFRA: a rule for service addresses: each is unique and disjoint from pod prefixes and infrastructure addresses (how it is derived stays out).
4. DNS: the resolver answers every query from its upstreams, its cache, or DNS64 synthesis.
5. PRX and DNS: drop the incomplete fault-clearing restatements and link FLT's host faults rows.
6. PRX:254: "where the runtime reads them".
7. INFRA:59: "the host's machine identifier"; the path goes in LINUX's host facilities table.
8. IMG:44: read the context; if it is about replay say "on replay", else cut.
9. ACTU: define the tmpfs location in Terminology; name `--data-dir` in the data directory definition. ACTU:134 backend wording: the backend of the data directory.
10. PRIO:51: a `Critical` workload is the kernel's least preferred victim within the order above.
11. IMG:51 "equals".
12. NAT64:63: scope with "While NAT64 is active"; a start with no app installed may leave the translator torn down.
13. ACTU:120 tmpfs survival stays; it is a code divergence.
