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

## Noted, not actioned

- `TlsCertState::Failed` is never constructed anywhere in the tree. Its mention in
  `update_certificate`'s doc comment is gone — that function's contract is now written to
  what it does — but the variant itself, and the `CHECK` constraint admitting it, are left
  alone. Worth a sweep, but it is not this card's.
- `i[tls.policy.list]` still documents a `"manual"` strategy carrying `cert_id`, which
  `store.rs:203-206` drops on read. A pre-existing spec/code divergence, and deciding
  whether manual policies exist at all is larger than this card.
