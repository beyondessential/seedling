-- r[impl tls.csr.renew]
-- A CSR may be begun as the renewal of another certificate, and records which.
--
-- Deleting the renewed certificate leaves the renewal standing with nothing to
-- retire. At most one renewal of a certificate is pending at a time; the index
-- enforces it where two begins could otherwise both pass a check made first.
ALTER TABLE tls_certificates
    ADD COLUMN renews INTEGER REFERENCES tls_certificates(id) ON DELETE SET NULL;

CREATE UNIQUE INDEX tls_certificates_one_pending_renewal
    ON tls_certificates(renews)
    WHERE state = 'csr_pending' AND renews IS NOT NULL;
