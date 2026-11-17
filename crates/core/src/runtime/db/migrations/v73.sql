-- The fingerprint of every key revoked through key management, so that the
-- bootstrap `authorized_keys` file, read on each start, does not authorise a
-- revoked key again.
CREATE TABLE revoked_keys (
    fingerprint TEXT NOT NULL PRIMARY KEY,
    revoked_at  INTEGER NOT NULL
);
