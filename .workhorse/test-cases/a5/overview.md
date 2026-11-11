# Renewing a CSR or manual certificate

## Runtime and interface

- [x] A renewal with no hostnames requests the certificate's issued SAN set, not its original request (verifies spec: tls.csr.renew)
- [x] A renewal uploaded with a different primary SAN still retires the certificate it renews (verifies spec: tls.csr.renew)
- [x] A renewal that does not cover everything the old certificate serves leaves it active (verifies spec: tls.cert.supersede)
- [x] A manual upload can be renewed, and names given replace the seeded set (verifies spec: tls.csr.renew)
- [x] Renewing an unknown id, a pending CSR, or with neither hostnames nor renews is refused (verifies spec: tls.cert.csr.begin)
- [x] A second pending renewal is refused, naming the first; cancelling the first frees it (verifies spec: tls.csr.renew)
- [x] The database refuses a second pending renewal of the same certificate (verifies spec: tls.csr.renew)
- [x] Deleting the renewed certificate leaves its renewal pending with no link (verifies spec: tls.csr.renew)
- [x] The certificate list reports `san_dns_names`, `renews` and `pending_renewal` (verifies spec: tls.cert.list)
- [x] The expiry fault names the certificate and the renew command (verifies spec: tls.fault.expiring)
- [x] `tls certs csr renew <id>` sends no hostnames unless given (verifies spec: tls.csr.renew)

## Web

- [x] Renew is offered on an expiring CSR or manual certificate and not on a healthy one (verifies spec: routes.certificates)
- [x] The Renew dialog is pre-filled with the SAN set, lists the hostnames served, and sends `renews` (verifies spec: routes.certificates)
- [x] A certificate with a pending renewal leads to it, and the pending row names what it renews (verifies spec: routes.certificates)

## Manual

- [ ] Renew a widened CSR certificate end to end against a real CA, upload the result, and check the old row is superseded and the fault clears
