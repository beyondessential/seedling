-- r[impl tls.cert.retry-block]
-- Auto-set retry blocks are no longer used: the runtime now drives
-- issuance from the reconciler tick, not from Caddy handshakes, and a
-- failed attempt is debounced via the attempt log rather than a separate
-- state row. Drop the legacy auto rows so the retry-blocks table reflects
-- only operator-set pauses going forward.
DELETE FROM tls_cert_retry_blocks WHERE set_by = 'auto';

-- r[impl tls.cert.force-retry]
-- Persistent "operator wants a fresh attempt" signal. The reconciler
-- consumes the row at the start of an issuance run (deleting it
-- atomically) so the request survives a daemon restart between the
-- operator clicking retry and the reconciler picking it up.
CREATE TABLE IF NOT EXISTS tls_cert_force_retry (
    hostname     TEXT PRIMARY KEY,
    requested_at INTEGER NOT NULL
);
