# Review loop log

## Round 1

### Fidelity: runtime A

Restored: PRX rate-limit client identity (address attributed to the request); PRX persistent connections MUST; SITE snapshots per BTRFS-backed volume; SITE partial DNS answers MUST; INFRA output rules IPv6 only; DNS caching MUST; IMG GC at the GC interval (MUST) and 30-day retention unless operator-configured; UPD stale Jobs restarted immediately.

For the user: host prefix derivation (originals conflict); restart record retention configurable; PRIO kill order MUSTs; NAT64 egress "such as"; INFRA output-chain translation and loopback replies (code-only); code-derived value pins; fault thresholds pinned at 5; ACTU certificate warming.

### Fidelity: runtime TLS

Restored: TLS serving lookup cost and content-free miss as MUST; hostname state shows the responsible strategy; CHAIN HTTP-01 withheld only while Canopy is responsible. Fixed by me: renewal of other-origin certificates excludes Tailscale hostnames (conflicted with the settled Tailscale section).

For the user: contact-email gate; staged certificate counts as current; retry block refuses operator issuance; one ACME order at a time; pinned values (1 h after-failure, proxy hold-back, 500-char reasons, P-256 SHOULD); Canopy asks only on successful ticks plus pacing; proxy orders when undecidable; Canopy-only untyped fault; rejected CSR fault.

### Fidelity: web and root

Restored: plain HTTP endpoint; connect token/password client rules; Tailscale Serve trust as a note; safety mode in every heartbeat, MUST on change, pagehide heartbeat; UI surfaces enumerated as MUSTs; fault count, healthcheck and priority indicators; volume shell labels and read-write modes; shell sidebar; other-session flag placement and colour; route paths; certificates page contents and key-material ban; glossary: app registered, action closure covers change handlers, volume not necessarily persistent.

For the user: oversized/non-object relay requests vs WIRE requirements_invalid; error surfacing widened (keep); CSP without Host vs wt_url localhost; actors display reading; TLSM ACME account keys (move); HSTS note.

### Fidelity: interface B

Restored: TRN fingerprint probe MUST; SHL root over read-only mounts; RLY stream reset on limits; IMGM pins cleared regardless of removal outcome; CLI VCS metadata dirs "such as"; CLI log display MUST; LOG resource_kind links WIRE type names.

For the user: deregistering code dropped; log instance exact match; log stream member; key label default; EventsDropped.skipped optional; list results wrapped; image record shape; port-forward install gate; install gate widened to Uninstalling (SHL, ACI); volume shell ShellStarted name; shell stderr (fix WIRE description); stdout-write session end; UDP failure coalescing; 16 MiB request bodies; uninstall discards stops; registry remove not_found; log source name; nested .seedignore; fingerprint write failure; keep-alive; nullable previous_generation.

### Fidelity: runtime B

Restored: OPS operation volumes get fresh unique names; FLT faults not passed to scripts; health_check_failed clears on unschedule; cert_acquisition_failed identifies hostname and error; CNR re-resolves identity only on failures suggesting it is wrong; AUTO clamps on any re-evaluation; SUSP cadence MAY with example; GEN bundle sharing MUST; RCN no unbounded blocking; OBS fact at most once per run (check against decided "record again on change"); RCK infrequent and spread as MUST, 6 h as example.

For the user: cancel at barrier or rt call; NotInstalled with recorded instances moved to Uninstalling; image_pull_failed threshold and subject; cert_acquisition_failed 180 s; faulted resources excluded from reconciliation (drop); code-derived fault kinds; rt.exec output MUST (RT); crash-loop defaults; clamping deletes scale; sub-action history; bundle on every history entry; re-check back-off values; first schedule fire; audit file mode; engine limit details; new lifecycle cycle; CNR additions.

### Fidelity: interface A

Restored: STS no expensive computation; APPS status order (operating above not_installed); PRM plan lists handlers that would be scheduled; TLSM manual upload accepts any PEM order, stored leaf first; TLSM CSR SAN set without order or subject constraint.

For the user: TLSM stale manual-policy claims; auto_issue_kicked; APPS removal only when not installed; uninstall specified twice (APPS and ACI); Faulted vs Degraded meaning; restarts shape; history redaction shape; route served and `/` route; install result and no-install-action; empty string as no value; unknown parameter keys (originals conflict); PRM internal path; DEFN limits and digests; STS SemVer.

### Round 1 triage

Fixed by me: WIRE shell stderr description; APPS uninstall reduced to a link to ACN; TLSM never returns ACME account keys; WEB relay refusals answered requirements_invalid; CSP localhost without Host; one ACME order SHOULD; Canopy-only untyped and rejected-CSR faults; PRIO kill order MUST where the platform allows; NAT64 egress definition back to the original, Linux specifics in LINUX#egress; restart record retention configurable; volume shell event name; zero balance interval fails evaluation.
Accepted code-derived details: recorded in gaps.md "Decided in the review loop".
Code checks filed on D6.
Asked the user: Faulted vs Degraded; shell attach timing; rt.write on mapped external volumes.
Settled by the user: withholding faults decide faulted; blocking shell.attach with ShellExit; rt.write to mapped external volumes unless read-only.

### Structure

Moved: Barrier defined once in RT (SUSP keeps the mechanism); cycle errors to ACN; sub-action validation and reserved keys to ACN; start-action schedule rule to ACN; secret redaction to SEC; rate-limit budgets to PRX; container defaults to CTR; 0640 writes to ACTU; ingress serving to PRX; certificate warming to RT; template and Canopy offer events to EVT; key fingerprint to the root term; TLS terms from TLSM to TLS; client identity key to CLI; fault restatements across 18 specs now link FLT rows, with descriptions moved into FLT.
Fixed by me after: FLT clearing wording for cert_expiring_soon and canopy_certificate_failed taken from TLS and TCAN; AUD#generations removed (EVT holds the members), citations to EVT; field ownership moved to INFRA#database, upsert citations retargeted; WIRE internal row says a failed read is never not_found or empty; withholding faults link AUTO.
Left: WebSession events and web error codes stay in WEB; hostname syntax differs between TLSM and ING (not unified); /volumes/*, /services/exported/list and OI certificate statuses need design, not moves.

## Round 2

### Runtime B (found: 5 critical, 12 suggestion, 7 nit)

Fixed in place: CNR statuses use the APPS names (no `Deregistering`); actors on bare requirements in CNR, OPS, FLT, LCY, TRD, AUD; "what" stand-ins; fault kinds named; link targets corrected.

Critical: canopy_offer_absent shape vs restart; service leaving Ready has no target state; OBS says certificates decide ingress Ready (contradicts LCY); cancelled outcome missing from history and events; container_runtime_unavailable clears only on restart.
Suggestion: teardown process unspecified; stuck uninstall exit; crash_loop clears between crashes; unreadable priorities behaviour home and scope; failed scheduled validation and refire; CNR identifier discard condition; action-call table incomplete; replay literal arguments; "lifecycle transition of that app"; registry_error two shapes; audit write failures; re-check "infrequently".
Nit: empty RUN; start-action hold stated thrice; side effects beyond rt.*; uninstalling apps and shared infrastructure; CNR resource field; OBS sentence; old reconstruction twice.

### Web and root (found: 1 critical, 13 suggestion, 7 nit)

Fixed in place: WEB actors, borrowed terms, relay wording to WIRE terms, shell relay stream names; UI Deployment health checks and links; glossary entries aligned with defining specs and pointed at sections; external documents rows completed for LINUX, RFC 1035/1123 split.
Critical: UI shows health check state no OI method or event reports.
Suggestion: CSP host-source cannot carry a bracketed IPv6 literal; closing a shell needs dangerous vs volume shells in every mode; key-material ban belongs to the server; web key file name and rules; subscription back-off reset; shell relay stream end without exit frame; /connect bad body answer; option conflicts per listener; daemon reconnect failure; /forwards/start through the relay; response framing; restart record "initiator"; web session count.
Nit: WebSession events in EVT; TOML version pin; untestable "within the app"; duplicated PRIO note; Tailscale header charset; actor members nullable.

### Interface B (found: 1 critical, 18 suggestion, 5 nit)

Fixed in place: WIRE actors and FWD links; TRN note; KEY fingerprint link; CLI wording and LINUX link for the state path; LOG framing link; EVT links and terms; CTL effective scale link; IMGM registry link; RLY shapes and terms; FWD borrowed EDNS(0); SHL install phase links.
Critical: 38 OI methods have no interface spec (held, site, app, exported and external volumes; site, app, exported and external services; site ingresses and discovery).
Suggestion: EVT "as X specifies" where X never mentions the event; restart operation_id vs lifecycle operation; EVT same-name members differ; shell signal exit_code and CLI mapping; in-use image removal code; image pull allowlist; RLY non-relaying connections code; volume shell preparation code; FLTS _system listing and unregistered apps; fault subject mapping in records; forward end and lifetime; actor on install and param_change triggers; fingerprint input rules twice; CLI server trust (known_hosts) unspecified; discovery handler names and param key collisions; CTL gating for scale/restart/priority/unstop; datagram for a TCP forward key.
Nit: actor null in forwards and shells; duplicated stream-end and 64 KiB rules; LOG tail range and platform terms; volume reference home; ServerBusy reasons, forward key exhaustion, RFC 6891 row, unparseable request code.

### Language (found: 3 critical, 17 suggestion, 5 nit)

Fixed in place: BSL overview sentence split; link targets (action closure term, builder term, runtime handle, OPS); "what" stand-ins; run-on sentences; volume external definition aligned with the glossary; absences removed.
Critical: ACI refuses a name that names a shell (contradicts separate namespaces); app.external_volume in the action context unspecified; command without args drops the image's args (unstated).
Suggestion: change handler vs NotInstalled; validator context vs named resources; action-context behaviour of declarations, Param builders, Service.http/ingress; tmpfs note in RT; compress defaults vs resolution; routes per service or per port; route prefix characters; redirect-or-bound wording; default content types list; ingress redirect codes vs route redirect codes; redirect before a certificate and host port; ingress names; RT resources vs instances in Started, Termination and barriers; allowlist enforcement home; rt.exec env rules; "TLS-terminating ingress" term; optional empty params and kind validation; warm_certs repeat; zero interval/timeout meaning.
Nit: zero-argument change handlers vs runtime handle; two homes for Action selection and redirect binding; "action context" vs "while an action closure is running"; UDP without tls(); mountpoint canonical form.
