-- r[impl tls.acme.account.persist]
-- Persisted ACME account state, one row per (directory_url, contact_email).
-- account_key_ciphertext stores the PKCS#8-encoded EC P-256 private key for
-- the account, encrypted with the secret key. account_url is the URL
-- returned by the directory's newAccount endpoint and is required to issue
-- subsequent orders without re-registering.
CREATE TABLE IF NOT EXISTS tls_acme_accounts (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    directory_url          TEXT NOT NULL,
    contact_email          TEXT NOT NULL,
    account_key_ciphertext BLOB NOT NULL,
    account_url            TEXT NOT NULL,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    UNIQUE (directory_url, contact_email)
);

-- r[impl tls.cert.metadata]
-- Identifies how the cert was obtained, so the renewal task knows which
-- rows it owns. 'manual' for operator-uploaded certs, 'csr' for certs
-- issued against a server-generated keypair, 'acme_dns' for certs the
-- daemon obtained via ACME-DNS.
ALTER TABLE tls_certificates
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual'
    CHECK (origin IN ('manual', 'csr', 'acme_dns'));

-- For acme_dns rows, the ACME account that issued the certificate. NULL for
-- manual and csr rows. ON DELETE RESTRICT is intentional: deleting an
-- account that has live certs would leave them un-renewable.
ALTER TABLE tls_certificates
    ADD COLUMN acme_account_id INTEGER REFERENCES tls_acme_accounts(id) ON DELETE RESTRICT;
