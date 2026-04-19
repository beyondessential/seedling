CREATE TABLE IF NOT EXISTS authorized_keys (
    fingerprint TEXT    PRIMARY KEY,
    label       TEXT    NOT NULL,
    added_at    INTEGER NOT NULL
);
