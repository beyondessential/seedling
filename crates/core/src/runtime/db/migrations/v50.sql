-- r[impl ingress.site.tailscale]
-- Allow `origin = 'tailscale'` on tls_certificates rows that the runtime
-- obtained from the host's local Tailscale facility. SQLite doesn't support
-- altering an existing CHECK constraint in place, so we rebuild the table
-- (mirrors the v40 site_service_endpoints reshape).
--
-- Foreign keys from tls_policies and tls_cert_attempts reference this table
-- by name, so renaming the rebuilt copy back to `tls_certificates` keeps
-- those references intact.
CREATE TABLE tls_certificates_new (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname           TEXT NOT NULL,
    state              TEXT NOT NULL
        CHECK (state IN ('csr_pending', 'active', 'superseded', 'failed')),
    cert_pem           TEXT,
    csr_pem            TEXT,
    key_ciphertext     BLOB NOT NULL,
    key_type           TEXT NOT NULL,
    issuer             TEXT,
    not_before         INTEGER,
    not_after          INTEGER,
    serial             TEXT,
    self_signed        INTEGER NOT NULL DEFAULT 0,
    note               TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    origin             TEXT NOT NULL DEFAULT 'manual'
        CHECK (origin IN ('manual', 'csr', 'acme_dns', 'tailscale')),
    acme_account_id    INTEGER REFERENCES tls_acme_accounts(id) ON DELETE RESTRICT,
    ari_window_start   INTEGER,
    ari_window_end     INTEGER,
    ari_polled_at      INTEGER
);

INSERT INTO tls_certificates_new
    (id, hostname, state, cert_pem, csr_pem, key_ciphertext, key_type,
     issuer, not_before, not_after, serial, self_signed, note,
     created_at, updated_at, origin, acme_account_id,
     ari_window_start, ari_window_end, ari_polled_at)
SELECT
    id, hostname, state, cert_pem, csr_pem, key_ciphertext, key_type,
    issuer, not_before, not_after, serial, self_signed, note,
    created_at, updated_at, origin, acme_account_id,
    ari_window_start, ari_window_end, ari_polled_at
FROM tls_certificates;

DROP TABLE tls_certificates;
ALTER TABLE tls_certificates_new RENAME TO tls_certificates;

CREATE INDEX IF NOT EXISTS tls_certificates_hostname_state
    ON tls_certificates (hostname, state);
