# CSR cert upload binds by the requested hostname instead of the cert's SANs

## The actual defect

The card was filed as "the upload never checks SAN coverage", but the sharper framing is
that `csr_upload_cert` treats the *requested* hostname as the thing the certificate binds
to, where every other path in the TLS subsystem binds by the certificate's own SAN set.

- `upload_manual` (`crates/core/src/oi/handler/tls.rs:355`) sets the row's `hostname`
  column to the leaf's first SAN — documented there as "just the first SAN, kept as a
  primary label for display" — and supersedes on that label.
- `csr_upload_cert` (`crates/core/src/oi/handler/tls.rs:578`) leaves `hostname` as the name
  the CSR was begun for and supersedes on *that*.
- Both serving lookups (`store.rs:411`, `state.rs:346`) take an exact `hostname`-column
  match as a fast path before falling back to a SAN scan.

So a CA that signs a different name set than requested produces a row claiming to be
`www.example.com` while carrying a cert for `example.com`. The fast path then serves it for
`www.example.com`, and `supersede_other_active_for_hostname` has already retired the good
cert that did cover it. Adding a rejection check would close the hole, but it would leave
the CSR path as the one place where a certificate's identity is something other than its
SANs.

Manual policies are already retired (`store.rs:203-206` drops `strategy = 'manual'` rows on
read), so `r[tls.csr.flow]`'s "transition the hostname's strategy to manual" described a
mechanism the code had already abandoned — the same stale assumption in spec form.

## Decisions

- **Bind by the cert's SAN set on both upload paths.** A CSR upload is accepted whenever the
  cert matches the stored key and passes manual-upload validation; it then binds to whatever
  it covers. The requested hostname never decides what is served or what is superseded.
- **Retain the requested hostname in its own column**, rather than leaving it implicit in the
  retained `csr_pem`, so the operator interface can show "asked for X, got Y" without parsing
  a CSR. Costs migration 56.
- **An unmet request is a warning, not a rejection.** `request_not_covered` joins
  `self_signed` / `not_yet_valid` in the response warnings. The durable signal is the
  existing per-hostname rollup, which goes on showing the requested hostname as having no
  active certificate. Accepted limitation: the rollup only covers ingress-declared
  hostnames, so a CSR for a hostname no ingress declares leaves no standing signal.
- **Extract the shared store-an-operator-certificate step** rather than fixing
  `csr_upload_cert` in place, per the AGENTS.md rule that a contract implemented at several
  call sites is itself the bug. The two paths differ only in insert-vs-update and where the
  private key comes from.
- **The web UI is in scope.** Without it the unmet-request signal exists only in an upload
  response the operator sees once, which would undercut the reason for retaining the
  requested hostname at all.
- **Multi-name CSRs are out of scope** — `csr_begin` keeps taking a single hostname, and the
  coverage check is written over a set of one so widening it later is not a rewrite. Spawned
  as its own card.

## Checklist

- [x] Reframe `r[tls.cert.validation.san-coverage]` from an upload-rejection rule to the
      binding invariant, including the "never supersede a cert serving a hostname the
      arriving cert does not cover" clause that is the actual outage guard
- [x] Rewrite `r[tls.csr.flow]`'s acceptance bullet; drop the retired strategy-to-manual
      language; add the record-the-request and report-an-unmet-request bullets
- [x] Update `i[tls.cert.csr.upload-cert]` and `i[tls.cert.list]` in the interface spec
- [x] Update `w[routes.certificates]`: group by primary SAN, surface the requested hostname
      on CSR-origin rows, flag an uncovered request with the same treatment as the
      self-signed and near-expiry flags
- [x] Migration 56: add `requested_hostname` to `tls_certificates`, nullable, as a new
      block at the bottom of `db.rs` — no shipped block edited
- [x] Backfill `requested_hostname = hostname WHERE origin = 'csr'` in the same migration.
      Accurate by construction: before this change nothing ever rewrote `hostname` on a CSR
      row, so the pre-migration value *is* the requested name. Leaving it null would discard
      information the runtime holds
- [x] Persist `requested_hostname` on `csr_begin`; carry it through `row_to_certificate`,
      the `TlsCertificate` struct, and `tls.cert.list`
- [x] Extract the shared "store a validated operator certificate" step (primary SAN, metadata,
      chain PEM) as `StoredCert::from_validated`, and route both `upload_manual` and
      `csr_upload_cert` through it
- [x] `csr_upload_cert`: rewrite `hostname` to the issued cert's primary SAN on activation,
      supersede on that, and delete the incorrect comment about SPKI implying SAN coverage
- [x] Add the post-hoc `parse::san_covers(&issued_sans, &requested)` check emitting
      `request_not_covered`
- [x] `Certificates.tsx`: surface `requested_hostname` on CSR-origin rows and flag an
      uncovered request alongside the existing self-signed / near-expiry flags, so the row
      has one flag vocabulary rather than two
- [x] Tracey annotations on the new code; `tracey query status` clean
- [x] `cargo clippy`, `cargo fmt`, full workspace tests, frontend `tsc -b` and vitest

## Done beyond the original checklist

Each of these came out of the work rather than being planned, and each is recorded here
rather than left for a reader to find in the diff.

- **`CERT_COLUMNS`.** The certificate column list was written out by hand in four queries,
  read positionally by one function. Adding a column to three of the four would have shifted
  every index silently. Written once now.
- **`NewCertificate`.** `insert_certificate` took eleven positional arguments behind an
  `#[expect(clippy::too_many_arguments)]`, four of them `Option`s. A twelfth adjacent
  optional name — `requested_hostname`, right next to `hostname` — is precisely the pair a
  positional call gets the wrong way round, so the arguments became a struct. Nine call
  sites updated.
- **`request_covered` on `tls.cert.list`.** The web UI has to flag an uncovered request, and
  the listing did not carry enough to decide it. Returning the SAN list instead would have
  put a second implementation of the RFC 6125 wildcard rule in TypeScript, so the runtime
  decides it. Null where there is nothing to decide, and also where the stored PEM will not
  parse — which is not the same answer as "does not cover". Spec updated to match.
- **rcgen `x509-parser` feature, dev-dependency only.** Signing over a CSR's public key needs
  rcgen's CSR parser, which is feature-gated. Added under `[dev-dependencies]`, so under
  resolver 3 the production build is unchanged. No new external dependency: the crate
  already depends on `x509-parser` directly.
- **Schema-version assertions.** Two `runtime::db::tests` pin the migration count; bumped
  from 55 to 56.

## Review round 1

Three things came back, two of them real defects in this card's own work.

- **Supersession compared labels, not SAN sets** (critical, confirmed). Retiring every active
  row sharing the arriving certificate's primary SAN strands any name those rows carried and
  the arriving one does not: a CSR for `www.example.com` signed as
  `[example.com, www.example.com]` would retire an incumbent carrying
  `[example.com, shop.example.com]`, leaving `shop.example.com` with nothing. This is exactly
  what the clause added to `r[tls.cert.validation.san-coverage]` this round forbids, so the
  code contradicted the spec written alongside it, and the doc comment asserting the
  guarantee was simply wrong. `supersede_other_active_for_hostname` now takes the arriving
  SAN set and retires a candidate only when it covers every name that candidate serves. Fixed
  in the store rather than at the four call sites, so ACME and Tailscale issuance — which had
  the same hazard against a manual cert with extra SANs — are covered too.

- **The serving fast path trusted the label** (root cause behind the flagged test). The
  reviewer was right that the serving tests never built the row they were written to guard
  against, and the reason they could not is that the fast path had no defence to test: it
  matched the `hostname` column and returned. Making the write path label rows correctly does
  nothing for rows written before this change, which a database upgraded from an affected
  deployment still holds. The label is now an index hint confirmed against the certificate
  before it is trusted, falling through to the coverage scan when it does not hold. That
  satisfies the spec's serving clause unconditionally, and heals legacy mislabelled rows at
  read time — which is why no label-repair migration was added: such a row is no longer
  served for a name it cannot carry, and `request_covered` surfaces it as
  "request not covered" in the listing so an operator can see it.

- **Chain parse per listed row** (suggestion). `parse_chain` re-encodes the whole chain and
  allocates SPKI, serial and AKI bytes to answer a coverage question that needs none of them.
  Added `parse::leaf_san_dns_names` and `parse::cert_covers`, which read the leaf and stop,
  and routed the listing, both serving matchers, and supersession through them. Coverage is
  computed rather than persisted: a stored boolean would be derived state able to drift from
  the certificate it describes.

Test fixtures across `store`, `state` and `serve` carried placeholder PEMs like `"PEM"`. Now
that a row's label is confirmed against its certificate, those rows are unservable by
construction, so the fixtures build real certificates for the name they are stored under.

Not actioned: the below-threshold note that `CERT_COLUMNS` turned four `&'static str`
statements into per-call `format!` allocations. True, but the same path now does an X.509
leaf parse, which dominates a ~200-byte allocation by orders of magnitude; a macro to recover
it would cost more in readability than it returns.

## Review round 2

Three of the seven were already fixed in round 1's response — the listing's chain parse (now
`parse::cert_covers`), the tests that could not fail (rewritten to build the mislabelled row),
and the migration not repairing labels (answered by the fast path confirming the label against
the certificate, which is the fix that comment itself proposes). The rest were real.

- **Supersession could downgrade a working certificate** (critical, confirmed). Round 1
  narrowed supersession to certificates the arriving one fully covers, which stopped names
  being stranded but not a serviceable certificate being replaced by a worse one.
  `validate_upload` accepts self-signed and not-yet-valid leaves as warnings, and the serving
  lookups filtered `not_after` but never `not_before` — so a CA-chosen SAN set could retire a
  trusted, valid certificate in favour of one clients reject.

  This is the second narrowing of the same function, which is the signal that it had no
  articulated rule to narrow *towards*. So the rule is now written down in
  `r[tls.cert.validation.san-coverage]` and implemented once: a certificate supersedes another
  only when it replaces it in full — covers every hostname it serves, is inside its own
  validity window, and is not self-signed unless the incumbent already was. Alongside it,
  `r[tls.cert.serve]` now says a certificate outside its validity window is not served at
  either end, which is what makes staging a cutover ahead of `notBefore` mean anything.

  The function now reads the arriving certificate back from its own row instead of taking its
  properties as arguments, so what is compared is what was actually stored — and the SAN
  plumbing round 1 added at four call sites goes away again.

- **The label invariant was documented more strongly than it is enforced.** The doc comments
  claimed a row's `hostname` is the certificate's primary SAN, but ACME and Tailscale issuance
  label rows with the name they issued for. What the serving lookups actually need is weaker
  and true everywhere: the label is *a name the certificate covers*. Narrowed the wording on
  `TlsCertificate::hostname`, `NewCertificate::hostname`, `i[tls.cert.list]`,
  `w[routes.certificates]` and the TS type rather than relabelling the issuer paths, which
  would change their behaviour to no end. The spec itself never claimed the stronger form.

- **`StoredCert` rebuilt `CertMetadata` field by field** when `parsed.metadata` already is one.
  A new field would have been silently dropped there while every other path carried it.
  Collapsed to a clone.

Not actioned, both below threshold and reasoned rather than dismissed:

- *Wildcard certificates now take the scan path on serving lookups.* True, and a consequence of
  binding by SAN — but not new: every manual wildcard upload has always been labelled with its
  primary SAN and resolved this way. Making it cheap needs a persisted SAN list or a memoised
  resolution with invalidation, which is a caching design rather than this card's bug. The scan
  and both fast paths now use the leaf-only parse, which cuts the per-row cost meaningfully in
  the meantime. Worth its own card.
- *A renewal whose primary SAN differs from the incumbent's label leaves the old row active.*
  Real but benign: resolution picks the newest covering certificate, so serving is correct and
  the stale row is untidy history rather than an outage. Fixing it means superseding by
  coverage rather than by label, which would also let a wildcard retire every specific
  certificate under it — a behaviour change beyond this card.

## Review round 3

Fourteen suggestions, no criticals. The falling severity is the useful signal, but the
recurring location is the more useful one: every round has landed on the same thing. Round 1,
supersession trusted the row label. Round 2, the label invariant was documented more strongly
than any path enforced it, and the serving fast path trusted the label. Round 3, the fast path
is redundant dispatch with its own ordering. That is one defect being approached from three
sides, not three defects, so this round removes the thing rather than narrowing it again.

- **The exact-label fast path is gone from both matchers.** It existed to skip a parse; once it
  had to confirm the label against the certificate it no longer skipped anything on a miss, and
  it re-parsed on the way to a scan that parsed again. Worse, it was a second policy: it ordered
  by `id DESC` and preferred a label match over a *newer covering* certificate, contradicting
  `r[tls.strategy.manual]`'s "the most recently created active row wins". Two orderings for one
  question is where the next mislabelling hides. Both matchers are now a single newest-first
  coverage scan, and a row's label takes no part in deciding what is served — which is what this
  card has been about from the start.

  The cost is that every lookup scans rather than hitting an index. That is the trade the repo's
  own rule asks for ("a subsystem with a central decision function admits no dispatch before the
  decision"), the scan uses the leaf-only parse, and the rows are operator-scale. Making it fast
  again means a persisted SAN list or a memoised resolution — still worth its own card, now more
  so.

- **CSR activation is transactional and rechecks its precondition.** The handler reads the row,
  decrypts its key and validates the certificate outside any transaction, then wrote in a second
  call. A `csr/cancel` landing in that window left the update matching no rows while the
  supersession still ran — retiring incumbents for a row that no longer existed, and returning
  success. `store::activate_pending_csr` now does the update guarded on `state = 'csr_pending'`,
  supersedes only if it matched, and commits; the handler reports `requirements_invalid` when it
  did not. The same guard closes the concurrent-double-upload window. With CSR activation owning
  its own function, `update_certificate` drops the label parameter round 2 added to it.

- **`created_at` moves to activation time.** A CSR row is created when the request is begun, but
  no certificate exists then. With resolution ranking by `created_at`, a certificate that arrived
  today would otherwise lose to one stored yesterday because its request predates it.

- **One definition of "the DNS names in this leaf".** The SAN-only parse added in round 2 copied
  `parse_chain`'s extension walk, so a fix to one would silently miss the other and the listing
  and validation could disagree about the same certificate. Both now call a private
  `san_dns_names`.

- **Two silent "could not tell" branches spoken out loud.** A candidate whose certificate cannot
  be parsed is skipped by supersession and by resolution; an active row in that state is both
  un-retirable and unservable, with nothing saying why. Both branches now `warn!`. And
  `served.iter().all(...)` was vacuously true for an empty SAN set, retiring a candidate on the
  strength of names never read — guarded.

- **CSR upload dialog copy.** It still told the operator the runtime checks "SAN coverage", which
  is no longer a check that rejects. It now says the certificate binds to the domains its own
  SANs cover, which may not be the one requested.

Not actioned: `request_covered` collapsing "nothing to decide" and "could not parse" into null.
The spec added this round already says so explicitly, both mean "no flag" to the operator, and
the unparseable case now warns in the log where it is actionable.

## Review round 4

Four of the comments describe code round 3 deleted — the exact-label fast path, its
`unwrap_or(false)`, the scan running only on fast-path misses, and `update_certificate`'s label
parameter. Reviewed against the previous revision, so not actioned. The rest were real, and the
critical one was a gap this card opened.

- **The control-plane matcher ignored the validity window the serving path had started
  enforcing** (critical). Round 2 made serving skip a certificate staged ahead of its
  `notBefore`; `state::find_active_for_hostname` — which feeds the rollup, `decide`, and the
  renewal scheduler — was not changed to match. So an operator staging a cutover got a hostname
  that serving returned 204 for while the control plane reported it covered, scheduled no
  issuance, and filed no fault. Staged certificates are now skipped there too. Expiry
  deliberately stays unfiltered in that matcher, because the renewal scheduler has to see an
  expiring certificate in order to renew it — the two halves of the window are not symmetric
  here, and the doc comment says why.

- **Supersession returned `Ok(0)` for "cannot tell" and "nothing to retire" alike.** AGENTS.md is
  explicit: if failure and a definite negative produce the same value, split the type. A stored
  chain that cannot be read back now warns and errors rather than reporting a clean pass, so a
  replaced certificate cannot quietly stay active beside its replacement.

- **`r[tls.cert.validation.san-coverage]` had grown four rules under one id** across these
  rounds. The supersession rule — the one that grew most — moves to its own
  `r[tls.cert.supersede]`, with the impl and verify annotations following it, and the interface
  spec's three "supersedes any prior active certificate" sentences now point at it instead of
  describing a primary-SAN match that no longer decides anything.

- **Six copies of the self-sign test helper**, three added by this PR. Now one
  `runtime::tls::test_support::self_signed_pem`, used by `store`, `state`, `serve` and
  `expiring`.

Not actioned:

- *No clock-skew allowance on `not_before`.* Real in principle, but CAs backdate `notBefore`
  precisely for this (Let's Encrypt by an hour), and a staged upload is deliberate. Inventing a
  tolerance here would be guessing at a number.
- *Legacy mislabelled rows still display their old label.* Cosmetic since round 3: the label no
  longer decides anything, and `request_covered` already flags those rows as "request not
  covered" in the listing.
- *`request_covered` parses per CSR row on an unpaginated listing.* Cut substantially in round 2
  by the leaf-only parse; the rest is the pruning-superseded-history question, which is its own
  card.

## Review round 5 — and the pattern across rounds

Fixed this round, all confirmed against the branch:

- **Resolution and supersession disagreed about trust** (critical). Supersession refuses to
  retire a CA-issued certificate for a self-signed one; resolution took the newest covering
  certificate with no such test. So a self-signed `*.example.com` upload retired nothing — the
  guard worked — and was then served for every subdomain anyway, shadowing a valid ACME
  certificate that stayed active and never got handed out. The exact-label fast path used to
  make this impossible, so this is a regression from removing it in round 3.

  `r[tls.strategy.manual]`'s precedence sentence now states the full rule and
  `runtime::tls::resolve` implements it once for both matchers: an exact SAN beats a wildcard
  (RFC 6125 §6.4.4), a CA-issued certificate beats a self-signed one, and only then does the
  newest win. None of it reads the row label. Ranking every candidate rather than taking the
  first means the serving scan now selects only what ranking needs and re-fetches the winner,
  so walking the table no longer materialises every stored PEM and encrypted key.

- **Insert-then-supersede was atomic on one path of four.** `activate_pending_csr` got a
  transaction in round 3; `upload_manual`, ACME and Tailscale issuance did not, and supersession
  can now fail outright rather than merely retiring nothing. The insert would commit, the
  retirement would not, and the caller would report failure — on the ACME path getting the
  attempt recorded failed and the coordinator re-issuing against the CA. `insert_and_supersede`
  now wraps all three.

- **Supersession took a label it already had.** It reads the arriving row back precisely so what
  is compared is what was stored, then took `hostname` as a parameter anyway — two sources of
  truth, and a caller passing anything else would retire rows under a label the certificate does
  not carry. Dropped; it uses `arriving.hostname`.

- **A 2027 time bomb.** `insert_test_cert` and the `serve` fixture hard-code
  `not_after: 1_800_000_000` (15 Jan 2027). Harmless until this PR put a validity gate on
  supersession and resolution; now those tests would have gone red on that date. Both are
  relative to now.

- **Doc and annotation placement in `parse.rs`** — the block describing the public PEM entry
  point, and the `r[impl]` with it, were stacked on the private extension walk, leaving
  `leaf_san_dns_names` undocumented. Moved.

### The pattern, and why it should stop here

Five rounds, and each round's critical has been caused by the previous round's fix:

| Round | Critical | Caused by |
|---|---|---|
| 1 | supersession strands names | the original bug |
| 2 | supersession downgrades a working cert | round 1's narrowing |
| 3 | *(none)* | — |
| 4 | control plane ignores the validity window | round 2's `notBefore` serving filter |
| 5 | a self-signed cert shadows a valid one | round 3's fast-path removal |

Every finding has been correct, and each fix has been right for the defect in front of it. But a
card filed to add one missing `san_covers` check has ended up rewriting TLS resolution
precedence, supersession semantics, the serving validity window, and the control-plane matcher —
and each of those is load-bearing for serving, renewal, the rollup and issuance at once. The
scope grew because each round's finding was genuinely a defect, not because any round was wrong
to raise it.

The remaining open items are no longer local bug fixes. They need a decision about how much of
the TLS resolution path this card should own, which is the user's call, not the reviewer's and
not mine:

- **The serving lookup is an unfiltered scan on a remotely-driven path.** Raised three times now,
  most recently framed as an amplification vector: unknown SNI is the most expensive case, and
  nothing memoises it. Bounding it means a resolution cache with invalidation on certificate
  writes, or SQL pre-filtering — a caching design, not a bug fix.
- **`compute_state` now parses per hostname per tick.** `renewal::due_hostnames`,
  `expiring::compute_desired` and the OI rollup each loop over hostnames calling it, so the cost
  is H×C leaf parses. The fix is to parse each certificate's SAN list once per snapshot, which
  means changing `Snapshot`.
- **Skipping not-yet-valid certificates can drive repeated ACME issuance.** A freshly issued
  certificate whose `notBefore` has not arrived on this host's clock reads as uncovered, so the
  coordinator issues again. Either a skew tolerance (a number to be chosen, not derived) or
  `decide` distinguishing "staged" from "absent".
- **The self-signed guard does not catch a private CA.** `self_signed` is issuer DN == subject
  DN, so a certificate from an untrusted internal CA passes it and can retire a publicly trusted
  incumbent. Real chain validation against a trust store is a feature in its own right; the
  alternative is narrowing the spec wording to what is actually enforced.

## Review round 6

Two of the three criticals are items flagged at the end of round 5 as needing a decision; the
third is a regression from round 5's ranking change. Fixed the regressions and the local items;
left the two that are still open decisions.

- **Skipping not-yet-valid certificates drove an unbounded ACME re-issuance loop** (critical, and
  a regression from round 4's fix). `debounce_until` only debounces after a *failed* attempt, so
  a successful issuance is never debounced: a certificate whose `notBefore` had not arrived on
  this host's clock read as absent, and the coordinator issued another every tick.

  The round-4 fix was right that the control plane must not report a staged certificate as
  serving, and wrong to conclude it should report nothing. Staged is a third state, so it is one
  now: `compute_state` reports the servable certificate as `active_cert` and, when there is none,
  when a stored certificate starts serving. `decide_acme_dns` returns `Scheduled` at that time
  rather than issuing. Both round-4's property and this one hold.

- **`Rank` had no notion of expiry** (regression from round 5). Safe in the serving lookup, which
  pre-filters expired rows, but not in the control-plane matcher, which deliberately keeps them
  so the renewal scheduler can see them. With `exact` outranking recency, an expired certificate
  naming the hostname exactly beat the newer valid wildcard actually being served — so the
  expiry sweep would fault a hostname that was correctly covered. `unexpired` now ranks above
  everything else; it is a no-op on the serving side.

- **The last silent "could not tell".** `request_covered` discarded its parse error while every
  other such branch warns, and null reads as "no flag" to the operator — the same as a met
  request. It warns now.

- **Two queries reading more than they use.** Supersession pulled each candidate's PEM *and*
  encrypted key to read three fields. Narrowed, as the serving scan already was.

### Still open, and now escalating

Round 5 flagged four items as needing a decision about how much of the TLS resolution path this
card should own. That question has not been answered, and two of the four came back this round
as criticals rather than suggestions:

- **The serving lookup is an unfiltered scan on the handshake path.** Now argued with the detail
  that makes it more than performance: `DbHandle` is a single serialized worker thread, so every
  lookup blocks reconcile, the OI and issuance for its duration, and unknown SNI — remote
  attacker-controlled — is the most expensive case, scanning and parsing every active row to
  return nothing. The reviewer asks for a bound before merge: an indexed pre-filter, a
  `tls_cert_sans` table making coverage a SQL join, or memoised resolution. Any of the three is a
  design, and the deferral in round 3 was made when this was still a fallback path rather than
  the only one.

- **A CSR-signed certificate binds by the CA's chosen SANs.** This is the card's central design
  decision, taken deliberately at interview: bind by what the certificate carries, report an
  unmet request as a warning plus the rollup rather than a fault. The reviewer's point is a
  consequence of it — a CA that returns names outside the request gets those names served, and a
  substituted name no ingress declares leaves no standing signal, because the rollup only covers
  ingress-declared hostnames. Reversing it means either rejecting out-of-request SANs or filing
  a fault, both of which were considered and not chosen. Not something to overturn without the
  decision being revisited.

Also still open from round 5 and unchanged: `compute_state` parsing per hostname per tick, and
the self-signed guard not catching a private CA.

## Round 6 follow-up: the CA-chosen SAN question, settled

The reviewer read a CSR certificate binding by the CA's SAN set as the CA controlling which
hostnames this host serves. Two things close it.

**Nothing serves a name no ingress declares.** `build_policy` gives Caddy an explicit `subjects`
list — ingress-declared TLS vhosts plus warm-cert hostnames — with `on_demand: false`, so Caddy
never asks for a certificate for a name outside that set, and the serving endpoint is token-gated
besides. A substituted SAN for an undeclared name is an unused row, not a served certificate.
This also deflates the amplification framing of the serving-scan critical: random SNI does not
reach the lookup, so the scan is bounded by declared hostnames rather than by attacker input.

**The remaining case is the intended behaviour, confirmed by the user.** A CSR begun for
`a.doma.in` that comes back as `*.doma.in` should be taken up by every name it covers —
`b.doma.in` included. That is auto-binding working, not a substitution attack.

Checking that against the ranking turned up a defect in round 5's fix that no review round had
reported: `Rank` ordered `exact` above `trusted`, so a **self-signed certificate naming the
hostname exactly outranked a trusted wildcard** — round 5's critical in a new shape, and it
would have stopped exactly the takeover the user wants. Trust now ranks above specificity: a
certificate clients reject is no use for a hostname however precisely it names it, so a wildcard
they accept has to be able to take over from a dedicated certificate they do not. Spec
precedence reordered to match.

Where a hostname still holds a valid, trusted, dedicated certificate, that one goes on serving
and the wildcard takes over when it expires — `unexpired` ranks top. That is the wanted
behaviour in both directions.

Spawned **A5**: renewing a CSR certificate starts from the original request rather than from
what the CA issued, so an operator who received a wildcard retypes the name that did not get
issued last time. Depends on W4 for multi-SAN certificates, and carries an open choice between
re-requesting the issued set and re-requesting the needed set.

## The resolution-cost split

The control-plane half is done here; the serving half is **B5**.

`state::Snapshot` now parses each active certificate's SAN list once when it is built, and both
in-memory matchers read through that. Resolution is per hostname and every caller loops over
hostnames — renewal over due certificates, the expiry sweep over ingress targets, the operator
rollup over the whole set — so parsing inside the matcher cost one X.509 parse per hostname per
certificate, per tick. The snapshot is immutable for the length of those loops, so the parse
belongs to the snapshot.

The memo is not a source of truth. A certificate missing from the map is parsed on the spot, so
a snapshot assembled without it is slower and never wrong — the same discipline this subsystem
learned the hard way about the `hostname` label, applied before rather than after.

**B5** covers the serving lookup, which still scans. Its card records the thing worth not
losing: an earlier round argued the scan as a remote denial-of-service vector, and that framing
does not hold. Caddy is configured with an explicit `subjects` list and `on_demand: false`, and
the endpoint is token-gated, so arbitrary remote SNI never reaches the lookup. It is a
performance card, and its shape — a `tls_cert_sans` index that narrows candidates but is
confirmed against the certificate before anything is served — is written down there.

## Noted, not actioned

- `TlsCertState::Failed` is never constructed anywhere in the tree. Its mention in
  `update_certificate`'s doc comment is gone — that function's contract is now written to
  what it does — but the variant itself, and the `CHECK` constraint admitting it, are left
  alone. Worth a sweep, but it is not this card's.
- `i[tls.policy.list]` still documents a `"manual"` strategy carrying `cert_id`, which
  `store.rs:203-206` drops on read. A pre-existing spec/code divergence, and deciding
  whether manual policies exist at all is larger than this card.
