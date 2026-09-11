# Test cases: CSR cert upload binds by SAN coverage

Per the audit note, these are exercisable with `TestOi` and an in-memory DB: begin a CSR,
build a cert carrying the CSR's public key but an arbitrary SAN set, upload, assert. The
CSR's private key never leaves the runtime, so the tests sign over its public key with a
separate CA, exactly as a real CA does.

## Binding

- [x] A CSR begun for `www.example.com` whose signed cert carries SAN `example.com` only is
      accepted, and the row's primary-SAN label becomes `example.com` (verifies spec:
      `tls.cert.validation.san-coverage`)
- [x] That upload does **not** supersede a pre-existing active certificate covering
      `www.example.com`; the prior cert is still active and still served for that hostname
      (verifies spec: `tls.cert.validation.san-coverage`)
- [x] Serving lookup for a hostname returns the certificate that covers it rather than a
      newer one that does not (see "Serving a mislabelled row" below for the sharper form of
      this, which pins the invariant rather than restating it)
- [x] A CSR begun for `foo.example.com` whose cert is issued for `*.example.com` is accepted
      and does cover the request — no `request_not_covered` warning (verifies spec:
      `tls.cert.validation.san-coverage`)
- [x] A CSR begun for `a.b.example.com` whose cert is issued for `*.example.com` is accepted
      but warns: one wildcard label does not reach two (verifies spec:
      `tls.cert.validation.san-coverage`)
- [x] An upload whose issued cert covers the request exactly supersedes a prior active cert
      with the same primary SAN, as before

## Supersession

- [x] An arriving certificate does not retire an incumbent that shares its primary-SAN
      label but also serves a name the arriving certificate does not cover — a CSR for
      `www.example.com` signed as `[example.com, www.example.com]` must leave an incumbent
      carrying `[example.com, shop.example.com]` active, or `shop.example.com` is stranded
      with no certificate at all (verifies spec: `tls.cert.validation.san-coverage`)
- [x] The renewal case still works: an arriving certificate that covers everything the
      incumbent carried does retire it (verifies spec: `tls.cert.validation.san-coverage`)
- [x] A candidate whose stored certificate cannot be parsed is left active rather than
      retired — unable to tell what it serves is not the same as knowing it is replaced

## Serving a mislabelled row

- [x] A row labelled `www.example.com` whose certificate carries only `example.com` — the
      shape a database upgraded from before this change can hold — is not served for
      `www.example.com` (verifies spec: `tls.cert.validation.san-coverage`)
- [x] With such a row present, a certificate that genuinely covers `www.example.com` is the
      one handed out, even though the mislabelled row is newer and matches the label exactly
- [x] Both matchers are covered: `store::find_active_for_hostname` and the in-memory
      `state::find_active_for_hostname`

## Requested hostname

- [x] `tls.cert.list` reports `requested_hostname` for a CSR-origin row, and it survives the
      upload that rewrites the primary-SAN label (verifies spec: `tls.csr.flow`)
- [x] `requested_hostname` is null for manual-origin rows (verifies spec: `tls.cert.list`)
- [ ] `requested_hostname` is null for acme_dns-origin rows. Not covered: producing an
      ACME-issued row needs a directory to issue against, so this one wants an integration
      test rather than a unit one
- [ ] A database migrated from version 55 opens, and pre-existing CSR rows read back with
      their requested hostname backfilled. Not covered: `Db::open_in_memory` runs the whole
      migration chain, so there is no harness for opening at an older version

## Warnings and rejections

- [x] `request_not_covered` appears in the upload response warnings when the issued cert
      omits the requested hostname, alongside any `self_signed` / `not_yet_valid`
      (verifies spec: `tls.cert.csr.upload-cert`)
- [x] A cert whose SPKI does not match the stored CSR key is still rejected with
      `requirements_invalid`, and the row stays `csr_pending`
- [x] An expired cert is still rejected, and the row stays `csr_pending`
- [x] A cert carrying no DNS SANs is rejected (verifies spec:
      `tls.cert.validation.san-coverage`)
- [x] Every rejection path leaves both the prior active certificate and the pending row
      untouched — asserted on the listing and on the CSR still being retrievable, not just
      on the error code

## Manual path unchanged

- [x] Manual upload behaviour is unchanged by the extraction: primary SAN labelling,
      supersession, warnings, and auto-binding all still hold (verifies spec:
      `tls.strategy.manual`)

## Operator surfaces

- [x] The web UI certificates table shows the requested hostname on a CSR-origin row and
      flags an uncovered request with the same treatment as the self-signed and near-expiry
      flags (verifies spec: `routes.certificates`)
- [x] A row whose certificate does meet its request shows neither the requested name nor the
      flag, so the extra line only appears where it says something
- [x] Stored certificates group by primary SAN, so a relabelled CSR row groups with the
      other certificates for the name it actually covers (verifies spec:
      `routes.certificates`)
- [ ] `seedling tls csr upload-cert` surfaces the `request_not_covered` warning in its
      output. Not covered: `print_result` prints the whole response JSON, so the warning
      reaches the operator, but nothing asserts it
