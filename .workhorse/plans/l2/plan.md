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
- [ ] Migration 56: add `requested_hostname` to `tls_certificates`, nullable, as a new
      `version < N` block at the bottom of `db.rs` — never edit a shipped block
- [ ] Backfill `requested_hostname = hostname WHERE origin = 'csr'` in the same migration.
      Accurate by construction: before this change nothing ever rewrote `hostname` on a CSR
      row, so the pre-migration value *is* the requested name. Leaving it null would discard
      information the runtime holds
- [ ] Persist `requested_hostname` on `csr_begin`; carry it through `row_to_certificate`,
      the `TlsCertificate` struct, and `tls.cert.list`
- [ ] Extract the shared "store a validated operator certificate" step (primary SAN, metadata,
      chain PEM, supersede-on-primary-SAN) and route both `upload_manual` and
      `csr_upload_cert` through it
- [ ] `csr_upload_cert`: rewrite `hostname` to the issued cert's primary SAN on activation,
      supersede on that, and delete the incorrect comment at `tls.rs:605-608`
- [ ] Add the post-hoc `parse::san_covers(&issued_sans, &requested)` check emitting
      `request_not_covered`
- [ ] `Certificates.tsx`: surface `requested_hostname` on CSR-origin rows and flag an
      uncovered request alongside the existing self-signed / near-expiry flags, so the row
      has one flag vocabulary rather than two. Grouping is by primary SAN, which for
      relabelled CSR rows is a visible change
- [ ] Tracey annotations on the new code; `tracey query status` clean
- [ ] `cargo clippy`, `cargo fmt`

## Noted, not actioned

- `TlsCertState::Failed` is never constructed anywhere in the tree, and
  `update_certificate`'s doc comment still claims CSR validation failure transitions
  pending → failed. This change does not introduce the state either. Worth a sweep, but it
  is not this card's.
- `i[tls.policy.list]` still documents a `"manual"` strategy carrying `cert_id`, which
  `store.rs:203-206` drops on read. A pre-existing spec/code divergence, and deciding
  whether manual policies exist at all is larger than this card.
