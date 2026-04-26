-- r[impl tls.dns-provider.lifecycle]
CREATE TABLE IF NOT EXISTS tls_dns_providers (
    name              TEXT PRIMARY KEY,
    kind              TEXT NOT NULL,
    config_ciphertext BLOB NOT NULL,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

-- r[impl tls.csr.flow]
-- r[impl tls.strategy.manual]
CREATE TABLE IF NOT EXISTS tls_certificates (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname        TEXT NOT NULL,
    state           TEXT NOT NULL
        CHECK (state IN ('csr_pending', 'active', 'superseded', 'failed')),
    cert_pem        TEXT,
    csr_pem         TEXT,
    key_ciphertext  BLOB NOT NULL,
    key_type        TEXT NOT NULL,
    issuer          TEXT,
    not_before      INTEGER,
    not_after       INTEGER,
    serial          TEXT,
    self_signed     INTEGER NOT NULL DEFAULT 0,
    note            TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS tls_certificates_hostname_state
    ON tls_certificates (hostname, state);

-- r[impl tls.strategy.acme-dns]
-- r[impl tls.strategy.manual]
-- r[impl tls.policy.apply]
-- One row per hostname with a non-default policy. Hostnames absent from
-- this table use the HTTP-01 ACME default per tls.strategy.default.
CREATE TABLE IF NOT EXISTS tls_policies (
    hostname     TEXT PRIMARY KEY,
    strategy     TEXT NOT NULL
        CHECK (strategy IN ('acme_dns', 'manual')),
    dns_provider TEXT REFERENCES tls_dns_providers(name) ON DELETE RESTRICT,
    cert_id      INTEGER REFERENCES tls_certificates(id) ON DELETE RESTRICT,
    updated_at   INTEGER NOT NULL,
    CHECK (
        (strategy = 'acme_dns' AND dns_provider IS NOT NULL AND cert_id IS NULL) OR
        (strategy = 'manual'   AND cert_id IS NOT NULL AND dns_provider IS NULL)
    )
);
