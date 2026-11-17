-- When the runtime next fetches a certificate's ACME renewal information.
--
-- RFC 9773 §4.3 sets the schedule: the Retry-After of the last fetch, within
-- limits, or six hours after a failed one. A certificate whose information
-- was fetched before this column existed is due six hours after that fetch.
ALTER TABLE tls_certificates ADD COLUMN ari_next_poll_at INTEGER;

UPDATE tls_certificates
   SET ari_next_poll_at = ari_polled_at + 21600
 WHERE ari_polled_at IS NOT NULL;
