-- r[impl tls.csr.flow]
-- Record the hostname a CSR was requested for, separately from the row's
-- primary-SAN label.
--
-- `hostname` on a certificate row is a display label and the supersession
-- key: the certificate's own primary SAN. It was doing double duty on CSR
-- rows, where it held the name the operator asked for instead. A CA is free
-- to sign a name set other than the one requested, so the two are not the
-- same thing, and conflating them let a CSR upload label itself with — and
-- retire the certificate serving — a hostname it did not cover.
ALTER TABLE tls_certificates ADD COLUMN requested_hostname TEXT;

-- Backfill is exact rather than a guess: before this migration nothing ever
-- rewrote `hostname` on a CSR row, so its current value is the requested
-- name. Leaving these null would discard something the runtime already knows.
UPDATE tls_certificates
SET requested_hostname = hostname
WHERE origin = 'csr';
