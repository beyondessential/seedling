-- r[impl tls.cert.serve]
-- r[impl tls.canopy.revoked]
-- An index for the certificate queries that run on a timer.
--
-- The only index on `tls_certificates` is on `(hostname, state)`, and the two
-- queries asked on every tick name no hostname: the reconciler asks which
-- stored certificates could be served right now, and the Canopy report asks
-- which of the certificates it holds are still active. Both had to scan the
-- whole table, which keeps every superseded row and each row's chain and
-- encrypted key, so the scan grows with every renewal the instance has ever
-- done rather than with what it serves.
CREATE INDEX IF NOT EXISTS tls_certificates_state_origin
    ON tls_certificates (state, origin);
