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
- [x] An arriving certificate that is not yet within its validity window retires nothing:
      staging a cutover must not retire the certificate currently serving the name
      (verifies spec: `tls.cert.validation.san-coverage`)
- [x] A candidate staged ahead of its own validity window is not retired: it was stored for
      a cutover, and retiring it leaves nothing to take over (verifies spec:
      `tls.cert.supersede`)
- [x] A shorter-lived arrival does not retire a longer-lived incumbent, which would cost the
      hostname its TLS at the arrival's expiry (verifies spec: `tls.cert.supersede`)
- [x] A self-signed arriving certificate does not retire a CA-issued incumbent — clients
      accept the incumbent and would reject the replacement (verifies spec:
      `tls.cert.validation.san-coverage`)
- [x] A certificate whose `notBefore` has not arrived is not served (verifies spec:
      `tls.cert.serve`)
- [x] Nor is it reported as the hostname's active certificate by the control-plane matcher —
      otherwise the rollup calls the hostname covered while handshakes for it fail, and
      nothing schedules a fix (verifies spec: `tls.cert.serve`)
- [x] A certificate the runtime cannot read back after storing it reports a failure rather
      than a count of nothing retired (verifies spec: `tls.cert.supersede`)

## Serving a mislabelled row

- [x] A row labelled `www.example.com` whose certificate carries only `example.com` — the
      shape a database upgraded from before this change can hold — is not served for
      `www.example.com` (verifies spec: `tls.cert.validation.san-coverage`)
- [x] With such a row present, a certificate that genuinely covers `www.example.com` is the
      one handed out, even though the mislabelled row is newer and matches the label exactly
- [x] Both matchers are covered: `store::find_active_for_hostname` and the in-memory
      `state::find_active_for_hostname`

## CSR activation is atomic

- [x] Activating a row that was cancelled mid-upload writes nothing and retires no incumbent —
      without the precondition the update matches no rows while supersession still runs
      (verifies spec: `tls.csr.flow`)
- [x] A second activation of an already-active row reports that nothing was pending rather
      than relabelling and superseding again

## Resolution precedence

- [x] A certificate whose SAN names the hostname exactly is served in preference to a newer
      wildcard that merely covers it (verifies spec: `tls.strategy.manual`)
- [x] A CA-issued certificate is served in preference to a newer self-signed one, so
      resolution cannot serve what supersession just refused to let retire anything
      (verifies spec: `tls.strategy.manual`)
- [x] Trust ranks above specificity: a trusted wildcard takes over a hostname whose only
      dedicated certificate is self-signed, so a wildcard obtained for one name is picked up
      by the others it covers (verifies spec: `tls.strategy.manual`)
- [x] With neither distinction in play, the most recently created still wins

## Staged certificates

- [x] A certificate whose `notBefore` has not arrived yields a scheduled decision at that
      time, not another issuance — a successful issuance is never debounced, so reporting it
      as absent would re-issue every tick (verifies spec: `tls.cert.serve`)
- [x] It is still not reported as the hostname's active certificate, so the rollup does not
      claim a hostname is covered while handshakes for it fail
- [x] A staged renewal suppresses re-issuance even while the expiring incumbent is still
      active and due — the loop is reachable through the incumbent-present path too
      (verifies spec: `tls.cert.serve`)
- [x] A certificate staged far ahead does not suppress issuance: the hostname has no TLS in
      the meantime, so it needs one now and the staged certificate takes over later
      (verifies spec: `tls.cert.serve`)

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
- [x] Stored certificates group by label, so a relabelled CSR row groups with the other
      certificates for the name it actually covers (verifies spec: `routes.certificates`)
- [ ] `seedling tls csr upload-cert` surfaces the `request_not_covered` warning in its
      output. Not covered: `print_result` prints the whole response JSON, so the warning
      reaches the operator, but nothing asserts it
