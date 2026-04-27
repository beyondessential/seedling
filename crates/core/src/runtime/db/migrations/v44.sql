-- r[impl tls.settings.contact-email]
-- Global TLS settings, currently just the operator contact email used
-- whenever the runtime registers an ACME account. One row enforced via
-- the singleton primary key. Defaults to empty so the daemon starts up
-- cleanly even before an operator configures one.
CREATE TABLE IF NOT EXISTS tls_settings (
    singleton     INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    contact_email TEXT    NOT NULL DEFAULT '',
    updated_at    INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO tls_settings (singleton, contact_email, updated_at)
    VALUES (1, '', 0);
