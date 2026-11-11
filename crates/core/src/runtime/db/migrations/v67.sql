-- r[impl tls.cert.serve]
-- An index of every stored certificate's SAN DNS names, so resolving a
-- hostname narrows to the certificates that could cover it instead of parsing
-- every one.
--
-- It is an index and never a source of truth: whatever it selects is confirmed
-- against the certificate's own PEM before it is acted on.
--
--   * `tls_cert_index` has one row per certificate that has been indexed,
--     carrying whether its leaf is self-issued. `self_issued` is NULL when the
--     PEM could not be parsed: the certificate is indexed, as covering nothing,
--     so it is not parsed again on every pass.
--   * `tls_cert_sans` has one row per (certificate, lowercased SAN DNS name).
--     It references the index row rather than the certificate, so removing a
--     certificate's index row removes its names with it.
--   * A certificate with a PEM and no index row has not been indexed. That is
--     what the background sweep looks for.
--   * The trigger removes a certificate's index row whenever its PEM changes,
--     so a change that is not reindexed in the same transaction leaves the
--     certificate unindexed (which the sweep can see) rather than stale (which
--     it cannot).
--
-- A migration that rebuilds `tls_certificates` drops the trigger along with the
-- table and must recreate it. Dropping the table also cascades to the index,
-- which the sweep then rebuilds.

CREATE TABLE tls_cert_index (
    cert_id     INTEGER PRIMARY KEY
        REFERENCES tls_certificates(id) ON DELETE CASCADE,
    self_issued INTEGER
);

CREATE TABLE tls_cert_sans (
    cert_id INTEGER NOT NULL
        REFERENCES tls_cert_index(cert_id) ON DELETE CASCADE,
    san     TEXT NOT NULL,
    PRIMARY KEY (cert_id, san)
);

CREATE INDEX tls_cert_sans_san ON tls_cert_sans (san);

CREATE TRIGGER tls_certificates_pem_changed
AFTER UPDATE OF cert_pem ON tls_certificates
WHEN OLD.cert_pem IS NOT NEW.cert_pem
BEGIN
    DELETE FROM tls_cert_index WHERE cert_id = NEW.id;
END;
