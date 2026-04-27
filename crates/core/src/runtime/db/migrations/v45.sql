-- r[impl tls.cert.attempt-log]
-- Every cert-issuance attempt (on-demand, operator-triggered, or autonomous
-- renewal) lands here so operators have a history independent of whether
-- the attempt succeeded. `triggered_by` distinguishes the source:
--   on_demand : Caddy hit get_certificate and we ran issuance synchronously.
--   manual    : operator called /tls/certificates/issue-acme-dns.
--   renewal   : the autonomous renewal task ran issuance.
-- `cert_id` is set on success and points to the resulting tls_certificates row.
-- `error` is set on failure.
CREATE TABLE IF NOT EXISTS tls_cert_attempts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname     TEXT NOT NULL,
    triggered_by TEXT NOT NULL
        CHECK (triggered_by IN ('on_demand', 'manual', 'renewal')),
    started_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    outcome      TEXT NOT NULL
        CHECK (outcome IN ('pending', 'success', 'failure')),
    cert_id      INTEGER REFERENCES tls_certificates(id) ON DELETE SET NULL,
    error        TEXT
);

CREATE INDEX IF NOT EXISTS tls_cert_attempts_hostname_started
    ON tls_cert_attempts (hostname, started_at DESC);

-- r[impl tls.cert.retry-block]
-- Per-hostname auto-retry block. Set automatically when an issuance attempt
-- fails (so Caddy's repeated handshakes don't trigger an issuance loop) and
-- when operators want to pause issuance manually. Cleared automatically by
-- /tls/certificates/issue-acme-dns and by successful on-demand issuance.
CREATE TABLE IF NOT EXISTS tls_cert_retry_blocks (
    hostname    TEXT PRIMARY KEY,
    set_at      INTEGER NOT NULL,
    set_by      TEXT NOT NULL CHECK (set_by IN ('auto', 'operator')),
    reason      TEXT
);
