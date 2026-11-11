# Renewing a CSR or manual certificate

## Technical notes

- The renewal link is a new `renews` column on `tls_certificates`, added in a new migration block. Deleting the renewed certificate nulls it.
- One pending renewal per certificate is enforced by the database (a partial unique index on `renews` where `state = 'csr_pending'`), not by a read-then-insert check, so two concurrent begins can't both succeed.
- Seeding happens server-side in `csr_begin` when `renews` is given without `hostnames`, by parsing the renewed row's `cert_pem`. An unparseable certificate is refused, never seeded as an empty set.
- On upload, `supersede_other_active_for_hostname` takes the renewed row as a candidate alongside the rows sharing the label, under the same full-replacement test.
- `tls.cert.list` already parses each PEM for `requested_not_covered`. `san_dns_names` comes from the same parse.
- CLI: `tls certs csr renew <id> [--hostname ...]`, calling `csr/begin` with `renews`.
- Web: the Renew action reuses `CsrBeginDialog`, pre-filled from `san_dns_names`. The hostnames the certificate serves come from `tls.hostname.list` rows whose `active_cert.id` matches.
- `cert_expiring_soon` description names the certificate id and the renew command.

## Build steps

- [x] Migration v66: `renews` column with `ON DELETE SET NULL`, partial unique index on pending renewals
- [x] Store: `renews` on `NewCertificate`/`TlsCertificate`; pending-renewal lookup; supersession takes the renewed row as a candidate
- [x] OI: `csr.begin` accepts `renews` and seeds from the SAN set; `cert.list` reports `san_dns_names`, `renews`, `pending_renewal`
- [x] Expiring fault names the certificate and the renew command
- [x] CLI: `tls certs csr renew <id>`
- [x] Web: Renew action within the expiry window, pre-filled dialog with served hostnames, pending renewal shown
- [x] Tests and docs
