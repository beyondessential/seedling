-- r[impl tls.strategy.canopy]
-- r[impl tls.strategy.canopy-only]
-- r[impl tls.canopy.key]
-- r[impl tls.canopy.revoked]
-- r[impl tls.chain.ladder]
-- Canopy as a certificate source.
--
--   * `tls_policies.strategy` gains 'canopy', the Canopy-only policy.
--   * `tls_certificates.origin` gains 'canopy', and `state` gains 'revoked',
--     which every "state = 'active'" filter in the serving and decision paths
--     already excludes, so a revoked certificate stops being served and stops
--     counting as usable without each of them learning a new rule.
--   * `tls_chain_state` records where a hostname has got to in its strategy
--     chain, for the cases the clock alone cannot answer.
--
-- SQLite cannot alter a CHECK constraint in place, so both tables are rebuilt,
-- as v50 did for `origin`. Three things differ from v50:
--
--   * `requested_hostname` (added after v50) is carried over.
--   * Dropping `tls_certificates` while foreign keys are on performs an implicit
--     DELETE, and `tls_cert_attempts.cert_id` is ON DELETE SET NULL, so the
--     attempt log would lose which certificate each attempt produced. The ids
--     are saved first and put back after the rebuild.
--   * `tls_policies.cert_id` was ON DELETE RESTRICT, so a policy row carrying a
--     certificate id would block the drop. Only the `manual` rows carried one,
--     the code has ignored those since manual certificates began binding by
--     coverage, and the column keeps its value but no longer references the
--     table.

CREATE TEMP TABLE _tls_attempt_cert_ids AS
    SELECT id, cert_id FROM tls_cert_attempts WHERE cert_id IS NOT NULL;

CREATE TABLE tls_policies_new (
    hostname     TEXT PRIMARY KEY,
    strategy     TEXT NOT NULL
        CHECK (strategy IN ('acme_dns', 'manual', 'canopy')),
    dns_provider TEXT REFERENCES tls_dns_providers(name) ON DELETE RESTRICT,
    cert_id      INTEGER,
    updated_at   INTEGER NOT NULL,
    CHECK (
        (strategy = 'acme_dns' AND dns_provider IS NOT NULL AND cert_id IS NULL) OR
        (strategy = 'manual'   AND cert_id IS NOT NULL AND dns_provider IS NULL) OR
        (strategy = 'canopy'   AND cert_id IS NULL AND dns_provider IS NULL)
    )
);

INSERT INTO tls_policies_new (hostname, strategy, dns_provider, cert_id, updated_at)
SELECT hostname, strategy, dns_provider, cert_id, updated_at FROM tls_policies;

DROP TABLE tls_policies;
ALTER TABLE tls_policies_new RENAME TO tls_policies;

CREATE TABLE tls_certificates_new (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname           TEXT NOT NULL,
    state              TEXT NOT NULL
        CHECK (state IN ('csr_pending', 'active', 'superseded', 'failed', 'revoked')),
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
        CHECK (origin IN ('manual', 'csr', 'acme_dns', 'tailscale', 'canopy')),
    acme_account_id    INTEGER REFERENCES tls_acme_accounts(id) ON DELETE RESTRICT,
    ari_window_start   INTEGER,
    ari_window_end     INTEGER,
    ari_polled_at      INTEGER,
    requested_hostname TEXT
);

INSERT INTO tls_certificates_new
    (id, hostname, state, cert_pem, csr_pem, key_ciphertext, key_type,
     issuer, not_before, not_after, serial, self_signed, note,
     created_at, updated_at, origin, acme_account_id,
     ari_window_start, ari_window_end, ari_polled_at, requested_hostname)
SELECT
    id, hostname, state, cert_pem, csr_pem, key_ciphertext, key_type,
    issuer, not_before, not_after, serial, self_signed, note,
    created_at, updated_at, origin, acme_account_id,
    ari_window_start, ari_window_end, ari_polled_at, requested_hostname
FROM tls_certificates;

DROP TABLE tls_certificates;
ALTER TABLE tls_certificates_new RENAME TO tls_certificates;

CREATE INDEX IF NOT EXISTS tls_certificates_hostname_state
    ON tls_certificates (hostname, state);

UPDATE tls_cert_attempts
SET cert_id = (SELECT s.cert_id FROM _tls_attempt_cert_ids s WHERE s.id = tls_cert_attempts.id)
WHERE id IN (SELECT id FROM _tls_attempt_cert_ids);

DROP TABLE _tls_attempt_cert_ids;

-- One row per hostname with a strategy chain in progress.
--
-- A chain's position is a function of the certificate being replaced and the
-- clock, so a row is only written when something moves a hostname ahead of the
-- clock (a refusal, a revocation) or must outlive the certificate (an open
-- order). The position the clock implies is computed from these fields, never
-- stored, so a restart resumes exactly where it was.
--
--   replacing_cert_id   the certificate whose replacement this cycle is, or
--                       NULL when the hostname had none. A row whose id no
--                       longer matches the serving certificate belongs to an
--                       earlier cycle and is ignored.
--   replacing_not_after the expiry of that certificate, kept so the ladder can
--                       still be measured after the certificate is revoked or
--                       deleted. NULL when there was none.
--   step                the index in the chain of the strategy responsible as of
--                       `step_since`.
--   step_since          when that strategy became responsible, unix seconds.
--   open_order_since    when an order with the responsible strategy was opened,
--                       or NULL when none is open. A hostname stays with a
--                       strategy while it has an order open.
--   last_error          why the responsible strategy last failed to produce a
--                       certificate, or NULL when its last attempt did not fail.
--                       Cleared when a certificate arrives.
--   retry_not_before    the earliest the responsible strategy is asked again after
--                       a refusal or a rejected request, or NULL when it may be
--                       asked on the next opportunity.
CREATE TABLE tls_chain_state (
    hostname            TEXT PRIMARY KEY,
    replacing_cert_id   INTEGER,
    replacing_not_after INTEGER,
    step                INTEGER NOT NULL,
    step_since          INTEGER NOT NULL,
    open_order_since    INTEGER,
    last_error          TEXT,
    retry_not_before    INTEGER,
    updated_at          INTEGER NOT NULL
);
