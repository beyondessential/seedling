# Test cases: CSR cert upload binds by SAN coverage

Per the audit note, these are exercisable with `TestOi` and an in-memory DB: begin a CSR,
build a cert carrying the CSR's public key but an arbitrary SAN set, upload, assert.

## Binding

- [ ] A CSR begun for `www.example.com` whose signed cert carries SAN `example.com` only is
      accepted, and the row's primary-SAN label becomes `example.com` (verifies spec:
      `tls.cert.validation.san-coverage`)
- [ ] That upload does **not** supersede a pre-existing active certificate covering
      `www.example.com`; the prior cert is still active and still served for that hostname
      (verifies spec: `tls.cert.validation.san-coverage`)
- [ ] Serving lookup for `www.example.com` after such an upload returns the prior covering
      cert, not the newly uploaded one — exercise both `store::find_active_for_hostname` and
      the in-memory `state::find_active_for_hostname`, since the bug lived in their shared
      exact-hostname fast path
- [ ] A CSR begun for `foo.example.com` whose cert is issued for `*.example.com` is accepted
      and does cover the request — no `request_not_covered` warning (verifies spec:
      `tls.cert.validation.san-coverage`)
- [ ] A CSR begun for `a.b.example.com` whose cert is issued for `*.example.com` is accepted
      but warns: one wildcard label does not reach two (verifies spec:
      `tls.cert.validation.san-coverage`)
- [ ] An upload whose issued cert covers the request exactly supersedes a prior active cert
      with the same primary SAN, as before

## Requested hostname

- [ ] `tls.cert.list` reports `requested_hostname` for a CSR-origin row, and it survives the
      upload that rewrites the primary-SAN label (verifies spec: `tls.csr.flow`)
- [ ] `requested_hostname` is null for manual-origin and acme_dns-origin rows (verifies
      spec: `tls.cert.list`)
- [ ] A database migrated from version 55 opens, and pre-existing CSR rows read back with a
      null `requested_hostname` rather than failing

## Warnings and rejections

- [ ] `request_not_covered` appears in the upload response warnings when the issued cert
      omits the requested hostname, alongside any `self_signed` / `not_yet_valid`
      (verifies spec: `tls.cert.csr.upload-cert`)
- [ ] A cert whose SPKI does not match the stored CSR key is still rejected with
      `requirements_invalid`, and the row stays `csr_pending`
- [ ] An expired cert is still rejected, and the row stays `csr_pending`
- [ ] A cert carrying no DNS SANs is rejected (verifies spec:
      `tls.cert.validation.san-coverage`)
- [ ] Every rejection path leaves both the prior active certificate and the pending row
      untouched — assert on the full cert list, not just the error code

## Manual path unchanged

- [ ] Manual upload behaviour is unchanged by the extraction: primary SAN labelling,
      supersession, warnings, and auto-binding all still hold (verifies spec:
      `tls.strategy.manual`)

## Operator surfaces

- [ ] The web UI certificates table shows the requested hostname on a CSR-origin row and
      flags an uncovered request with the same treatment as the self-signed and near-expiry
      flags (verifies spec: `routes.certificates`)
- [ ] A manual-origin or acme_dns-origin row shows no requested hostname and no such flag
- [ ] Stored certificates group by primary SAN, so a relabelled CSR row groups with the
      other certificates for the name it actually covers (verifies spec:
      `routes.certificates`)
- [ ] `seedling tls csr upload-cert` surfaces the `request_not_covered` warning in its output
