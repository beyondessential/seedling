-- r[impl tls.cert.ari]
-- ACME Renewal Information (RFC 9773): the CA returns a suggested
-- renewal window (start, end) for each active cert. We capture it at
-- issuance time and refresh it periodically; the renewal task picks
-- ARI's recommendation over the fixed 1/3-lifetime fallback when present.
ALTER TABLE tls_certificates ADD COLUMN ari_window_start INTEGER;
ALTER TABLE tls_certificates ADD COLUMN ari_window_end   INTEGER;
ALTER TABLE tls_certificates ADD COLUMN ari_polled_at    INTEGER;
