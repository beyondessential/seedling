-- What the runtime knows of each gate an app declares: the digest of the key
-- of its last success and the generation that run was for, whether that
-- success still stands, the runs that failed since, and the key a cancelled
-- run paused it for. Only a digest of the key is held, since a key may carry
-- a secret.
CREATE TABLE gate_records (
    app                  TEXT    NOT NULL,
    gate                 TEXT    NOT NULL,
    key_digest           BLOB,
    success_generation   INTEGER,
    valid                INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_failure_at      TEXT,
    last_failure         TEXT,
    paused_key_digest    BLOB,
    PRIMARY KEY (app, gate)
);

-- The resources a gate's run holds stopped, so that the hold outlasts a
-- restart of the runtime and a failed run.
CREATE TABLE gate_holds (
    app  TEXT NOT NULL,
    gate TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (app, gate, kind, name)
);

-- An app that was installed before gates ran its setup then.
ALTER TABLE registered_apps ADD COLUMN set_up INTEGER NOT NULL DEFAULT 0;
UPDATE registered_apps SET set_up = installed;
